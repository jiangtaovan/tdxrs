#!/usr/bin/env python3
"""分时载荷步长定位：用同一时刻的 5min K 收盘价序列做交叉锚定。"""

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
    # 同一连接内先取 5min K（末 12 根，含 10:25-11:20 收盘价），再取分时
    f = bytearray(_v2._BARS)
    f[_v2._BARS_CATEGORY_OFF:_v2._BARS_CATEGORY_OFF + 2] = struct.pack("<H", 0)
    f[_v2._BARS_COUNT_OFF:_v2._BARS_COUNT_OFF + 2] = struct.pack("<H", 12)
    c._send(_v2._put_code(bytes(f), _v2._BARS_CODE_OFF, market, code))
    pb = c._recv_payload()
    closes5 = []
    for i in range(12):
        rec = pb[33 + i*36: 33 + i*36 + 36]
        _, z, o, h, l, cl, amt, vol = struct.unpack("<I7f", rec[:32])
        closes5.append((struct.unpack("<I", rec[4:8])[0], cl))
    print("5min (end_sec, close):", closes5)
    latest_close = closes5[-1][1]

    p = c.get_minute_time_data(market, code)
    n = len(p)
    print(f"分时载荷 {n}B")

    # 全偏移扫描价格候选
    cands = []
    for i in range(n - 3):
        v = struct.unpack("<f", p[i:i+4])[0]
        if 1200.0 < v < 1350.0:
            cands.append((i, round(v, 2)))
    print(f"价格候选 {len(cands)} 个，前10: {cands[:10]}")
    print(f"后10: {cands[-10:]}")

    # 相邻候选间距分布 → 步长
    from collections import Counter
    gaps = Counter(b[0] - a[0] for a, b in zip(cands, cands[1:]) if b[0] - a[0] < 64)
    print("间距分布:", gaps.most_common(8))

    # 候选最后位置与最新5min收盘吻合度
    last_off, last_val = cands[-1]
    print(f"最后候选 offset={last_off} val={last_val} vs 最新5min收 {latest_close} (距载荷尾 {n - last_off - 4}B)")

    # 以最后候选为终点、按主流步长回溯，打印每 5 条的采样与 5min K 对照
    for stride in [8, 12, 16]:
        if (last_off - 0) % 1 != 0:
            continue
        seq = []
        off = last_off
        while off >= 0:
            v = struct.unpack("<f", p[off:off+4])[0]
            if not (1200.0 < v < 1350.0):
                break
            seq.append(round(v, 2))
            off -= stride
        seq.reverse()
        if len(seq) < 50:
            print(f"stride={stride}: 仅回溯出 {len(seq)} 条，跳过")
            continue
        # 5min 收盘应每隔 5 条出现
        samples = [seq[i] for i in range(len(seq)-1, -1, -5)][:12]
        samples.reverse()
        ref = [c for _, c in closes5]
        print(f"stride={stride}: 共 {len(seq)} 条; 每5条采样={samples}")
        print(f"             5min收盘   ={ref}")
        # 头部长度 = 最后候选绝对偏移 - (len(seq)-1)*stride
        head = last_off - (len(seq) - 1) * stride
        print(f"             推断头长={head}B (head%stride={head % stride})")
        # 记录内其它字段：打印头后前 3 条记录全字节
        for k in range(3):
            roff = head + k * stride
            rec = p[roff:roff + stride]
            print(f"             rec[{k}] @{roff}: {rec.hex(' ')}")
