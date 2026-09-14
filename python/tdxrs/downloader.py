"""批量数据下载服务

多服务器分发 + 自动翻页 + 限流 + 断点续传。

默认输出 TDX 二进制 .day 格式，可被 DailyBarReader 直接读取。
支持 csv 和 parquet 格式 (需用户设置)。

复权规则:
- fq=0 (默认): 下载原始数据，支持增量更新
- fq=1 (前复权): 下载复权数据，不支持增量更新，需全量覆盖
- fq=2 (后复权): 同 fq=1

使用方式::

    from tdxrs.downloader import Downloader

    # 默认 .day 格式，原始数据，支持增量更新
    dl = Downloader(data_dir="./data")
    dl.run(markets=["sh", "sz"], categories=["daily"])
    dl.update()  # 增量更新

    # 前复权数据 (不支持增量更新)
    dl = Downloader(data_dir="./data", fq=1)
    dl.run(markets=["sh"], categories=["daily"])

    # CSV 格式
    dl = Downloader(data_dir="./data", format="csv")

    # 按日下载分时/逐笔 (协议原生日期查询，codes 为必填)
    dl.download_minute("2026-06-25", codes=["600519", "000858"])
    dl.download_ticks(["2026-06-25", "2026-06-24"], codes=["600519"])
"""

import csv
import json
import os
import time
from datetime import datetime
from pathlib import Path

from tdxrs._internal import TdxDirectClient
from tdxrs.constants import (
    MARKET_SH, MARKET_SZ, MARKET_BJ,
    KLINE_5MIN, KLINE_15MIN, KLINE_30MIN, KLINE_1HOUR,
    KLINE_DAILY, KLINE_WEEKLY, KLINE_MONTHLY,
)

# 板块文件名
BLOCK_ZS = "block_zs.dat"

# 周期配置: (category_code, dir_name, max_per_request)
_CATEGORY_MAP = {
    "daily":   (KLINE_DAILY,   "daily",  800),
    "weekly":  (KLINE_WEEKLY,  "weekly", 800),
    "monthly": (KLINE_MONTHLY, "monthly", 800),
    "min5":    (KLINE_5MIN,    "min5",   800),
    "min15":   (KLINE_15MIN,   "min15",  800),
    "min30":   (KLINE_30MIN,   "min30",  800),
    "min60":   (KLINE_1HOUR,   "min60",  800),
}

# 默认服务器列表 (与 Rust PRIMARY_SERVERS 一致)
_DEFAULT_SERVERS = [
    ("海通8",       "58.63.254.191", 7709),
    ("广发1",       "119.29.19.242", 7709),
    ("华林4",       "202.96.138.90", 7709),
    ("广发13",      "183.60.224.177", 7709),
    ("杭州联通J2",  "60.12.136.250", 7709),
    ("华林5",       "218.106.92.182", 7709),
    ("海通2",       "175.6.5.153", 7709),
    ("海通4",       "182.131.3.245", 7709),
    ("杭州电信J3",  "218.75.126.9", 7709),
    ("上海电信Z1",  "180.153.18.170", 7709),
]


def _get_phase():
    """判断当前交易阶段 (简化: 不考量假期, 午休视为交易中)

    Returns
    -------
    str
        "trading"    — 交易时段 (工作日 9:30-15:00)
        "pre_post"   — 盘前/盘后 (工作日非交易时段)
        "closed"     — 休市 (周末)
    """
    now = datetime.now()
    if now.weekday() >= 5:
        return "closed"
    t = now.hour * 60 + now.minute
    if 9 * 60 + 30 <= t <= 15 * 60:
        return "trading"
    return "pre_post"


# 限流分档: phase → (日K乘数, 分时乘数)
# base_rate * multiplier = effective_rate, 上限 200
_RATE_MULTIPLIER = {
    "trading":  (1.0, 1.0),   # 交易时段: 保持基础限流
    "pre_post": (2.0, 1.5),   # 盘前盘后: 放宽
    "closed":   (4.0, 3.0),   # 休市:     大幅放宽
}


