"""tdxrs CLI — 命令行快速查询工具

用法:
    tdxrs quote 600519
    tdxrs bars 600519 --count 30
    tdxrs download --market sh
    tdxrs --help
"""

import argparse
import random
import sys
from datetime import datetime

from tdxrs._internal import (
    TdxDirectClient,
    DailyBarReader,
    MinBarReader,
    LcMinBarReader,
    BlockReader,
)
from tdxrs.constants import (
    MARKET_SH, MARKET_SZ,
    KLINE_5MIN, KLINE_15MIN, KLINE_30MIN, KLINE_1HOUR,
    KLINE_DAILY, KLINE_WEEKLY, KLINE_MONTHLY, KLINE_YEARLY, KLINE_3MONTH,
    FQ_NONE, FQ_QFQ, FQ_HFQ,
)
from tdxrs.cli_format import format_output, truncate
from tdxrs.downloader import _DEFAULT_SERVERS as _DOWNLOADER_SERVERS

# ============================================================
# CLI 参数限制
# ============================================================

CLI_LIMITS = {
    "quote_codes":    {"max": 20,   "desc": "行情查询股票数量"},
    "bars_count":     {"default": 10,   "max": 800,  "desc": "K线条数"},
    "trades_count":   {"default": 10,   "max": 500,  "desc": "逐笔成交条数"},
    "stocks_count":   {"default": 10,   "max": 200,  "desc": "股票列表数量"},
    "index_count":    {"default": 10,   "max": 100,  "desc": "指数成分数量"},
    "minutes_count":  {"default": 20,   "max": 240,  "desc": "分时数据条数"},
    "download_rps":   {"default": 15,   "max": 50,   "desc": "下载限速(req/s)"},
    "download_count": {"default": 250,  "max": 1000, "desc": "每股下载条数"},
}

# 默认服务器 (从 downloader 导入，确保一致性)
# CLI 使用 2-element tuples (ip, port)，downloader 使用 3-element tuples (name, ip, port)
_DEFAULT_SERVERS = [(ip, port) for _, ip, port in _DOWNLOADER_SERVERS]

# K线周期映射
_CATEGORY_MAP = {
    "5min": KLINE_5MIN,
    "15min": KLINE_15MIN,
    "30min": KLINE_30MIN,
    "60min": KLINE_1HOUR,
    "day": KLINE_DAILY,
    "week": KLINE_WEEKLY,
    "month": KLINE_MONTHLY,
    "season": KLINE_3MONTH,
    "year": KLINE_YEARLY,
}

# 复权映射
_FQ_MAP = {
    0: FQ_NONE,
    1: FQ_QFQ,
    2: FQ_HFQ,
}


# ============================================================
# 参数校验
# ============================================================

def check_limit(key, value):
    """校验 CLI 参数限制，超限则报错退出"""
    info = CLI_LIMITS.get(key)
    if not info:
        return value
    max_val = info["max"]
    if value > max_val:
        print(
            f"error: {info['desc']} 最大 {max_val}，当前 {value}。"
            f"如需更多数据请使用 tdxrs Python API。",
            file=sys.stderr,
        )
        sys.exit(1)
    return value


def auto_market(code):
    """根据代码自动判断市场"""
    if code.startswith(("6", "5", "9")):
        return MARKET_SH
    return MARKET_SZ


def make_client(timeout=5.0):
    """创建 TdxDirectClient (随机服务器)"""
    ip, port = random.choice(_DEFAULT_SERVERS)
    return TdxDirectClient(ip, port, timeout)


# ============================================================
# 命令实现
# ============================================================

def cmd_quote(args):
    """实时行情"""
    codes = [c.strip() for c in args.code.split(",") if c.strip()]
    if not codes:
        print("error: 请指定至少一个股票代码", file=sys.stderr)
        sys.exit(1)
    check_limit("quote_codes", len(codes))

    client = make_client(args.timeout)
    pairs = [(auto_market(c), c) for c in codes]
    results = client.get_security_quotes(pairs)

    columns = [
        ("代码", "代码", 8),
        ("最新", "最新", 10),
        ("涨跌%", "涨跌%", 8),
        ("开盘", "开盘", 10),
        ("最高", "最高", 10),
        ("最低", "最低", 10),
        ("成交量", "成交量", 12),
        ("成交额", "成交额", 14),
    ]

    rows = []
    for r in results:
        code = r.get("code", "")
        price = r.get("price", 0)
        open_ = r.get("open", 0)
        high = r.get("high", 0)
        low = r.get("low", 0)
        vol = r.get("vol", 0)
        amount = r.get("amount", 0)
        last_close = r.get("last_close", 0)
        change_pct = ((price - last_close) / last_close * 100) if last_close else 0

        rows.append({
            "代码": code,
            "最新": f"{price:.2f}",
            "涨跌%": f"{change_pct:+.2f}",
            "开盘": f"{open_:.2f}",
            "最高": f"{high:.2f}",
            "最低": f"{low:.2f}",
            "成交量": f"{vol:,.0f}",
            "成交额": f"{amount:,.0f}",
        })

    format_output(rows, columns, args.format)


