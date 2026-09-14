#!/usr/bin/env python3
"""v2 协议能力探测：偏移字段 / 周期类别 / 分时载荷 / 旧服务器兼容性。

2026-09-13 为把 CLI 全部命令迁移到 v2 做的前置验证。
"""

import importlib.util
import struct
import sys
from pathlib import Path

# 直接按路径加载纯 Python v2_client，绕开依赖原生模块的包 __init__
_spec = importlib.util.spec_from_file_location(
    "v2_client", Path(__file__).resolve().parents[1] / "python" / "tdxrs" / "v2_client.py"
)
_v2 = importlib.util.module_from_spec(_spec)
sys.modules["v2_client"] = _v2
_spec.loader.exec_module(_v2)
TdxV2Client = _v2.TdxV2Client
_BARS = _v2._BARS
_BARS_CATEGORY_OFF = _v2._BARS_CATEGORY_OFF
_BARS_COUNT_OFF = _v2._BARS_COUNT_OFF
_BARS_CODE_OFF = _v2._BARS_CODE_OFF
_put_code = _v2._put_code

HOST, PORT = "121.36.248.138", 7709


def raw_bars(client, market, code, category, count, u32_at_40=0):
    """发送 bars 帧并返回原始载荷（可改 u32@40 探测偏移字段）。"""
    f = bytearray(_BARS)
    f[_BARS_CATEGORY_OFF:_BARS_CATEGORY_OFF + 2] = struct.pack("<H", category)
    f[_BARS_COUNT_OFF:_BARS_COUNT_OFF + 2] = struct.pack("<H", count)
    f[40:44] = struct.pack("<I", u32_at_40)
    client._send(_put_code(bytes(f), _BARS_CODE_OFF, market, code))
    return client._recv_payload()


def bars_dates(payload, code):
    try:
        bars = _v2.decode_bars(payload, code)
    except Exception as e:
        return None, f"decode失败: {e}"
    return bars, None


def main():
    code, market = "600519", 1

    with TdxV2Client(HOST, PORT) as c:
        # ① 基线：count=700
        p = raw_bars(c, market, code, 4, 700)
        bars, err = bars_dates(p, code)
        n = len(bars) if bars else 0
        print(f"① 基线 count=700: {n} 条, 载荷 {len(p)}B, "
              f"首 {bars[0].date if bars else '-'} 末 {bars[-1].date if bars else '-'}")

        # ② u32@40 = 700 → 若为起始偏移，窗口应整体前移且不与基线重叠
        p = raw_bars(c, market, code, 4, 700, u32_at_40=700)
        bars, err = bars_dates(p, code)
        if bars:
            print(f"② u32@40=700 count=700: {len(bars)} 条, "
                  f"首 {bars[0].date} 末 {bars[-1].date}  (若末日期 < 基线首日期 → 偏移字段成立)")
        else:
            print(f"② u32@40=700: 解码失败 {err}, 载荷 {len(p)}B")

        # ③ 大 count：payload u16 上限探测
        p = raw_bars(c, market, code, 4, 5000)
        bars, err = bars_dates(p, code)
        print(f"③ count=5000: 载荷 {len(p)}B, 记录 {len(bars) if bars else err}")

        # ④ 周期类别探测（老协议枚举: 0=5min 1=15min 2=30min 3=1h 4=day 5=week 6=month 7=1min 8=k 9=day 10=3mon 11=year）
        for cat, name in [(0, "5min"), (1, "15min"), (2, "30min"), (3, "1hour"),
                          (5, "week"), (6, "month"), (10, "3month"), (11, "year")]:
            p = raw_bars(c, market, code, cat, 5)
            if len(p) < 40:
                print(f"④ category={cat:2d} {name:6s}: 载荷仅 {len(p)}B（空/拒绝）")
                continue
            # 直接把前 36B 记录区的 u32 读出来看日期形态
            head = struct.unpack("<I", p[33:37])[0]
            try:
                bars2, err2 = bars_dates(p, code)
                if bars2:
                    b = bars2[0]
                    print(f"④ category={cat:2d} {name:6s}: {len(bars2)} 条, "
                          f"首记录 date={b.date} o={b.open:.2f} c={b.close:.2f} vol={b.vol:.0f}")
                else:
                    print(f"④ category={cat:2d} {name:6s}: {err2} | 头部u32={head} 载荷{len(p)}B")
            except Exception as e:
                print(f"④ category={cat:2d} {name:6s}: 异常 {e} | 头部u32={head} 载荷{len(p)}B")

        # ⑤ 分时载荷结构分析
        p = c.get_minute_time_data(market, code)
        print(f"\n⑤ 分时载荷 {len(p)}B:")
        print("   头 64B:", p[:64].hex(" "))
        # 猜测：回显区 + N 条 [价格][成交量] 记录。尝试统计尾部规律
        body = p[34:]  # 跳过 34B 回显猜测区
        print(f"   跳过34B后 {len(body)}B, 若除以8={len(body)/8:.1f} 除以6={len(body)/6:.1f} 除以4={len(body)/4:.1f}")

    # ⑥ 旧协议默认服务器是否兼容 v2 帧
    from tdxrs.downloader import _DEFAULT_SERVERS
    name, ip, port = _DEFAULT_SERVERS[0]
    try:
        with TdxV2Client(ip, port, timeout=6) as c2:
            q = c2.get_quote(market, code)
            print(f"\n⑥ 旧默认服务器 {name}({ip}): v2 快照 OK, price={q['price']}")
    except Exception as e:
        print(f"\n⑥ 旧默认服务器 {name}({ip}): v2 失败 → {e}")

    # ⑦ 旧协议目录类 API 存活验证（stocks / index 命令依赖）
    from tdxrs._internal import TdxDirectClient
    try:
        cl = TdxDirectClient(ip, port, 6.0)
        total = cl.get_security_count(1)
        lst = cl.get_security_list(1, 0)
        print(f"⑦ 目录类: get_security_count(SH)={total}, get_security_list 前3={[d['code'] for d in lst[:3]]}")
    except Exception as e:
        print(f"⑦ 目录类: 失败 → {e}")


if __name__ == "__main__":
    main()
