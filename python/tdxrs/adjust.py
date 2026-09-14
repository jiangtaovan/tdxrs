"""复权调整（纯 Python 移植自 src/protocol/adjuster.rs v0.6.6 算法）。

背景：服务端已断供旧协议 K 线，但旧协议除权目录类接口仍然存活
（2026-09-13 实测 get_xdxr_info 正常返回），因此复权流程改为：
v2 协议取原始 K 线 + 旧协议取除权记录 + 本模块本地调整。

算法与精度约定与 Rust 实现一致：

- 仅处理 category == 1 的综合除权事件；
- 因子 = (C - D + P_r × R_r) / (C × (1 + R_b + R_r))，各项为每股口径；
- QFQ 自最新向历史累乘事件之后发生的因子，HFQ 用倒数自最旧向最新累乘；
- 前收盘价 close_before：先在 bars 内找日期严格小于事件日的最后一条，
  找不到再回退 context_bars；两者都无则跳过该事件；
- 除权日本身不应用本事件因子（严格大于/小于等于分界）；
- 全程 f64，输出统一四舍五入到 3 位小数（FQ_PRICE_PRECISION）。

bars 元素为 v2 Bar（date 为 YYYYMMDD 整数）或含同名键的 dict，
返回与输入同构的列表（dataclass 复制，dict 复制）。
"""

from __future__ import annotations

import dataclasses

FQ_PRICE_PRECISION = 3

# 复权上下文分档（近似年数 × 240 交易日，与 Rust FqContextTier 一致）
TIER_BARS = {"low": 2400, "mid": 4800, "high": 7200}


def _get(bar, key):
    if isinstance(bar, dict):
        return bar[key]
    return getattr(bar, key)


def _date_key(xd: dict) -> int:
    return int(xd.get("year", 0)) * 10000 + int(xd.get("month", 0)) * 100 + int(xd.get("day", 0))


def collect_events(xdxr_list: list[dict]) -> list[tuple[int, dict]]:
    """提取 category==1 的除权事件（升序），单价字段折算为每股口径。"""
    events = []
    for xd in xdxr_list or []:
        if xd.get("category") != 1:
            continue
        events.append((_date_key(xd), {
            "div": float(xd.get("fenhong") or 0.0) / 10.0,
            "bonus": float(xd.get("songzhuangu") or 0.0) / 10.0,
            "rights": float(xd.get("peigu") or 0.0) / 10.0,
            "rights_price": float(xd.get("peigujia") or 0.0),
        }))
    events.sort(key=lambda e: e[0])
    return events


def calc_qfq_factor(close_before: float, parts: dict) -> float:
    denominator = close_before * (1.0 + parts["bonus"] + parts["rights"])
    numerator = close_before - parts["div"] + parts["rights_price"] * parts["rights"]
    if abs(denominator) < 1e-10 or abs(close_before) < 1e-10:
        return 1.0
    return numerator / denominator


def find_close_before(bars, context_bars, date_key: int):
    """事件前收盘价：bars 正向取最后一条 date < date_key，否则 context 反向找。"""
    found = None
    for b in bars:
        if _get(b, "date") < date_key:
            found = b
        else:
            break
    if found is not None:
        return _get(found, "close")
    for b in reversed(list(context_bars or [])):
        if _get(b, "date") < date_key:
            return _get(b, "close")
    return None


def adjust_bars(bars, context_bars, xdxr_list: list[dict], fq: int):
    """按 fq (1=前复权 2=后复权) 调整 K 线，返回新列表；fq 其他值原样返回。"""
    if fq not in (1, 2) or not bars:
        return list(bars)
    events = collect_events(xdxr_list)
    if not events:
        return list(bars)

    factor_map: dict[int, float] = {}
    for date_key, parts in events:
        close_before = find_close_before(bars, context_bars, date_key)
        if close_before is not None:
            factor_map[date_key] = calc_qfq_factor(close_before, parts)
    if not factor_map:
        return list(bars)

    scale = 10 ** FQ_PRICE_PRECISION
    out = []
    if fq == 1:  # 前复权：自最新向历史，累乘发生在 bar 之后的因子
        cumulative = 1.0
        ei = len(events) - 1
        for bar in reversed(bars):
            bar_key = int(_get(bar, "date"))
            while ei >= 0 and events[ei][0] > bar_key:
                f = factor_map.get(events[ei][0])
                if f is not None:
                    cumulative *= f
                ei -= 1
            if abs(cumulative - 1.0) > 1e-10:
                o, h, l, c = (_get(bar, "open") * cumulative, _get(bar, "high") * cumulative,
                              _get(bar, "low") * cumulative, _get(bar, "close") * cumulative)
            else:
                o, h, l, c = (_get(bar, "open"), _get(bar, "high"),
                              _get(bar, "low"), _get(bar, "close"))
            out.append(_copy_bar(bar, o, h, l, c, scale))
        out.reverse()
    else:  # 后复权：自最旧向最新，累乘 ≤ bar 的因子倒数
        cumulative = 1.0
        ei = 0
        for bar in bars:
            bar_key = int(_get(bar, "date"))
            while ei < len(events) and events[ei][0] <= bar_key:
                f = factor_map.get(events[ei][0])
                if f is not None:
                    cumulative *= 1.0 / f
                ei += 1
            if abs(cumulative - 1.0) > 1e-10:
                o, h, l, c = (_get(bar, "open") * cumulative, _get(bar, "high") * cumulative,
                              _get(bar, "low") * cumulative, _get(bar, "close") * cumulative)
            else:
                o, h, l, c = (_get(bar, "open"), _get(bar, "high"),
                              _get(bar, "low"), _get(bar, "close"))
            out.append(_copy_bar(bar, o, h, l, c, scale))
    return out


def _copy_bar(bar, o: float, h: float, l: float, c: float, scale: float):
    r = lambda p: round(p * scale) / scale  # noqa: E731
    if isinstance(bar, dict):
        nb = dict(bar)
        nb["open"], nb["high"], nb["low"], nb["close"] = r(o), r(h), r(l), r(c)
        return nb
    return dataclasses.replace(bar, open=r(o), high=r(h), low=r(l), close=r(c))


def auto_tier(xdxr_list: list[dict], current_year: int) -> str:
    """按最早事件年份自动分档（与 Rust FqService::auto_detect_tier 一致）。"""
    years = [_date_key(xd) // 10000 for xd in xdxr_list or []]
    if not years:
        return "mid"
    span = current_year - min(years)
    if span <= 10:
        return "low"
    if span <= 20:
        return "mid"
    return "high"