def cmd_bars(args):
    """K线数据"""
    code = args.code
    market = auto_market(code)
    cat = _CATEGORY_MAP.get(args.category)
    if cat is None:
        print(f"error: 不支持的周期 '{args.category}'", file=sys.stderr)
        sys.exit(1)

    count = check_limit("bars_count", args.count)
    fq = _FQ_MAP.get(args.fq, FQ_NONE)

    client = make_client(args.timeout)
    bars = client.get_security_bars(cat, market, code, 0, count, fq)

    columns = [
        ("日期", "日期", 12),
        ("开盘", "开盘", 10),
        ("最高", "最高", 10),
        ("最低", "最低", 10),
        ("收盘", "收盘", 10),
        ("成交量", "成交量", 12),
    ]

    rows = []
    for b in bars:
        date_str = b.get("datetime", b.get("date", ""))
        if isinstance(date_str, str) and len(date_str) > 10:
            date_str = date_str[:10]
        rows.append({
            "日期": date_str,
            "开盘": f"{b.get('open', 0):.2f}",
            "最高": f"{b.get('high', 0):.2f}",
            "最低": f"{b.get('low', 0):.2f}",
            "收盘": f"{b.get('close', 0):.2f}",
            "成交量": f"{b.get('vol', 0):,.0f}",
        })

    format_output(rows, columns, args.format)


def cmd_minutes(args):
    """分时数据"""
    code = args.code
    market = auto_market(code)
    count = check_limit("minutes_count", getattr(args, "count", CLI_LIMITS["minutes_count"]["default"]))

    client = make_client(args.timeout)
    # 使用历史分时 API (支持今日数据，格式更可靠)
    today = int(datetime.now().strftime('%Y%m%d'))
    data = client.get_history_minute_time_data(market, code, today)

    # 获取昨收价 (用于计算涨跌幅)
    # 优先从实时行情获取 (新股/次新股 last_close 为 IPO 发行价)
    # 回退到 K 线数据 (历史日期场景)
    yesterday_close = 0.0
    try:
        quotes = client.get_security_quotes([(market, code)])
        if quotes and quotes[0].get("last_close", 0) > 0:
            yesterday_close = quotes[0]["last_close"]
    except Exception:
        pass
    if yesterday_close <= 0:
        try:
            bars = client.get_security_bars(9, market, code, 0, 2)
            if bars and len(bars) >= 2:
                yesterday_close = bars[-2]["close"]
        except Exception:
            pass

    # 限制返回数量 (数据已在 Rust 层倒序)
    data = data[:count] if data else []

    columns = [
        ("序号", "#", 6),
        ("时间", "时间", 8),
        ("价格", "价格", 10),
        ("涨跌幅%", "涨跌幅%", 8),
        ("均价", "均价", 10),
        ("成交量", "成交量", 12),
    ]

    rows = []
    for i, d in enumerate(data):
        price = d.get("price", 0)
        # 涨跌幅 = (当前价 - 昨收) / 昨收 * 100
        change_pct = ((price - yesterday_close) / yesterday_close * 100) if yesterday_close else 0
        rows.append({
            "序号": str(i + 1),
            "时间": d.get("time", ""),
            "价格": f"{price:.2f}",
            "涨跌幅%": f"{change_pct:+.2f}",
            "均价": f"{d.get('avg_price', 0):.2f}",
            "成交量": f"{d.get('vol', 0):,.0f}",
        })

    format_output(rows, columns, args.format)


