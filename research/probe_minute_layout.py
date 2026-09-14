#!/usr/bin/env python3
"""v2 分时载荷布局分析 + 盘中K时间字段探测。"""

import importlib.util
import struct
import sys
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "v2_client", Path(__file__).resolve().parents[1] / "python" / "tdxrs" / "v2_client.py"
)
_v2 = importlib.util.module_from_spec(_spec)
sys.modules["v2_client"] = _v2
_spec.loader.exec_module(_v2)

HOST, PORT = "121.36.248.138", 7709
code, market = "600519", 1

with _v2.TdxV2Client(HOST, PORT) as c:
    # ① 盘中 K（5min）完整记录：时间字段在哪？
    f = bytearray(_v2._BARS)
    f[_v2._BARS_CATEGORY_OFF:_v2._BARS_CATEGORY_OFF + 2] = struct.pack("<H", 0)  # 5min
    f[_v2._BARS_COUNT_OFF:_v2._BARS_COUNT_OFF + 2] = struct.pack("<H", 12)
    c._send(_v2._put_code(bytes(f), _v2._BARS_CODE_OFF, market, code))
    p = c._recv_payload()
    print(f"① 5min K 载荷 {len(p)}B, 记录区自 33:")
    pos = 33
    for i in range(12):
        rec = p[pos:pos + 36]
        date, z, o, h, l, cl, amt, vol = struct.unpack("<I7f", rec[:32])
        (share,) = struct.unpack("<f", rec[32:36])
        # z 的原始字节与各解读
        zbytes = rec[4:8]
        zu32 = struct.unpack("<I", zbytes)[0]
        print(f"   [{i:2d}] date={date} z_raw={zbytes.hex()}(u32={zu32}, f32={z:.1f}) "
              f"o={o:.2f} h={h:.2f} l={l:.2f} c={cl:.2f} amt={amt:.0f} vol={vol:.0f} share={share:.0f}")
        pos += 36

    # ② 分时载荷全 dump + 结构分析
    p = c.get_minute_time_data(market, code)
    n = len(p)
    print(f"\n② 分时载荷 {n}B")
    print("   hex[0:96]:", p[:96].hex(" "))
    print("   hex[-48:]:", p[-48:].hex(" "))

    # 找连续 float 价格区（60..1400 元）与 u32 量区
    runs = []
    i = 0
    while i + 4 <= n:
        v = struct.unpack("<f", p[i:i+4])[0]
        if 100.0 < v < 1500.0 and v == v:
            j = i
            cnt = 0
            while j + 4 <= n:
                vv = struct.unpack("<f", p[j:j+4])[0]
                if 100.0 < vv < 1500.0:
                    cnt += 1
                    j += 4
                else:
                    break
            runs.append((i, cnt))
            i = j
        else:
            i += 4
    print("   价格候选连续区 (offset, count):", runs[:8])

    # ③ 逐分钟记录步长假设检验：从最长价格区起点回退找头部
    if runs:
        best = max(runs, key=lambda r: r[1])
        off, cnt = best
        print(f"   最长价格区 offset={off} count={cnt}")
        # 步长 = ? 打印该区域按 8B 步进的 (价格, 后4B u32)
        print("   按8B步进前6组:")
        for k in range(6):
            seg = p[off + k*8: off + k*8 + 8]
            if len(seg) == 8:
                price = struct.unpack("<f", seg[:4])[0]
                tail_u32 = struct.unpack("<I", seg[4:8])[0]
                tail_f = struct.unpack("<f", seg[4:8])[0]
                print(f"     [{k}] price={price:.2f} tail_u32={tail_u32} tail_f={tail_f:.3g}")
