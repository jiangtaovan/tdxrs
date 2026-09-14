"""解析 pcap v2：不要求 SYN，按流收集双向前几个载荷帧。"""

import struct
import sys
from collections import OrderedDict


def read_pcap(path):
    data = open(path, "rb").read()
    magic = data[:4]
    endian = "<" if magic in (b"\xd4\xc3\xb2\xa1", b"\x4d\x3c\xb2\xa1") else ">"
    nano = magic in (b"\x4d\x3c\xb2\xa1", b"\xa1\xb2\x3c\x4d")
    linktype = struct.unpack(endian + "I", data[20:24])[0]
    off = 24
    while off + 16 <= len(data):
        ts_sec, ts_frac, incl, orig = struct.unpack(endian + "IIII", data[off:off+16])
        ts = ts_sec + (ts_frac / 1e9 if nano else ts_frac / 1e6)
        yield ts, linktype, data[off+16:off+16+incl]
        off += 16 + incl


def tcp_payload(pkt, linktype):
    if linktype == 1:
        if len(pkt) < 14:
            return None
        etype = struct.unpack(">H", pkt[12:14])[0]
        off = 14
        while etype == 0x8100 and len(pkt) >= off + 4:
            etype = struct.unpack(">H", pkt[off+2:off+4])[0]; off += 4
        if etype != 0x0800:
            return None
        ip = pkt[off:]
    else:
        ip = pkt
    if len(ip) < 20 or (ip[0] >> 4) != 4:
        return None
    ihl = (ip[0] & 0x0F) * 4
    if ip[9] != 6:
        return None
    src = ".".join(map(str, ip[12:16])); dst = ".".join(map(str, ip[16:20]))
    tcp = ip[ihl:]
    if len(tcp) < 20:
        return None
    sport, dport = struct.unpack(">HH", tcp[:4])
    doff = (tcp[12] >> 4) * 4
    if len(tcp) <= doff:
        return None
    return src, sport, dst, dport, tcp[doff:]


def main():
    path = sys.argv[1]
    flows = OrderedDict()
    for ts, lt, pkt in read_pcap(path):
        p = tcp_payload(pkt, lt)
        if not p:
            continue
        src, sport, dst, dport, payload = p
        if sport in (7709, 7615):
            key, direction = (dst, dport, src, sport), "s2c"
        elif dport in (7709, 7615):
            key, direction = (src, sport, dst, dport), "c2s"
        else:
            continue
        f = flows.setdefault(key, {"c2s": [], "s2c": []})
        if len(f[direction]) < 6:
            f[direction].append((ts, payload))

    for i, (key, f) in enumerate(flows.items(), 1):
        cli, srv = key[0], key[2]
        print(f"\n{'='*72}\n[{i}] {cli} <-> {srv}")
        for tag, items in (("客户端->主站", f["c2s"]), ("主站->客户端", f["s2c"])):
            print(f"  --- {tag} ({len(items)} 帧) ---")
            for ts, pl in items[:4]:
                show = pl[:96]
                print(f"  t={ts:.3f} len={len(pl):5d}: {show.hex(' ')}")
                ascii_part = "".join(chr(c) if 32 <= c < 127 else "." for c in show)
                print(f"      ascii: {ascii_part}")


if __name__ == "__main__":
    main()