def cmd_trades(args):
    """逐笔成交"""
    code = args.code
    market = auto_market(code)
    count = check_limit("trades_count", args.count)

    client = make_client(args.timeout)
    data = client.get_transaction_data(market, code, 0, count)

    columns = [
        ("时间", "时间", 10),
        ("价格", "价格", 10),
        ("成交量", "成交量", 10),
        ("笔数", "笔数", 8),
        ("买/卖", "买/卖", 6),
    ]

    rows = []
    for d in data:
        _BUYSELL = {0: "买", 1: "卖", 2: "中"}
        buy_sell = _BUYSELL.get(d.get("buyorsell", 0), "?")
        rows.append({
            "时间": d.get("time", ""),
            "价格": f"{d.get('price', 0):.2f}",
            "成交量": f"{d.get('vol', 0):,.0f}",
            "笔数": f"{d.get('num', 0):,}",
            "买/卖": buy_sell,
        })

    format_output(rows, columns, args.format)


def cmd_stocks(args):
    """股票列表"""
    market = MARKET_SH if args.market == "sh" else MARKET_SZ
    count = check_limit("stocks_count", args.count)

    client = make_client(args.timeout)
    total = client.get_security_count(market)
    data = client.get_security_list(market, args.offset)

    # 只取前 count 条
    data = data[:count] if data else []

    columns = [
        ("代码", "代码", 8),
        ("名称", "名称", 12),
        ("市场", "市场", 4),
    ]

    rows = []
    for d in data:
        rows.append({
            "代码": d.get("code", ""),
            "名称": truncate(d.get("name", ""), 12),
            "市场": args.market.upper(),
        })

    print(f"市场: {args.market.upper()}  总数: {total}")
    format_output(rows, columns, args.format)


def cmd_index(args):
    """指数成分"""
    code = args.code
    count = check_limit("index_count", args.count)

    # 指数代码 → 板块名称映射 (block_zs.dat 中的名称)
    # block_zs.dat 按板块名称组织，不按指数代码索引
    INDEX_NAME_MAP = {
        "000300": "沪深300",
        "000016": "上证50",
        "000905": "中证500",
        "000852": "中证1000",
        "399001": "深证成指",
        "399006": "创业板指",
        "000001": "上证指数",
        "399005": "中小100",
        "000688": "科创50",
        "399673": "创业板50",
    }

    block_name = INDEX_NAME_MAP.get(code)
    if not block_name:
        print(f"error: 未知指数代码 '{code}'。支持的指数: {', '.join(sorted(INDEX_NAME_MAP.keys()))}",
              file=sys.stderr)
        sys.exit(1)

    client = make_client(args.timeout)
    data = client.get_and_parse_block_info("block_zs.dat")

    if not data:
        print("error: 无法获取板块数据", file=sys.stderr)
        sys.exit(1)

    # 按板块名称分组
    from collections import defaultdict
    blocks = defaultdict(list)
    for d in data:
        blocks[d.get("blockname", "")].append(d.get("code", ""))

    # 查找成分股
    codes = blocks.get(block_name)
    if not codes:
        # 尝试模糊匹配
        for name, clist in blocks.items():
            if block_name in name:
                codes = clist
                block_name = name
                break

    if not codes:
        print(f"error: 未找到指数 {code} ({block_name}) 的成分数据", file=sys.stderr)
        sys.exit(1)

    codes = codes[:count]

    columns = [
        ("序号", "#", 6),
        ("代码", "代码", 8),
    ]

    rows = []
    for i, c in enumerate(codes):
        rows.append({
            "序号": str(i + 1),
            "代码": c,
        })

    print(f"指数: {code} ({block_name})  成分数: {len(rows)}")
    format_output(rows, columns, args.format)


