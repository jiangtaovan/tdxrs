"""tdxrs CLI — 命令行快速查询工具

用法:
    tdxrs quote 600519
    tdxrs bars 600519 --count 30
    tdxrs v2 quote 600519            # 新一代 7709 协议（2026-07 起旧协议行情断供）
    tdxrs v2 bars 600519 --count 30
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

_STARVED_HINT = (
    "提示: 2026-07 起服务端对旧协议行情功能族（快照/K线/分时/逐笔）静默断供\n"
    "（目录/财务/除权类仍存活），市场数据走新协议（本 CLI 默认自动 v2）。"
)


def cmd_quote(args):
    """实时行情"""
    from tdxrs.v2_client import TdxV2Error

    codes = [c.strip() for c in args.code.split(",") if c.strip()]
    if not codes:
        print("error: 请指定至少一个股票代码", file=sys.stderr)
        sys.exit(1)
    check_limit("quote_codes", len(codes))

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
    try:
        # v2: 单连接逐只取（快照帧一帧一票）
        from tdxrs.v2_client import TdxV2Client

        with TdxV2Client(_V2_DEFAULT_HOST, timeout=args.timeout) as client:
            for code in codes:
                r = client.get_quote(auto_market(code), code)
                change_pct = (
                    (r["price"] - r["last_close"]) / r["last_close"] * 100
                    if r["last_close"] else 0
                )
                rows.append({
                    "代码": code,
                    "最新": f"{r['price']:.2f}",
                    "涨跌%": f"{change_pct:+.2f}",
                    "开盘": f"{r['open']:.2f}",
                    "最高": f"{r['high']:.2f}",
                    "最低": f"{r['low']:.2f}",
                    "成交量": f"{r['vol']:,.0f}",
                    "成交额": f"{r['amount']:,.0f}",
                })
    except (TdxV2Error, OSError) as e:
        print(f"warn: v2 行情不可用({e})，尝试旧协议…", file=sys.stderr)
        client = make_client(args.timeout)
        pairs = [(auto_market(c), c) for c in codes]
        results = client.get_security_quotes(pairs)
        for r in results:
            price = r.get("price", 0)
            last_close = r.get("last_close", 0)
            change_pct = ((price - last_close) / last_close * 100) if last_close else 0
            rows.append({
                "代码": r.get("code", ""),
                "最新": f"{price:.2f}",
                "涨跌%": f"{change_pct:+.2f}",
                "开盘": f"{r.get('open', 0):.2f}",
                "最高": f"{r.get('high', 0):.2f}",
                "最低": f"{r.get('low', 0):.2f}",
                "成交量": f"{r.get('vol', 0):,.0f}",
                "成交额": f"{r.get('amount', 0):,.0f}",
            })
        if not rows:
            print("error: 旧协议快照为空（服务端断供）且 v2 不可用。" + _STARVED_HINT,
                  file=sys.stderr)
            sys.exit(1)

    format_output(rows, columns, args.format)


def _fetch_xdxr_old(market, code, timeout):
    """旧协议取除权记录（目录类接口 2026-09 实测仍存活）。"""
    try:
        return make_client(timeout).get_xdxr_info(market, code) or []
    except Exception:
        return []


def _fetch_bars_v2_with_context(market, code, count, category, timeout, xdxr_list):
    """v2 取 K 线；若最早除权事件早于窗口，按自动分档补拉历史上下文页。

    返回 (bars, context_bars)，均按日期升序。
    """
    from tdxrs import adjust as fq_adjust
    from tdxrs.v2_client import MAX_BARS_PER_REQUEST, TdxV2Client

    with TdxV2Client(_V2_DEFAULT_HOST, timeout=timeout) as client:
        bars = client.get_bars(market, code, count=min(count, MAX_BARS_PER_REQUEST),
                               category=category)
        context = []
        events = fq_adjust.collect_events(xdxr_list)
        if bars and events and bars[0].date > events[0][0]:
            tier = fq_adjust.auto_tier(xdxr_list, datetime.now().year)
            page_cap = -(-fq_adjust.TIER_BARS[tier] // MAX_BARS_PER_REQUEST)
            start = len(bars)
            for _ in range(page_cap):
                page = client.get_bars(market, code, count=MAX_BARS_PER_REQUEST,
                                       category=category, start=start)
                if not page:
                    break
                context = page + context  # 越翻越旧，前置保持升序
                if page[0].date <= events[0][0]:
                    break
                start += len(page)
    return bars, context


def cmd_bars(args):
    """K线数据（v2 协议支持全部周期；复权 = v2 原始K线 + 旧协议除权 + 本地调整）"""
    from tdxrs import adjust as fq_adjust
    from tdxrs.v2_client import TdxV2Client, TdxV2Error

    code = args.code
    market = auto_market(code)
    cat = _CATEGORY_MAP.get(args.category)
    if cat is None:
        print(f"error: 不支持的周期 '{args.category}'", file=sys.stderr)
        sys.exit(1)

    count = check_limit("bars_count", args.count)
    fq = _FQ_MAP.get(args.fq, FQ_NONE)
    intraday = cat in (0, 1, 2, 3)  # 分钟/小时级：日期列附结束时刻

    if fq == FQ_NONE:
        try:
            with TdxV2Client(_V2_DEFAULT_HOST, timeout=args.timeout) as client:
                bars = client.get_bars(market, code, count=min(count, 700), category=cat)
        except (TdxV2Error, OSError) as e:
            print(f"warn: v2 K线不可用({e})，尝试旧协议…", file=sys.stderr)
            client = make_client(args.timeout)
            bars = client.get_security_bars(cat, market, code, 0, count, fq)
            if not bars:
                print("error: 旧协议K线为空（服务端断供）且 v2 不可用。" + _STARVED_HINT,
                      file=sys.stderr)
                sys.exit(1)
    else:
        xdxr_list = _fetch_xdxr_old(market, code, args.timeout)
        if not xdxr_list:
            print(
                "error: 复权失败——旧协议除权接口无数据且新协议除权接口尚未逆向。\n"
                "替代方案: `tdxrs bars --fq 0` 取原始K线。",
                file=sys.stderr,
            )
            sys.exit(1)
        try:
            bars, context = _fetch_bars_v2_with_context(
                market, code, count, cat, args.timeout, xdxr_list)
        except (TdxV2Error, OSError) as e:
            print(f"error: 复权K线取数失败（v2 不可用: {e}）", file=sys.stderr)
            sys.exit(1)
        if not bars:
            print("error: 未取到K线数据。", file=sys.stderr)
            sys.exit(1)
        bars = fq_adjust.adjust_bars(bars, context, xdxr_list, fq)

    columns = [
        ("日期", "日期", 17 if intraday else 12),
        ("开盘", "开盘", 10),
        ("最高", "最高", 10),
        ("最低", "最低", 10),
        ("收盘", "收盘", 10),
        ("成交量", "成交量", 12),
    ]

    rows = []
    for b in bars:
        if isinstance(b, dict):  # 旧协议路径
            date_str = b.get("datetime", b.get("date", ""))
            if isinstance(date_str, str) and len(date_str) > 10 and not intraday:
                date_str = date_str[:10]
            o, h, l, c, vol = (b.get("open", 0), b.get("high", 0), b.get("low", 0),
                               b.get("close", 0), b.get("vol", 0))
        else:  # v2 Bar
            s = str(b.date)
            date_str = f"{s[:4]}-{s[4:6]}-{s[6:8]}"
            if intraday and b.time_sec:
                date_str += f" {b.time_sec // 3600:02d}:{b.time_sec % 3600 // 60:02d}"
            o, h, l, c, vol = b.open, b.high, b.low, b.close, b.vol
        rows.append({
            "日期": date_str,
            "开盘": f"{o:.2f}",
            "最高": f"{h:.2f}",
            "最低": f"{l:.2f}",
            "收盘": f"{c:.2f}",
            "成交量": f"{vol:,.0f}",
        })

    format_output(rows, columns, args.format)


def cmd_minutes(args):
    """分时数据（v2 协议当日分时，含均价）"""
    from tdxrs.v2_client import TdxV2Client, TdxV2Error

    code = args.code
    market = auto_market(code)
    count = check_limit("minutes_count", getattr(args, "count", CLI_LIMITS["minutes_count"]["default"]))

    data = None
    yesterday_close = 0.0
    try:
        with TdxV2Client(_V2_DEFAULT_HOST, timeout=args.timeout) as client:
            data = client.get_minute_time_data(market, code)
            try:
                yesterday_close = client.get_quote(market, code)["last_close"]
            except TdxV2Error:
                pass
    except (TdxV2Error, OSError) as e:
        print(f"warn: v2 分时不可用({e})，尝试旧协议…", file=sys.stderr)

    if not data:
        # 旧协议回退（历史分时 API，多半已断供）
        client = make_client(args.timeout)
        today = int(datetime.now().strftime('%Y%m%d'))
        raw = client.get_history_minute_time_data(market, code, today)
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
        if not raw:
            print("error: 旧协议分时为空（服务端断供）且 v2 不可用。" + _STARVED_HINT,
                  file=sys.stderr)
            sys.exit(1)
        data = [{"time": d.get("time", ""), "price": d.get("price", 0),
                 "avg_price": d.get("avg_price", 0), "vol": d.get("vol", 0)} for d in raw]

    # 限制返回数量（v2 升序取最早 N 条；与旧协议倒序语义一致地取"最新"）
    if len(data) > count:
        data = data[-count:]

    columns = [
        ("时间", "时间", 8),
        ("价格", "价格", 10),
        ("涨跌幅%", "涨跌幅%", 8),
        ("均价", "均价", 10),
        ("成交量", "成交量", 12),
    ]

    rows = []
    for d in data:
        if isinstance(d, dict):  # 旧协议
            time_str, price = d.get("time", ""), d.get("price", 0)
            avg, vol = d.get("avg_price", 0), d.get("vol", 0)
        else:  # v2 MinutePoint
            time_str, price = d.time, d.price
            avg, vol = d.avg_price, d.vol
        change_pct = ((price - yesterday_close) / yesterday_close * 100) if yesterday_close else 0
        rows.append({
            "时间": time_str,
            "价格": f"{price:.2f}",
            "涨跌幅%": f"{change_pct:+.2f}",
            "均价": f"{avg:.2f}",
            "成交量": f"{vol:,.0f}",
        })

    format_output(rows, columns, args.format)


def cmd_trades(args):
    """逐笔成交"""
    code = args.code
    market = auto_market(code)
    count = check_limit("trades_count", args.count)

    client = make_client(args.timeout)
    data = client.get_transaction_data(market, code, 0, count)

    if not data:
        print("error: 逐笔成交为空——服务端已断供旧协议逐笔功能族，"
              "新协议逐笔帧尚未逆向。\n替代方案: `tdxrs minutes` 看分钟级量价。",
              file=sys.stderr)
        sys.exit(1)

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

    if not data:
        print("error: 股票列表为空（目录类 API 异常或服务端变更）。", file=sys.stderr)
        sys.exit(1)

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
        print("error: 无法获取板块数据（目录类 API 异常或服务端变更）。", file=sys.stderr)
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
        print(
            "error: 未找到除权除息数据——服务端已断供旧协议除权功能族，"
            "新协议除权接口尚未逆向（2026-09 线缆级逆向结论）。",
            file=sys.stderr,
        )
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
        source=getattr(args, "source", "v2"),
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
        source=getattr(args, "source", "v2"),
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

    # 按代码前缀推断市场，避免在错误市场上无效查询（服务端会直接断连）
    markets = ["sh" if auto_market(c) == MARKET_SH else "sz" for c in codes]

    dl = Downloader(
        data_dir=args.output,
        servers=args.servers.split(",") if args.servers else None,
        rate_limit=rps,
    )

    print(f"下载除权除息数据: codes={codes}")
    print(f"保存位置: {dl.data_dir}")
    dl.run_xdxr(markets=sorted(set(markets)), codes=codes)
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
                "成交额": f"{d[5]:,.0f}",
                "成交量": f"{d[6]:,.0f}",
            })

    elif ftype == "min":
        reader = MinBarReader()
        data = reader.parse_file_tuples(str(filepath))
        columns = [
            ("日期", "日期", 12),
            ("时间", "时间", 8),
            ("开盘", "开盘", 10),
            ("最高", "最高", 10),
            ("最低", "最低", 10),
            ("收盘", "收盘", 10),
            ("成交量", "成交量", 12),
        ]
        rows = []
        for d in data:
            rows.append({
                "日期": d[0],
                "时间": str(d[1]),
                "开盘": f"{d[2]:.2f}",
                "最高": f"{d[3]:.2f}",
                "最低": f"{d[4]:.2f}",
                "收盘": f"{d[5]:.2f}",
                "成交量": f"{d[6]:,.0f}",
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
    """测试服务器连通性（目录类 + v2 行情数据面）"""
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
    print(f"目录类可用服务器 (旧协议): {ok_count}/{total}")

    if latencies:
        avg_latency = sum(latencies) / len(latencies)
        min_latency = min(latencies)
        max_latency = max(latencies)
        print(f"平均延迟: {avg_latency:.0f}ms")
        print(f"延迟范围: {min_latency:.0f}ms ~ {max_latency:.0f}ms")

    # v2 行情数据面探测：目录类"活着"不代表行情没被断供，
    # 用一次真实 K 线请求验证新协议数据面。
    from tdxrs.v2_client import TdxV2Client

    try:
        start = time.time()
        with TdxV2Client(_V2_DEFAULT_HOST, timeout=max(args.timeout, 6.0)) as client:
            bars = client.get_bars(MARKET_SH, "600519", count=1)
        elapsed = (time.time() - start) * 1000
        if bars:
            print(f"\n行情数据面 (v2 @{_V2_DEFAULT_HOST}): 正常 "
                  f"({elapsed:.0f}ms, 最新日K {bars[-1].date})")
        else:
            print(f"\n行情数据面 (v2 @{_V2_DEFAULT_HOST}): 异常（空响应）")
    except Exception as e:
        print(f"\n行情数据面 (v2 @{_V2_DEFAULT_HOST}): 不可用 → {e}")


def cmd_version(args):
    """版本信息"""
    from tdxrs import __version__
    print(f"tdxrs {__version__}")
    print(f"Python {sys.version.split()[0]}")
    print(f"平台 {sys.platform}")


# ============================================================
# 新一代 7709 协议（v2）
#
# 2026-07 起服务端对旧帧（0c 02 系）按功能族断供行情数据（快照/K线/
# 分时/除权静默返回空，目录/财务类仍应答），官方客户端改用新帧格式。
# 本命令组走 tdxrs.v2_client（逆向实现，详见该模块 docstring 与 research/）。
# ============================================================

_V2_DEFAULT_HOST = "121.36.248.138"


def _add_v2_args(p):
    p.add_argument("--host", default=_V2_DEFAULT_HOST, help=f"行情服务器 (默认{_V2_DEFAULT_HOST})")
    p.add_argument("--port", type=int, default=7709, help="端口 (默认7709)")
    p.add_argument("--timeout", type=float, default=6.0, help="超时秒数 (默认6)")
    p.add_argument("--format", choices=["table", "json", "csv"], default="table")


def cmd_v2_quote(args):
    """实时行情（新协议）"""
    from tdxrs.v2_client import TdxV2Client

    codes = [c.strip() for c in args.code.split(",") if c.strip()]
    if not codes:
        print("error: 请指定至少一个股票代码", file=sys.stderr)
        sys.exit(1)
    check_limit("quote_codes", len(codes))

    columns = [
        ("代码", "代码", 8), ("昨收", "昨收", 10), ("开盘", "开盘", 10),
        ("最高", "最高", 10), ("最低", "最低", 10), ("最新", "最新", 10),
        ("涨跌%", "涨跌%", 8), ("成交量(股)", "成交量(股)", 14), ("成交额", "成交额", 14),
    ]
    rows = []
    with TdxV2Client(args.host, args.port, timeout=args.timeout) as client:
        for code in codes:
            row = client.get_quote(auto_market(code), code)
            change = (
                (row["price"] - row["last_close"]) / row["last_close"] * 100
                if row["last_close"] else 0.0
            )
            rows.append({
                "代码": code,
                "昨收": f"{row['last_close']:.2f}",
                "开盘": f"{row['open']:.2f}",
                "最高": f"{row['high']:.2f}",
                "最低": f"{row['low']:.2f}",
                "最新": f"{row['price']:.2f}",
                "涨跌%": f"{change:+.2f}",
                "成交量(股)": f"{row['vol']:,.0f}",
                "成交额": f"{row['amount']:,.0f}",
            })
    format_output(rows, columns, args.format)


def cmd_v2_bars(args):
    """K线数据（新协议；全周期可用，复权需除权接口——尚未逆向）"""
    from tdxrs.v2_client import TdxV2Client

    cat = _CATEGORY_MAP.get(args.category)
    if cat is None:
        print(f"error: 不支持的周期 '{args.category}'", file=sys.stderr)
        sys.exit(1)
    count = check_limit("bars_count", args.count)
    intraday = args.category in ("5min", "15min", "30min", "60min")
    columns = [
        ("日期", "日期", 17 if intraday else 12), ("开盘", "开盘", 10), ("最高", "最高", 10),
        ("最低", "最低", 10), ("收盘", "收盘", 10), ("成交量(股)", "成交量(股)", 14),
        ("成交额", "成交额", 16),
    ]
    with TdxV2Client(args.host, args.port, timeout=args.timeout) as client:
        bars = client.get_bars(auto_market(args.code), args.code,
                               count=min(count, 700), category=cat,
                               start=args.start)
    rows = []
    for bar in bars:
        s = str(bar.date)
        date_str = f"{s[:4]}-{s[4:6]}-{s[6:8]}"
        if intraday and bar.time_sec:
            date_str += f" {bar.time_sec // 3600:02d}:{bar.time_sec % 3600 // 60:02d}"
        rows.append({
            "日期": date_str,
            "开盘": f"{bar.open:.2f}",
            "最高": f"{bar.high:.2f}",
            "最低": f"{bar.low:.2f}",
            "收盘": f"{bar.close:.2f}",
            "成交量(股)": f"{bar.vol:,.0f}",
            "成交额": f"{bar.amount:,.0f}",
        })
    format_output(rows, columns, args.format)


def cmd_v2_minutes(args):
    """当日分时（新协议，含均价）"""
    from tdxrs.v2_client import TdxV2Client

    columns = [
        ("时间", "时间", 8), ("价格", "价格", 10), ("均价", "均价", 10),
        ("成交量(股)", "成交量(股)", 14),
    ]
    with TdxV2Client(args.host, args.port, timeout=args.timeout) as client:
        points = client.get_minute_time_data(auto_market(args.code), args.code)
    rows = [{
        "时间": pt.time,
        "价格": f"{pt.price:.2f}",
        "均价": f"{pt.avg_price:.2f}",
        "成交量(股)": f"{pt.vol:,.0f}",
    } for pt in points]
    format_output(rows, columns, args.format)


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
    p.add_argument("--source", default="v2", choices=["v2", "old"],
                   help="行情数据源: v2=新协议(默认) old=旧协议(已断供)")
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
    p.add_argument("--source", default="v2", choices=["v2", "old"],
                   help="行情数据源: v2=新协议(默认) old=旧协议(已断供)")
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
    p.add_argument("--timeout", type=float, default=5.0)
    p.set_defaults(func=cmd_servers)

    # ── v2（新一代 7709 协议：2026-07 起旧协议行情断供的替代通道）──
    p = sub.add_parser("v2", help="新一代 7709 协议（实时行情/K线/分时）")
    v2_sub = p.add_subparsers(dest="v2_command", required=True)

    q = v2_sub.add_parser("quote", help="实时行情")
    q.add_argument("code", help="股票代码，多只用逗号分隔 (最多20)")
    _add_v2_args(q)
    q.set_defaults(func=cmd_v2_quote)

    b = v2_sub.add_parser("bars", help="K线数据（全周期）")
    b.add_argument("code", help="股票代码")
    b.add_argument("--category", default="day", choices=list(_CATEGORY_MAP.keys()),
                   help="周期 (默认day)")
    b.add_argument("--start", type=int, default=0,
                   help="起始偏移 (0=最新窗口，>0 向历史前移)")
    b.add_argument("--count", type=int, default=CLI_LIMITS["bars_count"]["default"],
                    help=f"条数 (默认{CLI_LIMITS['bars_count']['default']}，服务端单次上限700)")
    _add_v2_args(b)
    b.set_defaults(func=cmd_v2_bars)

    m = v2_sub.add_parser("minutes", help="当日分时")
    m.add_argument("code", help="股票代码")
    _add_v2_args(m)
    m.set_defaults(func=cmd_v2_minutes)

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
