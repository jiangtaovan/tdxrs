"""解码新协议日K记录布局：用 600519 已知价位定位日期/OHLC/量字段。"""

import socket
import struct

# ── 帧提取（复用已验证的帧） ──
data = open("/Users/yiyi/github/tdxrs/research/tdx3.pcap", "rb").read()
off = 24; LOGIN = None; first = None
while off + 16 <= len(data) and LOGIN is None:
    incl = struct.unpack("<I", data[off+8:off+12])[0]
    pkt = data[off+16:off+16+incl]; off += 16 + incl
    if len(pkt) < 40 or struct.unpack(">H", pkt[12:14])[0] != 0x0800: continue
    ip = pkt[14:]; ihl = (ip[0] & 0xF) * 4
    if ip[9] != 6: continue
    tcp = ip[ihl:]; sport, dport = struct.unpack(">HH", tcp[:4]); doff = (tcp[12] >> 4) * 4
    flags = tcp[13]; pl = tcp[doff:]
    if flags & 0x02 and not flags & 0x10 and dport == 7709 and first is None:
        first = sport; continue
    if first and sport == first and dport == 7709 and pl.startswith(b"\x00\x03"):
        LOGIN = pl

data2 = open("/Users/yiyi/github/tdxrs/research/tdx2.pcap", "rb").read()
off = 24; BARS = None
while off + 16 <= len(data2) and BARS is None:
    incl = struct.unpack("<I", data2[off+8:off+12])[0]
    pkt = data2[off+16:off+16+incl]; off += 16 + incl
    if len(pkt) < 40 or struct.unpack(">H", pkt[12:14])[0] != 0x0800: continue
    ip = pkt[14:]; ihl = (ip[0] & 0xF) * 4
    if ip[9] != 6: continue
    tcp = ip[ihl:]; _sport, dport = struct.unpack(">HH", tcp[:4])
    if dport != 7709: continue
    pl = tcp[(tcp[12] >> 4) * 4:]
    if pl.startswith(b"\x00\x0a\x00\x68") and len(pl) < 100:
        BARS = pl


def set_code(frame, mkt, code):
    f = bytearray(frame)
    i = f.find(b"300196")
    f[i-2] = mkt
    f[i:i+6] = code.encode()
    return bytes(f)


def fetch_bars(mkt, code):
    s = socket.create_connection(("121.36.248.138", 7709), timeout=8)
    s.sendall(LOGIN)
    s.settimeout(3)
    try:
        while s.recv(65536):
            pass
    except socket.timeout:
        pass
    s.sendall(set_code(BARS, mkt, code))
    buf = b""
    s.settimeout(6)
    try:
        while len(buf) < 300000:
            c = s.recv(16384)
            if not c:
                break
            buf += c
    except socket.timeout:
        pass
    s.close()
    return buf

buf = fetch_bars(1, "600519")
print(f"600519 日K响应: {len(buf)}B")
open("/Users/yiyi/github/tdxrs/research/bars_600519.bin", "wb").write(buf)
buf2 = fetch_bars(0, "000001")
print(f"000001 日K响应: {len(buf2)}B")
open("/Users/yiyi/github/tdxrs/research/bars_000001.bin", "wb").write(buf2)

# ── 定位日期字段: u32 in [19901219, 20260914] ──
def find_dates(b):
    hits = []
    for i in range(len(b) - 4):
        v = struct.unpack("<I", b[i:i+4])[0]
        if 19901219 <= v <= 20260914 and valid_ymd(v):
            hits.append(i)
    return hits


def valid_ymd(v):
    m = (v // 100) % 100
    d = v % 100
    return 1 <= m <= 12 and 1 <= d <= 31

hits = find_dates(buf)
print(f"疑似日期 u32 位置数: {len(hits)}")
# 聚类找等间隔序列
if len(hits) >= 3:
    diffs = [hits[i+1] - hits[i] for i in range(min(len(hits)-1, 30))]
    print("前30个间隔:", diffs[:30])
    from collections import Counter
    print("间隔众数:", Counter(diffs).most_common(5))
    # 打印前几个日期值
    for i in hits[:8]:
        print(f"  @{i}: {struct.unpack('<I', buf[i:i+4])[0]}")