def cmd_xdxr(args):
    """除权除息信息"""
    code = args.code
    market = auto_market(code)
    count = check_limit("index_count", getattr(args, "count", CLI_LIMITS["index_count"]["default"]))

    client = make_client(args.timeout)
    data = client.get_xdxr_info(market, code)

    if not data:
        print(f"error: 未找到 {code} 的除权除息数据", file=sys.stderr)
        sys.exit(1)

    # 限制返回数量 (最新的在前)
    data = data[:count] if data else []

    columns = [
        ("日期", "日期", 12),
        ("类型", "类型", 8),
        ("分红(元)", "分红(元)", 10),
        ("送股", "送股", 6),
        ("配股", "配股", 6),
        ("配股价", "配股价", 8),
        ("缩股", "缩股", 6),
    ]

    # 类型映射
    CATEGORY_MAP = {1: "分红", 2: "送股", 3: "配股", 4: "缩股"}

    rows = []
    for d in data:
        year = d.get("year", 0)
        month = d.get("month", 0)
        day = d.get("day", 0)
        cat = d.get("category", 0)
        fh = d.get("fenhong") or 0
        sg = d.get("songzhuangu") or 0
        pg = d.get("peigu") or 0
        pgj = d.get("peigujia") or 0
        sj = d.get("suogu") or 0

        rows.append({
            "日期": f"{year}-{month:02d}-{day:02d}" if year else "-",
            "类型": CATEGORY_MAP.get(cat, str(cat)),
            "分红(元)": f"{fh/100:.4f}" if fh else "-",
            "送股": f"{sg:.2f}" if sg else "-",
            "配股": f"{pg:.2f}" if pg else "-",
            "配股价": f"{pgj:.2f}" if pgj else "-",
            "缩股": f"{sj:.2f}" if sj else "-",
        })

    print(f"股票: {code}  除权除息记录: {len(data)} 条")
    format_output(rows, columns, args.format)


def cmd_download(args):
    """下载指定股票数据"""
    from tdxrs.downloader import Downloader

    # 解析股票代码
    codes = [c.strip() for c in args.code.split(",") if c.strip()]
    if not codes:
        print("error: 请指定至少一个股票代码", file=sys.stderr)
        sys.exit(1)
    check_limit("quote_codes", len(codes))

    rps = check_limit("download_rps", args.rate_limit)

    dl = Downloader(
        data_dir=args.output,
        servers=args.servers.split(",") if args.servers else None,
        rate_limit=rps,
        format=args.format,
        fq=args.fq,
    )

    # CLI 周期 → 下载器周期映射
    CATEGORY_MAP = {
        "day": "daily", "week": "weekly", "month": "monthly",
        "5min": "min5", "15min": "min15", "30min": "min30", "60min": "min60",
    }
    category = CATEGORY_MAP.get(args.category, args.category)

    print(f"开始下载: codes={codes} category={args.category} format={args.format}")
    if args.start:
        print(f"起始日期: {args.start}")
    if args.end:
        print(f"结束日期: {args.end}")
    print(f"保存位置: {dl.data_dir}")
    dl.run(categories=[category], codes=codes,
           start_date=args.start, end_date=args.end)
    print(f"下载完成: {dl.progress()}")


def cmd_update(args):
    """增量更新"""
    from tdxrs.downloader import Downloader

    rps = check_limit("download_rps", args.rate_limit)

    dl = Downloader(
        data_dir=args.output,
        servers=args.servers.split(",") if args.servers else None,
        rate_limit=rps,
        format=args.format,
    )

    markets = None if args.market == "all" else [args.market]

    # CLI 周期 → 下载器周期映射
    CATEGORY_MAP = {
        "day": "daily", "week": "weekly", "month": "monthly",
        "5min": "min5", "15min": "min15", "30min": "min30", "60min": "min60",
    }
    category = CATEGORY_MAP.get(args.category, args.category)
    categories = [category]

    # 解析股票代码
    codes = None
    if args.code:
        codes = [c.strip() for c in args.code.split(",") if c.strip()]
        if codes:
            check_limit("quote_codes", len(codes))

    print(f"增量更新: market={args.market} category={args.category} format={args.format}")
    if codes:
        print(f"股票代码: {codes}")
    if args.start:
        print(f"起始日期: {args.start}")
    if args.end:
        print(f"结束日期: {args.end}")
    print(f"保存位置: {dl.data_dir}")
    dl.update(markets=markets, categories=categories, codes=codes,
              start_date=args.start, end_date=args.end)
    print(f"更新完成: {dl.progress()}")


def cmd_download_xdxr(args):
    """下载除权除息数据"""
    from tdxrs.downloader import Downloader

    # 解析股票代码
    codes = [c.strip() for c in args.code.split(",") if c.strip()]
    if not codes:
        print("error: 请指定至少一个股票代码", file=sys.stderr)
        sys.exit(1)
    check_limit("quote_codes", len(codes))

    rps = check_limit("download_rps", args.rate_limit)

    dl = Downloader(
        data_dir=args.output,
        servers=args.servers.split(",") if args.servers else None,
        rate_limit=rps,
    )

    print(f"下载除权除息数据: codes={codes}")
    print(f"保存位置: {dl.data_dir}")
    dl.run_xdxr(codes=codes)
    print(f"下载完成: {dl.progress()}")


