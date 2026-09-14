#!/usr/bin/env python3
"""分时载荷头/记录/尾原始字节标注 dump。"""

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

def annotate(p, lo, hi):
    for i in range(lo, min(hi, len(p)), 4):
        seg = p[i:i+4]
        if len(seg) < 4:
            print(f"  @{i:4d} {seg.hex(' ')}")
            continue
        u = struct.unpack("<I", seg)[0]
        f = struct.unpack("<f", seg)[0]
        tags = []
        if 1200 < f < 1350:
            tags.append(f"f32={f:.2f}")
        if 0 < u < 100000:
            tags.append(f"u32={u}")
        if u == 0:
            tags.append("zero")
        print(f"  @{i:4d} {seg.hex(' ')}  {' '.join(tags)}")

with _v2.TdxV2Client(HOST, PORT) as c:
    p = c.get_minute_time_data(market, code)
    n = len(p)
    print(f"n={n}")
    print("== 头部 0..110 ==")
    annotate(p, 0, 110)
    print("== 记录区采样 55..130（18B 步进） ==")
    for k in range(4):
        roff = 37 + k * 18
        rec = p[roff:roff+18]
        if len(rec) == 18:
            f1, f2 = struct.unpack("<2f", rec[:8])
            u1, u2 = struct.unpack("<2I", rec[8:16])
            u16 = struct.unpack("<H", rec[16:18])[0]
            print(f"  rec@{roff}: f1={f1:.2f} f2={f2:.2f} u1={u1} u2={u2} u16={u16} | {rec.hex(' ')}")
    print("== 记录区尾部/载荷尾 1980..n ==")
    annotate(p, 1980, n)
