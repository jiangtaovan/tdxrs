"""序列重放：同一连接上 报价(00 05) -> 分时(00 07) -> K线(00 0a)，验证连接内状态依赖。"""

import socket
import struct

data = open("/Users/yiyi/github/tdxrs/research/tdx2.pcap", "rb").read()
off = 24
samples = {}
while off + 16 <= len(data):
    incl = struct.unpack("<I", data[off+8:off+12])[0]
    pkt = data[off+16:off+16+incl]
    off += 16 + incl
    if len(pkt) < 40:
        continue
    if struct.unpack(">H", pkt[12:14])[0] != 0x0800:
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
    if not pl:
        continue
    for pfx in (b"\x00\x07\x00\x3a", b"\x00\x0a\x00\x68"):
        if pl.startswith(pfx) and pfx not in samples and len(pl) < 100:
            samples[pfx] = pl

MINUTE = samples[b"\x00\x07\x00\x3a"]
BARS = samples[b"\x00\x0a\x00\x68"]
QUOTE = bytes.fromhex(
    "000500290001300030002b12ffffffffff58ff070000000200000000"
    "000000000100010036383838303100000000000000000000000000000000000000"
)
print(f"样本: 分时={len(MINUTE)}B K线={len(BARS)}B 报价={len(QUOTE)}B")


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
            c = s.recv(8192)
            if not c:
                break
            buf += c
    except socket.timeout:
        pass
    return buf


def floats_in(buf, lo=0.5, hi=3000, limit=8):
    out = []
    for i in range(16, min(len(buf), 1200) - 3, 4):
        v = struct.unpack("<f", buf[i:i+4])[0]
        if lo < abs(v) < hi:
            out.append(round(v, 2))
        if len(out) >= limit:
            break
    return out


for mkt, code, label, lo, hi in [(1, "600519", "茅台", 100, 2000), (0, "000001", "平安银行", 5, 30)]:
    print(f"\n===== {label} ({code}) =====")
    s = socket.create_connection(("121.36.248.138", 7709), timeout=8)
    s.sendall(set_code(QUOTE, mkt, code))
    r = rd(s, 2)
    print(f"  报价帧 -> {len(r)}B  floats={floats_in(r, lo, hi, 4)}")
    s.sendall(set_code(MINUTE, mkt, code))
    r = rd(s, 3)
    print(f"  分时帧 -> {len(r)}B  floats={floats_in(r, lo, hi, 6)}")
    s.sendall(set_code(BARS, mkt, code))
    r = rd(s, 3)
    print(f"  K线帧 -> {len(r)}B  floats={floats_in(r, lo, hi, 8)}")
    s.close()