class ServerPool:
    """多服务器连接池，轮转分发请求

    每个服务器维护独立的 TdxDirectClient 实例和限流状态。
    根据交易阶段自动调整限流: 交易时段保守，休市时段放宽。

    ## 限流设计

    - **粒度**: 按 ``next_client()`` 调用次数计算，不是按 TCP 握手次数。
      批量查询 (如 ``get_security_quotes`` 传入 N 只股票) 仅算 1 次请求。
      循环调用 ``get_security_bars`` 则每次算独立请求。
    - **Per-server**: 每个服务器独立计时 (``_last_request[i]``)，
      5 服务器轮转时全局吞吐 = per_server_rate × 5。
    - **线程安全**: 当前实现非线程安全。单线程 + 5 服务器轮转已隐含
      5× 并行，多线程收益有限 (同步阻塞 + GIL)。
    """

    def __init__(self, servers=None, rate_limit=15, phase=None):
        """
        Parameters
        ----------
        servers : list[tuple[str, str, int]]
            [(名称, IP, 端口), ...]，默认使用内置服务器列表。
        rate_limit : int
            每服务器每秒请求数上限 (交易时段基准值，其他阶段自动放大)。
        phase : str | None
            强制指定交易阶段 ("trading"/"pre_post"/"closed")，
            None 则自动检测。
        """
        if servers is None:
            servers = _DEFAULT_SERVERS
        self._servers = servers
        self._phase = phase or _get_phase()
        mult, mult_min = _RATE_MULTIPLIER.get(self._phase, (1.0, 1.0))
        base = min(rate_limit, 200)
        self._rate_limit = min(int(base * mult), 200)
        self._minute_rate_limit = min(int(10 * mult_min), 200)
        self._clients = []
        self._last_request = [0.0] * len(servers)
        self._idx = 0

        for name, ip, port in servers:
            c = TdxDirectClient(ip, port, 15.0)
            self._clients.append((name, c))

    @property
    def phase(self):
        return self._phase

    @property
    def server_count(self):
        return len(self._clients)

    def _wait(self, server_idx, is_minute=False):
        """单服务器限流等待 (per-server 独立计时，5 服务器轮转时全局吞吐 ×5)"""
        rps = self._minute_rate_limit if is_minute else self._rate_limit
        if rps <= 0:
            return
        min_interval = 1.0 / rps
        elapsed = time.monotonic() - self._last_request[server_idx]
        if elapsed < min_interval:
            time.sleep(min_interval - elapsed)
        self._last_request[server_idx] = time.monotonic()

    def next_client(self, is_minute=False):
        """轮转获取下一个可用客户端

        Returns
        -------
        tuple[str, TdxDirectClient]
            (服务器名称, 客户端实例)
        """
        idx = self._idx % len(self._clients)
        self._idx += 1
        self._wait(idx, is_minute)
        return self._clients[idx]


