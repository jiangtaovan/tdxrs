#!/usr/bin/env python3
"""⑥ 旧默认服务器 v2 兼容性 ⑦ 目录类 API 存活性。
需在装有原生模块的解释器下运行（tdxrs tool venv 或 REDSTACK 环境）。"""

import importlib.util
import sys
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "v2_client", Path(__file__).resolve().parents[1] / "python" / "tdxrs" / "v2_client.py"
)
_v2 = importlib.util.module_from_spec(_spec)
sys.modules["v2_client"] = _v2
_spec.loader.exec_module(_v2)

from tdxrs._internal import TdxDirectClient
from tdxrs.downloader import _DEFAULT_SERVERS

code, market = "600519", 1

for name, ip, port in _DEFAULT_SERVERS[:4]:
    try:
        with _v2.TdxV2Client(ip, port, timeout=6) as c:
            q = c.get_quote(market, code)
            print(f"⑥ {name}({ip}): v2 快照 OK price={q['price']:.2f}")
    except Exception as e:
        print(f"⑥ {name}({ip}): v2 失败 → {type(e).__name__}: {e}")

name, ip, port = _DEFAULT_SERVERS[0]
try:
    cl = TdxDirectClient(ip, port, 8.0)
    total = cl.get_security_count(1)
    lst = cl.get_security_list(1, 0)
    blk = cl.get_and_parse_block_info("block_zs.dat")
    print(f"⑦ 目录类@{name}: security_count(SH)={total}, security_list 前3={[d['code'] for d in (lst or [])[:3]]}, "
          f"block_zs 记录数={len(blk or [])}")
except Exception as e:
    print(f"⑦ 目录类: 失败 → {type(e).__name__}: {e}")
