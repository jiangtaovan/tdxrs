"""通达信协议录制代理：本地 7709 收流，转发到真实主站，双向落盘。

用途：让通达信终端（或 tdxrs）把"行情主站"指向 127.0.0.1:7709，
本代理原样转发字节并按连接记录两个方向的原始帧，用于对比
官方客户端与 tdxrs 的首包/握手差异。

用法:
    python3 record_proxy.py                # 上游默认 121.37.207.165:7709
    python3 record_proxy.py 119.29.19.242 7709
输出:
    captures/<时间戳>_<来源端口>/{c2s,s2c}.bin   原始字节流
    captures/<时间戳>_<来源端口>/meta.txt        连接元信息
"""

import asyncio
import sys
import time
from pathlib import Path

LISTEN_HOST = "127.0.0.1"
LISTEN_PORT = 7709
CAPTURE_DIR = Path(__file__).parent / "captures"


async def pump(reader: asyncio.StreamReader, writer: asyncio.StreamWriter,
               out: Path, tag: str) -> None:
    """单向搬运字节流并落盘，连接结束时打印统计。"""
    total = 0
    with out.open("ab") as f:
        try:
            while chunk := await reader.read(65536):
                writer.write(chunk)
                await writer.drain()
                f.write(chunk)
                f.flush()
                total += len(chunk)
        except (ConnectionResetError, asyncio.IncompleteReadError):
            pass
        finally:
            try:
                writer.close()
            except Exception:
                pass
    print(f"[{time.strftime('%H:%M:%S')}] {out.parent.name} {tag}: {total} bytes", flush=True)


async def handle(client_reader, client_writer, upstream_host, upstream_port):
    peer = client_writer.get_extra_info("peername")
    sport = peer[1] if peer else 0
    stamp = time.strftime("%Y%m%d_%H%M%S")
    session = CAPTURE_DIR / f"{stamp}_{sport}"
    session.mkdir(parents=True, exist_ok=True)
    session.joinpath("meta.txt").write_text(
        f"listen={LISTEN_HOST}:{LISTEN_PORT} upstream={upstream_host}:{upstream_port}\n", encoding="utf-8")
    print(f"[{time.strftime('%H:%M:%S')}] 新连接 来自 {peer} -> {upstream_host}:{upstream_port}", flush=True)

    try:
        up_reader, up_writer = await asyncio.wait_for(
            asyncio.open_connection(upstream_host, upstream_port), timeout=8.0)
    except Exception as e:
        print(f"上游连接失败 {upstream_host}:{upstream_port}: {e}", flush=True)
        client_writer.close()
        return

    c2s = session / "c2s.bin"  # 客户端 -> 服务器（首包在这里：握手/注册）
    s2c = session / "s2c.bin"  # 服务器 -> 客户端
    await asyncio.gather(
        pump(client_reader, up_writer, c2s, "客户端->主站"),
        pump(up_reader, client_writer, s2c, "主站->客户端"),
    )


async def main():
    upstream_host = sys.argv[1] if len(sys.argv) > 1 else "121.37.207.165"
    upstream_port = int(sys.argv[2]) if len(sys.argv) > 2 else 7709
    CAPTURE_DIR.mkdir(exist_ok=True)
    server = await asyncio.start_server(
        lambda r, w: handle(r, w, upstream_host, upstream_port), LISTEN_HOST, LISTEN_PORT)
    print(f"录制代理就绪: {LISTEN_HOST}:{LISTEN_PORT} -> {upstream_host}:{upstream_port}", flush=True)
    print(f"抓包目录: {CAPTURE_DIR}", flush=True)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