def cmd_parse(args):
    """本地文件解析"""
    from pathlib import Path

    filepath = Path(args.file)
    if not filepath.exists():
        print(f"error: 文件不存在: {filepath}", file=sys.stderr)
        sys.exit(1)

    ftype = args.type
    if ftype == "auto":
        suffix = filepath.suffix.lower()
        name = filepath.name.lower()
        if suffix == ".day" or "daily" in name:
            ftype = "daily"
        elif suffix in (".5", ".15", ".30", ".60") or "min" in name:
            ftype = "min"
        elif "block" in name:
            ftype = "block"
        elif "finance" in name or "gpcw" in name:
            ftype = "finance"
        else:
            ftype = "daily"  # 默认

    # 解析
    if ftype == "daily":
        reader = DailyBarReader()
        data = reader.parse_file_tuples(str(filepath))
        columns = [
            ("日期", "日期", 12),
            ("开盘", "开盘", 10),
            ("最高", "最高", 10),
            ("最低", "最低", 10),
            ("收盘", "收盘", 10),
            ("成交量", "成交量", 12),
            ("成交额", "成交额", 14),
        ]
        rows = []
        for d in data:
            rows.append({
                "日期": d[0],
                "开盘": f"{d[1]:.2f}",
                "最高": f"{d[2]:.2f}",
                "最低": f"{d[3]:.2f}",
                "收盘": f"{d[4]:.2f}",
                "成交量": f"{d[6]:,.0f}",
                "成交额": f"{d[5]:,.0f}",
            })

    elif ftype == "min":
        reader = MinBarReader()
        data = reader.parse_file_tuples(str(filepath))
        # 元组布局: (日期时间, 开, 高, 低, 收, 成交额, 成交量, 年, 月, 日, 时, 分)
        # 与 py_reader.rs 的 MinBar parse_file_tuples 构造一致（时间已并入 d[0]）
        columns = [
            ("日期时间", "日期时间", 17),
            ("开盘", "开盘", 10),
            ("最高", "最高", 10),
            ("最低", "最低", 10),
            ("收盘", "收盘", 10),
            ("成交量", "成交量", 12),
            ("成交额", "成交额", 14),
        ]
        rows = []
        for d in data:
            rows.append({
                "日期时间": d[0],
                "开盘": f"{d[1]:.2f}",
                "最高": f"{d[2]:.2f}",
                "最低": f"{d[3]:.2f}",
                "收盘": f"{d[4]:.2f}",
                "成交量": f"{d[6]:,.0f}",
                "成交额": f"{d[5]:,.0f}",
            })

    elif ftype == "block":
        reader = BlockReader()
        data = reader.parse_data_group(filepath.read_bytes(), str(filepath))
        columns = [
            ("代码", "代码", 8),
            ("名称", "名称", 12),
        ]
        rows = []
        if isinstance(data, dict):
            for group_name, stocks in data.items():
                for s in (stocks or []):
                    rows.append({
                        "代码": s.get("code", ""),
                        "名称": truncate(s.get("name", ""), 12),
                    })
        else:
            for s in (data or []):
                rows.append({
                    "代码": s.get("code", ""),
                    "名称": truncate(s.get("name", ""), 12),
                })
    else:
        print(f"error: 不支持的文件类型 '{ftype}'", file=sys.stderr)
        sys.exit(1)

    # 截断
    if args.count and args.count != "all":
        try:
            n = int(args.count)
            rows = rows[:n]
        except ValueError:
            pass

    print(f"文件: {filepath.name}  类型: {ftype}  记录数: {len(rows)}")
    format_output(rows, columns, args.format)


def cmd_servers(args):
    """测试服务器连通性"""
    import time

    print("测试服务器连通性...\n")

    ok_count = 0
    fail_count = 0
    latencies = []

    for ip, port in _DEFAULT_SERVERS:
        try:
            start = time.time()
            client = TdxDirectClient(ip, port, args.timeout)
            client.get_security_count(MARKET_SH)
            elapsed = (time.time() - start) * 1000
            latencies.append(elapsed)
            ok_count += 1
        except Exception:
            fail_count += 1

    total = ok_count + fail_count
    print(f"可用服务器: {ok_count}/{total}")

    if latencies:
        avg_latency = sum(latencies) / len(latencies)
        min_latency = min(latencies)
        max_latency = max(latencies)
        print(f"平均延迟: {avg_latency:.0f}ms")
        print(f"延迟范围: {min_latency:.0f}ms ~ {max_latency:.0f}ms")


