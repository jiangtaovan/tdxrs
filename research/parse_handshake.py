"""提取 tdx3.pcap 中每条 SYN 流的客户端前几个载荷帧（建连握手分析）。"""

import struct
from collections import OrderedDict

data = open("/Users/yiyi/github/tdxrs/research/tdx3.pcap", "rb").read()
magic = data[:4]
endian = "<" if magic == b"\xd4\xc3\xb2\xa1" else ">"
off = 24
pkts = []
while off + 16 <= len(data):
    ts_sec, ts_frac, incl, _ = struct.unpack(endian + "IIII", data[off:off+16])
    pkts.append((ts_sec + ts_frac / 1e6, data[off+16:off+16+incl]))
    off += 16 + incl

flows = OrderedDict()  # (src,sport,dst,dport) -> {"syn_ts", "c2s": [(ts,payload)], "s2c_first": bytes}
for ts, pkt in pkts:
    if len(pkt) < 34 or struct.unpack(">H", pkt[12:14])[0] != 0x0800:
        continue
    ip = pkt[14:]
    ihl = (ip[0] & 0x0F) * 4
    if ip[9] != 6 or len(ip) < ihl + 20:
        continue
    tcp = ip[ihl:]
    sport, dport = struct.unpack(">HH", tcp[:4])
    doff = (tcp[12] >> 4) * 4
    flags = tcp[13]
    payload = tcp[doff:]
    src = ".".join(map(str, ip[12:16])); dst = ".".join(map(str, ip[16:20]))
    if flags & 0x02 and not flags & 0x10:  # SYN
        flows[(src, sport, dst, dport)] = {"syn": ts, "c2s": [], "s2c": b""}
        continue
    k = (src, sport, dst, dport)
    rk = (dst, dport, src, sport)
    if k in flows and payload:
        if len(flows[k]["c2s"]) < 4:
            flows[k]["c2s"].append((ts, payload))
    elif rk in flows and payload and len(flows[rk]["s2c"]) < 48:
        flows[rk]["s2c"] += payload[:48]

print(f"共 {len(flows)} 条 SYN 流")
for i, (k, f) in enumerate(flows.items(), 1):
    src, sport, dst, dport = k
    t0 = f["syn"]
    print(f"\n[{i}] {src}:{sport} -> {dst}:{dport}  SYN@+{t0:.3f}")
    if not f["c2s"]:
        print("    (窗口内无客户端数据)")
    for j, (ts, pl) in enumerate(f["c2s"], 1):
        show = pl[:72]
        print(f"  C→S #{j} (+{ts-t0:.3f}s, {len(pl)}B): {show.hex(' ')}")
        print(f"        ascii: {''.join(chr(c) if 32 <= c < 127 else '.' for c in show)}")
    if f["s2c"]:
        print(f"  S→C 首段: {f['s2c'][:48].hex(' ')}")