class Downloader:
    """批量数据下载器

    多服务器分发 + 自动翻页 + 增量更新 + 断点续传。
    根据交易阶段自动调整限流速率。

    Parameters
    ----------
    data_dir : str
        数据存储根目录。
    servers : list[tuple] | None
        服务器列表 [(名称, IP, 端口), ...]，None 使用默认。
    rate_limit : int
        每服务器每秒请求数 (交易时段基准值，非交易时段自动放大)。
    format : str
        输出格式: "tdx"(默认, 可被 DailyBarReader 直接读取) / "csv" / "parquet"。
    fq : int
        复权类型: 0=不复权, 1=前复权, 2=后复权。
    source : str
        行情数据源: "v2"(默认, 新一代 7709 协议，旧协议 K 线已断供) / "old"。
        v2 复权 = 原始K线 + 旧协议除权记录本地调整；北交所仍走旧协议（市场字节未验证）；
        股票列表/元数据始终走旧协议目录类 API。

    Example
    -------
    ::

        dl = Downloader(data_dir="./data")  # 默认 .day 格式
        dl.run(markets=["sh"], categories=["daily"], codes=["600519"])

        # CSV 格式
        dl = Downloader(data_dir="./data", format="csv")

        # Parquet 格式 (需要 pyarrow)
        dl = Downloader(data_dir="./data", format="parquet")
    """

    def __init__(self, data_dir="./data", servers=None, rate_limit=15,
                 format="tdx", fq=0, source="v2", v2_host=None):
        # 路径标准化: 展开 ~ / ~user, 解析为绝对路径
        self.data_dir = Path(data_dir).expanduser().resolve()
        self.format = format
        self.fq = fq
        self.source = source
        if v2_host is None:
            from tdxrs.v2_client import _DEFAULT_HOST as _V2_HOST
            v2_host = _V2_HOST
        self.v2_host = v2_host
        self.pool = ServerPool(servers=servers, rate_limit=rate_limit)

        # 元数据目录
        self._meta_dir = self.data_dir / ".tdxrs_meta"
        self._meta_dir.mkdir(parents=True, exist_ok=True)

        self._checkpoint_path = self._meta_dir / "checkpoint.json"
        self._sync_path = self._meta_dir / "last_sync.json"

        self._stats = {"done": 0, "skipped": 0, "failed": 0, "errors": []}

    # ================================================================
    # 公共 API
    # ================================================================

    def run(self, markets=None, categories=None, codes=None,
            start_date=None, end_date=None):
        """全量下载

        Parameters
        ----------
        markets : list[str] | None
            市场列表 ["sh", "sz", "bj"]，None = 全部。
        categories : list[str] | None
            周期列表 ["daily", "weekly", "min5", ...]，None = ["daily"]。
        codes : list[str] | None
            股票代码列表，None = 全市场。
        start_date : str | None
            起始日期 "YYYY-MM-DD"，None = 从头开始。
        end_date : str | None
            结束日期 "YYYY-MM-DD"，None = 到最新。
        """
        if markets is None:
            markets = ["sh", "sz"]
        if categories is None:
            categories = ["daily"]

        market_map = {"sh": MARKET_SH, "sz": MARKET_SZ, "bj": MARKET_BJ}

        phase_labels = {"trading": "交易时段", "pre_post": "盘前盘后", "closed": "休市"}
        p = self.pool.phase
        print(f"[INFO] 限流模式: {phase_labels.get(p, p)}"
              f" (日K {self.pool._rate_limit} req/s, 分时 {self.pool._minute_rate_limit} req/s)")

        for cat_name in categories:
            if cat_name not in _CATEGORY_MAP:
                print(f"[WARN] 未知周期 '{cat_name}'，跳过")
                continue

            cat_code, dir_name, max_per_req = _CATEGORY_MAP[cat_name]
            is_minute = cat_code < 4  # 0-3 是分钟级

            for market_name in markets:
                market = market_map.get(market_name)
                if market is None:
                    print(f"[WARN] 未知市场 '{market_name}'，跳过")
                    continue

                # 获取股票列表
                if codes is None:
                    stock_list = self._fetch_stock_list(market)
                else:
                    stock_list = [(market, c) for c in codes]

                total = len(stock_list)
                print(f"[INFO] {market_name}/{dir_name}: {total} 只股票")

                for i, (mkt, code) in enumerate(stock_list):
                    try:
                        n = self._download_one(mkt, code, cat_code, dir_name,
                                               max_per_req, is_minute,
                                               start_date=start_date,
                                               end_date=end_date)
                        self._stats["done"] += 1
                        if n > 0:
                            print(f"  [{i+1}/{total}] {code}: +{n} 条")
                        else:
                            self._stats["skipped"] += 1
                    except Exception as e:
                        self._stats["failed"] += 1
                        self._stats["errors"].append(f"{code}: {e}")
                        print(f"  [{i+1}/{total}] {code}: ERROR {e}")

                    # 每 50 只保存进度
                    if (i + 1) % 50 == 0:
                        self._save_checkpoint(market_name, dir_name, code, i + 1, total)

                self._save_checkpoint(market_name, dir_name, "", total, total)

        self._print_summary()

    def update(self, markets=None, categories=None, codes=None,
               start_date=None, end_date=None):
        """增量更新 — 仅下载 last_sync 中缺失的日期

        仅 fq=0 (原始数据) 支持增量更新。
        fq>0 (复权数据) 不支持增量更新，请使用 run() 全量覆盖。

        与 run() 的区别:
        - codes=None 时，只更新 sync 记录中存在的股票
        - 不会下载之前未下载的股票

        Parameters
        ----------
        markets : list[str] | None
        categories : list[str] | None
        codes : list[str] | None
            股票代码列表，None = 仅更新 sync 中已有股票。
        start_date : str | None
            起始日期 "YYYY-MM-DD"，None = 从上次同步位置开始。
        end_date : str | None
            结束日期 "YYYY-MM-DD"，None = 到最新。
        """
        if self.fq != 0:
            print("[ERROR] 增量更新仅支持 fq=0 (原始数据)。"
                  "复权数据请使用 run() 全量覆盖。")
            return

        sync_data = self._load_sync()
        if not sync_data and codes is None:
            print("[INFO] 无历史同步记录，执行全量下载")
            self.run(markets, categories, codes=None,
                     start_date=start_date, end_date=end_date)
            return

        # 暂存原始 sync 数据，下载完成后合并
        self._sync_data = sync_data

        # 如果未指定 codes，从 sync 记录中获取已下载的股票
        if codes is None and sync_data:
            # 从 sync key 格式 "sh/600519" 解析出市场和代码
            sync_codes_by_market = {}
            for key in sync_data:
                parts = key.split("/")
                if len(parts) == 2:
                    market_name, code = parts
                    if market_name not in sync_codes_by_market:
                        sync_codes_by_market[market_name] = []
                    sync_codes_by_market[market_name].append(code)

            # 按市场分别更新
            if markets is None:
                markets = list(sync_codes_by_market.keys())

            for market_name in markets:
                market_codes = sync_codes_by_market.get(market_name, [])
                if market_codes:
                    self.run(markets=[market_name], categories=categories,
                             codes=market_codes, start_date=start_date,
                             end_date=end_date)
        else:
            self.run(markets=markets, categories=categories, codes=codes,
                     start_date=start_date, end_date=end_date)

    def progress(self):
        """返回当前统计信息"""
        return dict(self._stats)

    def run_xdxr(self, markets=None, codes=None):
        """下载除权除息数据

        保存为 CSV 文件: {data_dir}/xdxr/{market}/{code}.csv

        Parameters
        ----------
        markets : list[str] | None
            市场列表 ["sh", "sz"]，None = ["sh", "sz"]。
        codes : list[str] | None
            股票代码列表，None = 全市场。
        """
        # 2026-07 起服务端对旧协议除权功能族静默断供：先探针一只，全空则直接报错，
        # 避免对全市场数千只股票做无效扫描。
        market_map = {"sh": MARKET_SH, "sz": MARKET_SZ}
        if markets is None:
            markets = ["sh", "sz"]
        probe_mkt = market_map.get(markets[0], MARKET_SH)
        probe_code = codes[0] if codes else "600519"
        name, client = self.pool.next_client()
        if not client.get_xdxr_info(probe_mkt, probe_code):
            print("[ERROR] 除权除息下载不可用——服务端已断供旧协议除权功能族，"
                  "新协议除权接口尚未逆向（2026-09 线缆级逆向结论）。")
            return

        for market_name in markets:
            market = market_map.get(market_name)
            if market is None:
                continue

            if codes is None:
                stock_list = self._fetch_stock_list(market)
            else:
                stock_list = [(market, c) for c in codes]

            total = len(stock_list)
            print(f"[INFO] {market_name}/xdxr: {total} 只股票")

            out_dir = self.data_dir / "xdxr" / market_name
            out_dir.mkdir(parents=True, exist_ok=True)

            for i, (mkt, code) in enumerate(stock_list):
                try:
                    name, client = self.pool.next_client()
                    data = client.get_xdxr_info(mkt, code)
                    if data:
                        self._write_xdxr_csv(out_dir / f"{code}.csv", data)
                        self._stats["done"] += 1
                        print(f"  [{i+1}/{total}] {code}: {len(data)} 条")
                    else:
                        self._stats["skipped"] += 1
                except Exception as e:
                    self._stats["failed"] += 1
                    self._stats["errors"].append(f"{code}: {e}")
                    print(f"  [{i+1}/{total}] {code}: ERROR {e}")

        self._print_summary()

    def _write_xdxr_csv(self, path, data):
        """写入 XDXR CSV 文件"""
        with open(path, "w", newline="", encoding="utf-8") as f:
            writer = csv.writer(f)
            writer.writerow(["date", "category", "fenhong", "peigujia",
                             "songzhuangu", "peigu", "suogu"])
            for d in data:
                year = d.get("year", 0)
                month = d.get("month", 0)
                day = d.get("day", 0)
                date_str = f"{year}-{month:02d}-{day:02d}" if year else ""
                writer.writerow([
                    date_str,
                    d.get("category", 0),
                    d.get("fenhong") or 0,
                    d.get("peigujia") or 0,
                    d.get("songzhuangu") or 0,
                    d.get("peigu") or 0,
                    d.get("suogu") or 0,
                ])

    # ================================================================
    # 内部实现
    # ================================================================

    def _fetch_stock_list(self, market):
        """获取全市场股票列表"""
        name, client = self.pool.next_client()
        count = client.get_security_count(market)
        stocks = []
        page_size = 1000
        for start in range(0, count, page_size):
            page = client.get_security_list(market, start)
            if not page:
                break
            for item in page:
                stocks.append((market, item["code"]))
        return stocks

    def _download_one(self, market, code, category, dir_name, max_per_req, is_minute,
                      start_date=None, end_date=None):
        """下载单只股票的一个周期数据

        fq=0 时支持增量更新，fq>0 时全量覆盖。

        Parameters
        ----------
        market : int
        code : str
        category : int
        dir_name : str
        max_per_req : int
        is_minute : bool
        start_date : str | None
            起始日期 "YYYY-MM-DD"，None = 从头开始。
        end_date : str | None
            结束日期 "YYYY-MM-DD"，None = 到最新。

        Returns
        -------
        int
            新增记录数。0 表示无新数据。
        """
        # 确定市场目录
        if market == MARKET_SH:
            market_dir = "sh"
        elif market == MARKET_SZ:
            market_dir = "sz"
        elif market == MARKET_BJ:
            market_dir = "bj"
        else:
            market_dir = "sz"  # 默认
        out_dir = self.data_dir / market_dir / dir_name
        out_dir.mkdir(parents=True, exist_ok=True)
        ext = {"tdx": "day", "csv": "csv", "parquet": "parquet"}.get(self.format, "day")
        out_path = out_dir / f"{code}.{ext}"

        # 增量模式 (fq=0): 读取已有数据的最后日期
        existing_last_date = None
        if self.fq == 0 and out_path.exists():
            existing_last_date = self._get_existing_last_date(out_path)

        # 自动翻页拉取
        all_bars = []
        if self.source == "v2" and market != MARKET_BJ:
            # v2 数据源：新协议市场字节未验证北交所(92xxxx)，北交走旧协议
            raw_bars = self._fetch_bars_v2_raw(market, code, category,
                                               min(max_per_req, 700), is_minute)
            if raw_bars and self.fq != 0:
                # 旧协议除权目录接口仍存活：全量历史自身即上下文，本地复权
                from tdxrs import adjust as fq_adjust
                name, client = self.pool.next_client(is_minute)
                xdxr_list = client.get_xdxr_info(market, code) or []
                raw_bars = fq_adjust.adjust_bars(raw_bars, [], xdxr_list, self.fq)
            all_bars = [self._bar_to_dict(b, is_minute) for b in raw_bars]
        else:
            offset = 0
            while True:
                name, client = self.pool.next_client(is_minute)
                bars = client.get_security_bars(
                    category, market, code, offset, max_per_req, self.fq
                )
                if not bars:
                    break
                all_bars.extend(bars)
                if len(bars) < max_per_req:
                    break
                offset += max_per_req

        if not all_bars:
            return 0

        # 日期范围过滤
        if start_date:
            all_bars = [b for b in all_bars if b["datetime"][:10] >= start_date]
        if end_date:
            all_bars = [b for b in all_bars if b["datetime"][:10] <= end_date]

        # 增量过滤 (fq=0): 仅保留比已有数据更新的记录
        if existing_last_date:
            all_bars = [b for b in all_bars if b["datetime"][:10] > existing_last_date]
            if not all_bars:
                return 0

        # 写入文件
        if self.format == "csv":
            self._write_csv(out_path, all_bars, append=existing_last_date is not None)
        elif self.format == "parquet":
            self._write_parquet(out_path, all_bars)
        else:
            # TDX 二进制格式: 全量重写 (不支持追加)
            self._write_tdx(out_path, all_bars)

        # 更新同步记录 (仅 fq=0)
        if all_bars and self.fq == 0:
            last_date = all_bars[-1]["datetime"][:10]
            self._update_sync(market, code, dir_name, last_date)

        return len(all_bars)

    def _fetch_bars_v2_raw(self, market, code, category, max_per_req, is_minute=False):
        """v2 协议翻页拉取单只股票 K 线，返回按时间升序的 Bar 列表。

        start 偏移向历史方向前移，窗口实测与最新窗口无缝衔接；
        单次响应上限 700 条（payload u16 长度字段约束）。
        """
        from tdxrs.v2_client import TdxV2Client

        pages = []
        start = 0
        with TdxV2Client(self.v2_host, timeout=15.0) as client:
            while True:
                batch = client.get_bars(market, code, count=max_per_req,
                                        category=category, start=start)
                if not batch:
                    break
                pages.append(batch)
                if len(batch) < max_per_req:
                    break
                start += len(batch)

        bars = []
        for page in reversed(pages):  # start 大的历史页在前 → 整体升序
            bars.extend(page)
        return bars

    def _bar_to_dict(self, b, is_minute=False):
        """v2 Bar → 下载器写入契约 dict。"""
        date_s = f"{b.date // 10000:04d}-{b.date // 100 % 100:02d}-{b.date % 100:02d}"
        if is_minute and b.time_sec:
            date_s += f" {b.time_sec // 3600:02d}:{b.time_sec % 3600 // 60:02d}"
        return {
            "datetime": date_s,
            "open": b.open, "high": b.high, "low": b.low, "close": b.close,
            "amount": b.amount, "vol": b.vol,
        }

    def _get_existing_last_date(self, path):
        """读取已有文件的最后日期"""
        try:
            suffix = path.suffix
            if suffix == ".csv":
                with open(path, "r", encoding="utf-8") as f:
                    lines = f.readlines()
                    if len(lines) < 2:
                        return None
                    last_line = lines[-1].strip()
                    if not last_line:
                        return None
                    return last_line.split(",")[0]  # "2026-06-20"
            elif suffix == ".day":
                # TDX 二进制: 32 字节/条, 前 4 字节是日期
                import struct
                file_size = path.stat().st_size
                if file_size < 32:
                    return None
                with open(path, "rb") as f:
                    f.seek(file_size - 32)
                    data = f.read(4)
                    date_int = struct.unpack("<I", data)[0]
                    if date_int == 0:
                        return None
                    # TDX 日期解码: 两种格式
                    # 1. post-2004: (year-2004)*2048 + month*100 + day
                    # 2. pre-2004: YYYYMMDD
                    if date_int > 100000:
                        # YYYYMMDD 格式 (pre-2004)
                        s = str(date_int)
                        if len(s) == 8:
                            return f"{s[:4]}-{s[4:6]}-{s[6:8]}"
                    else:
                        # TDX 编码格式 (post-2004)
                        year = date_int // 2048 + 2004
                        remainder = date_int % 2048
                        month = remainder // 100
                        day = remainder % 100
                        if 2004 <= year <= 2099 and 1 <= month <= 12 and 1 <= day <= 31:
                            return f"{year}-{month:02d}-{day:02d}"
            elif suffix == ".parquet":
                try:
                    import pyarrow.parquet as pq
                    table = pq.read_table(path, columns=["date"])
                    dates = table.column("date").to_pylist()
                    return dates[-1] if dates else None
                except Exception:
                    return None
        except Exception:
            return None

    def _write_csv(self, path, bars, append=False):
        """写入 CSV 文件"""
        mode = "a" if append else "w"
        with open(path, mode, newline="", encoding="utf-8") as f:
            writer = csv.writer(f)
            if not append:
                writer.writerow(["date", "open", "high", "low", "close", "amount", "volume"])
            for bar in bars:
                dt = bar["datetime"]
                date_part = dt[:10] if len(dt) >= 10 else dt
                writer.writerow([
                    date_part,
                    f"{bar['open']:.2f}",
                    f"{bar['high']:.2f}",
                    f"{bar['low']:.2f}",
                    f"{bar['close']:.2f}",
                    f"{bar['amount']:.2f}",
                    int(bar["vol"]),
                ])

    def _write_tdx(self, path, bars):
        """写入 TDX 二进制格式 (.day) — 可被 DailyBarReader 直接读取"""
        import struct
        with open(path, "wb") as f:
            for bar in bars:
                dt = bar["datetime"]
                # TDX 日期编码: (year-2004)*2048 + month*100 + day
                # 注意: 2004年前的数据使用原始 YYYYMMDD 格式存储
                year = int(dt[:4])
                month = int(dt[5:7])
                day = int(dt[8:10])
                if year >= 2004:
                    date_int = (year - 2004) * 2048 + month * 100 + day
                else:
                    # 2004年前: 使用 YYYYMMDD 格式 (与 tdxpy 兼容)
                    date_int = year * 10000 + month * 100 + day
                f.write(struct.pack(
                    "<IIIIIfII",
                    date_int,
                    int(bar["open"] * 100),
                    int(bar["high"] * 100),
                    int(bar["low"] * 100),
                    int(bar["close"] * 100),
                    bar["amount"],
                    int(bar["vol"]),
                    0,  # reserved
                ))

    def _write_parquet(self, path, bars):
        """写入 Parquet 格式 (需要 pyarrow)"""
        try:
            import pyarrow as pa
            import pyarrow.parquet as pq
        except ImportError:
            raise ImportError("parquet 格式需要 pyarrow: pip install pyarrow")

        dates, opens, highs, lows, closes, amounts, volumes = [], [], [], [], [], [], []
        for bar in bars:
            dt = bar["datetime"]
            dates.append(dt[:10] if len(dt) >= 10 else dt)
            opens.append(bar["open"])
            highs.append(bar["high"])
            lows.append(bar["low"])
            closes.append(bar["close"])
            amounts.append(bar["amount"])
            volumes.append(int(bar["vol"]))

        table = pa.table({
            "date": pa.array(dates, type=pa.utf8()),
            "open": pa.array(opens, type=pa.float64()),
            "high": pa.array(highs, type=pa.float64()),
            "low": pa.array(lows, type=pa.float64()),
            "close": pa.array(closes, type=pa.float64()),
            "amount": pa.array(amounts, type=pa.float64()),
            "volume": pa.array(volumes, type=pa.int64()),
        })
        pq.write_table(table, path)

    # ================================================================
    # 分时 / 逐笔按日下载
    # ================================================================

    def download_minute(self, dates, codes, markets=None):
        """按日下载分时数据 (协议原生日期查询)

        每个交易日生成一个 CSV 文件: {data_dir}/{market}/minute/{code}_{date}.csv
        分时数据为当日 242 个时间点 (9:30-11:30 + 13:00-15:00)。

        Parameters
        ----------
        dates : str | int | list[str | int]
            日期，支持 "2026-06-25"、20260625、["2026-06-25", "2026-06-24"] 等。
        codes : list[str]
            股票代码列表，如 ["600519", "000858"]。
        markets : list[str] | None
            市场列表 ["sh", "sz"]，None = ["sh", "sz"]。
            多市场时 codes 中的代码会尝试匹配每个市场。
        """
        dates = self._normalize_dates(dates)
        if not codes:
            print("[ERROR] codes 不能为空，请传入股票代码列表")
            return
        if markets is None:
            markets = ["sh", "sz"]
        market_map = {"sh": MARKET_SH, "sz": MARKET_SZ}

        for market_name in markets:
            market = market_map.get(market_name)
            if market is None:
                continue

            stock_list = [(market, c) for c in codes]
            total = len(stock_list)
            print(f"[INFO] {market_name}/minute: {total} 只股票 x {len(dates)} 天")

            out_dir = self.data_dir / market_name / "minute"
            out_dir.mkdir(parents=True, exist_ok=True)

            for i, (mkt, code) in enumerate(stock_list):
                for date_int in dates:
                    try:
                        name, client = self.pool.next_client(is_minute=True)
                        data = client.get_history_minute_time_data(mkt, code, date_int)
                        if not data:
                            self._stats["skipped"] += 1
                            continue
                        out_path = out_dir / f"{code}_{date_int}.csv"
                        self._write_minute_csv(out_path, data, date_int)
                        self._stats["done"] += 1
                    except Exception as e:
                        self._stats["failed"] += 1
                        self._stats["errors"].append(f"{code}/{date_int}: {e}")
                if (i + 1) % 50 == 0:
                    print(f"  [{i+1}/{total}] {code}")

        self._print_summary()

    def download_ticks(self, dates, codes, markets=None):
        """按日下载逐笔成交数据 (协议原生日期查询)

        每个交易日生成一个 CSV 文件: {data_dir}/{market}/ticks/{code}_{date}.csv
        逐笔成交数据量大，活跃股单日可达数万条。

        Parameters
        ----------
        dates : str | int | list[str | int]
            日期，支持 "2026-06-25"、20260625、["2026-06-25", "2026-06-24"] 等。
        codes : list[str]
            股票代码列表，如 ["600519", "000858"]。
        markets : list[str] | None
            市场列表 ["sh", "sz"]，None = ["sh", "sz"]。
        """
        dates = self._normalize_dates(dates)
        if not codes:
            print("[ERROR] codes 不能为空，请传入股票代码列表")
            return
        if markets is None:
            markets = ["sh", "sz"]
        market_map = {"sh": MARKET_SH, "sz": MARKET_SZ}

        for market_name in markets:
            market = market_map.get(market_name)
            if market is None:
                continue

            stock_list = [(market, c) for c in codes]
            total = len(stock_list)
            print(f"[INFO] {market_name}/ticks: {total} 只股票 x {len(dates)} 天")

            out_dir = self.data_dir / market_name / "ticks"
            out_dir.mkdir(parents=True, exist_ok=True)

            for i, (mkt, code) in enumerate(stock_list):
                for date_int in dates:
                    try:
                        all_ticks = []
                        start = 0
                        max_per_req = 2000
                        while True:
                            name, client = self.pool.next_client(is_minute=True)
                            data = client.get_history_transaction_data(
                                mkt, code, start, max_per_req, date_int
                            )
                            if not data:
                                break
                            all_ticks.extend(data)
                            if len(data) < max_per_req:
                                break
                            start += max_per_req
                        if not all_ticks:
                            self._stats["skipped"] += 1
                            continue
                        out_path = out_dir / f"{code}_{date_int}.csv"
                        self._write_tick_csv(out_path, all_ticks, date_int)
                        self._stats["done"] += 1
                    except Exception as e:
                        self._stats["failed"] += 1
                        self._stats["errors"].append(f"{code}/{date_int}: {e}")
                if (i + 1) % 50 == 0:
                    print(f"  [{i+1}/{total}] {code}")

        self._print_summary()

    @staticmethod
    def _normalize_dates(dates):
        """统一日期格式为 int (YYYYMMDD)

        支持: "2026-06-25"、20260625、["2026-06-25", "20260624"]
        """
        if isinstance(dates, (int, str)):
            dates = [dates]
        result = []
        for d in dates:
            if isinstance(d, int):
                result.append(d)
            elif isinstance(d, str):
                result.append(int(d.replace("-", "")))
        return sorted(set(result))

    def _write_minute_csv(self, path, data, date_int):
        """写入分时 CSV

        A 股每日 242 个时间点: 9:30-11:30 (121) + 13:00-15:00 (121)
        """
        date_str = str(date_int)
        date_fmt = f"{date_str[:4]}-{date_str[4:6]}-{date_str[6:8]}"

        # 生成时间序列 (9:30 开始, 每分钟一个点)
        times = []
        # 上午: 9:30-11:30
        for m in range(121):
            t = 9 * 60 + 30 + m
            times.append(f"{t // 60:02d}:{t % 60:02d}")
        # 下午: 13:00-15:00
        for m in range(121):
            t = 13 * 60 + m
            times.append(f"{t // 60:02d}:{t % 60:02d}")

        with open(path, "w", newline="", encoding="utf-8") as f:
            writer = csv.writer(f)
            writer.writerow(["datetime", "price", "vol"])
            for idx, item in enumerate(data):
                time_str = times[idx] if idx < len(times) else f"idx{idx}"
                writer.writerow([
                    f"{date_fmt} {time_str}",
                    f"{item['price']:.3f}",
                    int(item["vol"]),
                ])

    def _write_tick_csv(self, path, data, date_int):
        """写入逐笔成交 CSV"""
        date_str = str(date_int)
        date_fmt = f"{date_str[:4]}-{date_str[4:6]}-{date_str[6:8]}"

        with open(path, "w", newline="", encoding="utf-8") as f:
            writer = csv.writer(f)
            writer.writerow(["datetime", "price", "vol", "num", "buyorsell"])
            for item in data:
                writer.writerow([
                    f"{date_fmt} {item['time']}",
                    f"{item['price']:.3f}",
                    int(item["vol"]),
                    int(item["num"]),
                    int(item["buyorsell"]),
                ])

    # ================================================================
    # 元数据持久化
    # ================================================================

    def _save_checkpoint(self, market, category, code, done, total):
        """保存断点续传进度"""
        data = {
            "updated_at": datetime.now().isoformat(),
            "market": market,
            "category": category,
            "last_code": code,
            "done": done,
            "total": total,
            "stats": dict(self._stats),
        }
        with open(self._checkpoint_path, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False, indent=2)

    def _load_sync(self):
        """加载增量同步记录"""
        if self._sync_path.exists():
            try:
                with open(self._sync_path, "r", encoding="utf-8") as f:
                    return json.load(f)
            except Exception:
                pass
        return {}

    def _update_sync(self, market, code, category, last_date):
        """更新增量同步记录"""
        # 确定市场名称
        if market == MARKET_SH:
            market_name = "sh"
        elif market == MARKET_SZ:
            market_name = "sz"
        elif market == MARKET_BJ:
            market_name = "bj"
        else:
            market_name = "sz"  # 默认
        key = f"{market_name}/{code}"
        sync = getattr(self, "_sync_data", None) or self._load_sync()
        if key not in sync:
            sync[key] = {}
        sync[key][category] = last_date
        self._sync_data = sync

        # 每 100 只写一次文件
        self._sync_counter = getattr(self, "_sync_counter", 0) + 1
        if self._sync_counter % 100 == 0:
            with open(self._sync_path, "w", encoding="utf-8") as f:
                json.dump(sync, f, ensure_ascii=False, indent=2)

    def _print_summary(self):
        """打印下载摘要"""
        s = self._stats
        print(f"\n{'='*40}")
        print(f"下载完成: {s['done']} 成功, {s['skipped']} 跳过, {s['failed']} 失败")
        if s["errors"]:
            print(f"错误列表 (前 10 条):")
            for e in s["errors"][:10]:
                print(f"  - {e}")
        print(f"{'='*40}")

        # 最终写入 sync 文件
        if hasattr(self, "_sync_data"):
            with open(self._sync_path, "w", encoding="utf-8") as f:
                json.dump(self._sync_data, f, ensure_ascii=False, indent=2)