def cmd_version(args):
    """版本信息"""
    from tdxrs import __version__
    print(f"tdxrs {__version__}")
    print(f"Python {sys.version.split()[0]}")
    print(f"平台 {sys.platform}")


# ============================================================
# 主入口
# ============================================================

def main():
    parser = argparse.ArgumentParser(
        prog="tdxrs",
        description="tdxrs — 通达信行情数据 CLI 工具",
    )
    sub = parser.add_subparsers(dest="command", help="可用命令")

    # ── quote ──
    p = sub.add_parser("quote", help="实时行情")
    p.add_argument("code", help="股票代码，多只用逗号分隔 (最多20)")

    p.add_argument("--timeout", type=float, default=5.0, help="超时秒数 (默认5)")
    p.add_argument("--format", choices=["table", "json", "csv"], default="table")
    p.set_defaults(func=cmd_quote)

    # ── bars ──
    p = sub.add_parser("bars", help="K线数据")
    p.add_argument("code", help="股票代码")
    p.add_argument("--category", default="day",
                    choices=list(_CATEGORY_MAP.keys()), help="周期 (默认day)")
    p.add_argument("--count", type=int, default=CLI_LIMITS["bars_count"]["default"],
                    help=f"条数 (默认{CLI_LIMITS['bars_count']['default']}，上限{CLI_LIMITS['bars_count']['max']})")
    p.add_argument("--fq", type=int, default=0, choices=[0, 1, 2],
                    help="复权: 0=不复权 1=前复权 2=后复权")

    p.add_argument("--timeout", type=float, default=5.0)
    p.add_argument("--format", choices=["table", "json", "csv"], default="table")
    p.set_defaults(func=cmd_bars)

    # ── minutes ──
    p = sub.add_parser("minutes", help="分时数据")
    p.add_argument("code", help="股票代码")
    p.add_argument("--count", type=int, default=CLI_LIMITS["minutes_count"]["default"],
                    help=f"条数 (默认{CLI_LIMITS['minutes_count']['default']}，上限{CLI_LIMITS['minutes_count']['max']})")

    p.add_argument("--timeout", type=float, default=5.0)
    p.add_argument("--format", choices=["table", "json", "csv"], default="table")
    p.set_defaults(func=cmd_minutes)

    # ── trades ──
    p = sub.add_parser("trades", help="逐笔成交")
    p.add_argument("code", help="股票代码")
    p.add_argument("--count", type=int, default=CLI_LIMITS["trades_count"]["default"],
                    help=f"条数 (默认{CLI_LIMITS['trades_count']['default']}，上限{CLI_LIMITS['trades_count']['max']})")

    p.add_argument("--timeout", type=float, default=5.0)
    p.add_argument("--format", choices=["table", "json", "csv"], default="table")
    p.set_defaults(func=cmd_trades)

    # ── stocks ──
    p = sub.add_parser("stocks", help="股票列表")
    p.add_argument("--market", default="sh", choices=["sh", "sz"], help="市场 (默认sh)")
    p.add_argument("--offset", type=int, default=0, help="起始偏移")
    p.add_argument("--count", type=int, default=CLI_LIMITS["stocks_count"]["default"],
                    help=f"数量 (默认{CLI_LIMITS['stocks_count']['default']}，上限{CLI_LIMITS['stocks_count']['max']})")

    p.add_argument("--timeout", type=float, default=5.0)
    p.add_argument("--format", choices=["table", "json"], default="table")
    p.set_defaults(func=cmd_stocks)

    # ── index ──
    p = sub.add_parser("index", help="指数成分")
    p.add_argument("code", help="指数代码 (如 000300)")
    p.add_argument("--offset", type=int, default=0)
    p.add_argument("--count", type=int, default=CLI_LIMITS["index_count"]["default"],
                    help=f"数量 (默认{CLI_LIMITS['index_count']['default']}，上限{CLI_LIMITS['index_count']['max']})")

    p.add_argument("--timeout", type=float, default=5.0)
    p.add_argument("--format", choices=["table", "json"], default="table")
    p.set_defaults(func=cmd_index)

    # ── xdxr ──
    p = sub.add_parser("xdxr", help="除权除息信息")
    p.add_argument("code", help="股票代码")
    p.add_argument("--count", type=int, default=CLI_LIMITS["index_count"]["default"],
                    help=f"条数 (默认{CLI_LIMITS['index_count']['default']}，上限{CLI_LIMITS['index_count']['max']})")

    p.add_argument("--timeout", type=float, default=5.0)
    p.add_argument("--format", choices=["table", "json", "csv"], default="table")
    p.set_defaults(func=cmd_xdxr)

    # ── download ──
    p = sub.add_parser("download", help="下载指定股票数据")
    p.add_argument("code", help="股票代码，多只用逗号分隔 (最多20)")
    p.add_argument("--category", default="day", choices=list(_CATEGORY_MAP.keys()))
    p.add_argument("--format", default="tdx", choices=["tdx", "csv", "parquet"])
    p.add_argument("--output", default="./data", help="输出目录 (默认 ./data)")
    p.add_argument("--fq", type=int, default=0, choices=[0, 1, 2],
                    help="复权: 0=原始(支持增量更新) 1=前复权(全量覆盖) 2=后复权(全量覆盖)")
    p.add_argument("--start", help="起始日期 YYYY-MM-DD")
    p.add_argument("--end", help="结束日期 YYYY-MM-DD")
    p.add_argument("--servers", help="服务器列表，逗号分隔")
    p.add_argument("--rate-limit", type=int, default=CLI_LIMITS["download_rps"]["default"],
                    help=f"限速 req/s (默认{CLI_LIMITS['download_rps']['default']}，上限{CLI_LIMITS['download_rps']['max']})")
    p.set_defaults(func=cmd_download)

    # ── update ──
    p = sub.add_parser("update", help="增量更新")
    p.add_argument("--code", help="股票代码，多只用逗号分隔 (默认更新已下载的股票)")
    p.add_argument("--market", default="all", choices=["sh", "sz", "all"])
    p.add_argument("--category", default="day", choices=list(_CATEGORY_MAP.keys()))
    p.add_argument("--format", default="tdx", choices=["tdx", "csv", "parquet"])
    p.add_argument("--output", default="./data")
    p.add_argument("--start", help="起始日期 YYYY-MM-DD")
    p.add_argument("--end", help="结束日期 YYYY-MM-DD")
    p.add_argument("--servers", help="服务器列表，逗号分隔")
    p.add_argument("--rate-limit", type=int, default=CLI_LIMITS["download_rps"]["default"],
                    help=f"限速 req/s (默认{CLI_LIMITS['download_rps']['default']}，上限{CLI_LIMITS['download_rps']['max']})")
    p.set_defaults(func=cmd_update)

    # ── download-xdxr ──
    p = sub.add_parser("download-xdxr", help="下载除权除息数据")
    p.add_argument("code", help="股票代码，多只用逗号分隔 (最多20)")
    p.add_argument("--output", default="./data", help="输出目录 (默认 ./data)")
    p.add_argument("--servers", help="服务器列表，逗号分隔")
    p.add_argument("--rate-limit", type=int, default=CLI_LIMITS["download_rps"]["default"],
                    help=f"限速 req/s (默认{CLI_LIMITS['download_rps']['default']}，上限{CLI_LIMITS['download_rps']['max']})")
    p.set_defaults(func=cmd_download_xdxr)

    # ── parse ──
    p = sub.add_parser("parse", help="本地文件解析")
    p.add_argument("file", help="文件路径")
    p.add_argument("--type", default="auto", choices=["auto", "daily", "min", "block"],
                    help="文件类型 (默认自动检测)")
    p.add_argument("--count", default="all", help="显示条数 (默认全部)")
    p.add_argument("--format", choices=["table", "json", "csv"], default="table")
    p.set_defaults(func=cmd_parse)

    # ── servers ──
    p = sub.add_parser("servers", help="测试服务器连通性")
    p.add_argument("--timeout", type=float, default=3.0, help="超时秒数 (默认3)")
    p.set_defaults(func=cmd_servers)

    # ── version ──
    p = sub.add_parser("version", help="版本信息")
    p.set_defaults(func=cmd_version)

    # 解析
    args = parser.parse_args()
    if not args.command:
        parser.print_help()
        sys.exit(0)

    args.func(args)


if __name__ == "__main__":
    main()
