"""HTTP CONNECT 录制代理：终端通讯设置里勾选"使用HTTP代理"指向本进程。

终端发出的 CONNECT host:port 会记录到 meta，隧道内的原始字节双向落盘
——通达信的行情协议字节流在隧道里原样经过，正好用于抓握手包。

用法:
    python3 record_httpproxy.py [监听端口，默认 18080]
输出:
    captures_http/<时间戳>_<来源端口>__<目标host_port>/{c2s,s2c}.bin
"""

import asyncio
import sys
import time
from pathlib import Path

LISTEN_HOST = "127.0.0.1"
LISTEN_PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18080
CAPTURE_DIR = Path(__file__).parent / "captures_http"


async def pump(reader: asyncio.StreamReader, writer: asyncio.StreamWriter,
               out: Path, tag: str) -> None:
    total = 0
    with out.open("ab") as f:
        try:
            while chunk := await reader.read(65536):
                writer.write(chunk)
                await writer.drain()
                f.write(chunk)
                f.flush()
                total += len(chunk)
        except (ConnectionResetError, asyncio.IncompleteReadError, BrokenPipeError):
            pass
        finally:
            try:
                writer.close()
            except Exception:
                pass
    print(f"[{time.strftime('%H:%M:%S')}] {out.parent.name} {tag}: {total} bytes", flush=True)


async def handle_tunnel(client_r, client_w, host, port):
    peer = client_w.get_extra_info("peername")
    sport = peer[1] if peer else 0
    stamp = time.strftime("%Y%m%d_%H%M%S")
    safe_target = f"{host}_{port}".replace(":", "_")
    session = CAPTURE_DIR / f"{stamp}_{sport}__{safe_target}"
    session.mkdir(parents=True, exist_ok=True)
    session.joinpath("meta.txt").write_text(
        f"CONNECT {host}:{port} via http-proxy {LISTEN_HOST}:{LISTEN_PORT}\n", encoding="utf-8")
    print(f"[{time.strftime('%H:%M:%S')}] 隧道建立 -> {host}:{port} (来自 :{sport})", flush=True)

    try:
        up_r, up_w = await asyncio.wait_for(
            asyncio.open_connection(host, port), timeout=10.0)
    except Exception as e:
        print(f"上游连接失败 {host}:{port}: {e}", flush=True)
        client_w.write(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
        await client_w.drain()
        client_w.close()
        return

    client_w.write(b"HTTP/1.1 200 Connection established\r\n\r\n")
    await client_w.drain()

    await asyncio.gather(
        pump(client_r, up_w, session / "c2s.bin", "客户端->主站"),
        pump(up_r, client_w, session / "s2c.bin", "主站->客户端"),
    )


async def handle(client_r, client_w):
    try:
        head = await asyncio.wait_for(client_r.readline(), timeout=15.0)
    except Exception:
        client_w.close()
        return
    parts = head.split()
    if len(parts) >= 2 and parts[0].upper() == b"CONNECT":
        hostport = parts[1].decode(errors="replace")
        host, _, port = hostport.rpartition(":")
        port = int(port) if port.isdigit() else 7709
        # 丢弃剩余请求头直到空行
        while True:
            line = await client_r.readline()
            if line in (b"\r\n", b"\n", b""):
                break
        await handle_tunnel(client_r, client_w, host, port)
    else:
        # 非 CONNECT（普通 HTTP）：不处理，直接关闭
        client_w.close()


async def main():
    CAPTURE_DIR.mkdir(exist_ok=True)
    server = await asyncio.start_server(handle, LISTEN_HOST, LISTEN_PORT)
    print(f"HTTP CONNECT 录制代理就绪: {LISTEN_HOST}:{LISTEN_PORT}", flush=True)
    print(f"抓包目录: {CAPTURE_DIR}", flush=True)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
