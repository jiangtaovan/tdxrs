"""终极验证：登录(00 03) -> K线(00 0a) -> 分时(00 07) -> 报价(00 05) 完整序列重放。"""

import socket
import struct

# ── 从 tdx3.pcap 提取 SYN 流的首帧（登录帧） ──
data = open("/Users/yiyi/github/tdxrs/research/tdx3.pcap", "rb").read()
off = 24
LOGIN = None
first_syn_flow = None
while off + 16 <= len(data):
    incl = struct.unpack("<I", data[off+8:off+12])[0]
    pkt = data[off+16:off+16+incl]
    off += 16 + incl
    if len(pkt) < 40 or struct.unpack(">H", pkt[12:14])[0] != 0x0800:
        continue
    ip = pkt[14:]
    ihl = (ip[0] & 0x0F) * 4
    if ip[9] != 6:
        continue
    tcp = ip[ihl:]
    sport, dport = struct.unpack(">HH", tcp[:4])
    doff = (tcp[12] >> 4) * 4
    flags = tcp[13]
    pl = tcp[doff:]
    if flags & 0x02 and not flags & 0x10 and dport == 7709 and LOGIN is None:
        first_syn_flow = sport
        continue
    if first_syn_flow and sport == first_syn_flow and dport == 7709 and pl.startswith(b"\x00\x03") and LOGIN is None:
        LOGIN = pl
if not LOGIN:
    raise SystemExit("未找到登录帧")

MINUTE = BARS = QUOTE = None
# 从 tdx2.pcap 提取 00 07 / 00 0a 请求帧
data2 = open("/Users/yiyi/github/tdxrs/research/tdx2.pcap", "rb").read()
off = 24
while off + 16 <= len(data2) and (MINUTE is None or BARS is None):
    incl = struct.unpack("<I", data2[off+8:off+12])[0]
    pkt = data2[off+16:off+16+incl]
    off += 16 + incl
    if len(pkt) < 40 or struct.unpack(">H", pkt[12:14])[0] != 0x0800:
        continue
    ip = pkt[14:]
    ihl = (ip[0] & 0x0F) * 4
    if ip[9] != 6:
        continue
    tcp = ip[ihl:]
    _sport, dport = struct.unpack(">HH", tcp[:4])
    if dport != 7709:
        continue
    doff = (tcp[12] >> 4) * 4
    pl = tcp[doff:]
    if pl.startswith(b"\x00\x07\x00\x3a") and MINUTE is None and len(pl) < 100:
        MINUTE = pl
    if pl.startswith(b"\x00\x0a\x00\x68") and BARS is None and len(pl) < 100:
        BARS = pl
# 从 tdx.pcap 提取 00 05 报价帧
data1 = open("/Users/yiyi/github/tdxrs/research/tdx.pcap", "rb").read()
off = 24
while off + 16 <= len(data1) and QUOTE is None:
    incl = struct.unpack("<I", data1[off+8:off+12])[0]
    pkt = data1[off+16:off+16+incl]
    off += 16 + incl
    if len(pkt) < 40 or struct.unpack(">H", pkt[12:14])[0] != 0x0800:
        continue
    ip = pkt[14:]
    ihl = (ip[0] & 0x0F) * 4
    if ip[9] != 6:
        continue
    tcp = ip[ihl:]
    _sport, dport = struct.unpack(">HH", tcp[:4])
    if dport != 7709:
        continue
    doff = (tcp[12] >> 4) * 4
    pl = tcp[doff:]
    if pl.startswith(b"\x00\x05\x00\x29") and len(pl) < 100:
        QUOTE = pl
assert MINUTE and BARS and QUOTE


def set_code(frame, mkt, code):
    f = bytearray(frame)
    i = f.find(b"300196")
    if i < 0:
        i = f.find(b"688801")
    f[i-2] = mkt
    f[i:i+6] = code.encode()
    return bytes(f)


def rd(s, t=3):
    s.settimeout(t)
    buf = b""
    try:
        while len(buf) < 262144:
            c = s.recv(16384)
            if not c:
                break
            buf += c
    except socket.timeout:
        pass
    return buf


def floats_in(buf, lo, hi, limit=8):
    out = []
    for i in range(16, min(len(buf), 2000) - 3, 4):
        v = struct.unpack("<f", buf[i:i+4])[0]
        if lo < abs(v) < hi:
            out.append(round(v, 2))
        if len(out) >= limit:
            break
    return out


print(f"登录帧: {len(LOGIN)}B {LOGIN[:24].hex(' ')}...")
for mkt, code, label, lo, hi in [(1, "600519", "茅台", 100, 2000), (0, "000001", "平安银行", 5, 30)]:
    print(f"\n===== {label} ({code}) =====")
    s = socket.create_connection(("121.36.248.138", 7709), timeout=8)
    s.sendall(LOGIN)
    r = rd(s, 3)
    print(f"  登录   -> {len(r)}B  头: {r[:16].hex(' ')}")
    s.sendall(set_code(BARS, mkt, code))
    r = rd(s, 4)
    print(f"  K线    -> {len(r)}B  floats={floats_in(r, lo, hi)}")
    s.sendall(set_code(MINUTE, mkt, code))
    r = rd(s, 3)
    print(f"  分时   -> {len(r)}B  floats={floats_in(r, lo, hi, 6)}")
    s.sendall(set_code(QUOTE, mkt, code))
    r = rd(s, 2)
    print(f"  报价   -> {len(r)}B  floats={floats_in(r, lo, hi, 6)}")
    s.close()
