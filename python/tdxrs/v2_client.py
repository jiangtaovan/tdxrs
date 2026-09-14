"""通达信新一代 7709 行情协议客户端（纯 Python，零依赖）。

背景：2026-07 起通达信服务端对 pytdx 系旧帧（0c 02 系）断供行情数据
（TCP 通、目录/财务类照常应答，快照/K线/分时/除权静默返回空），官方
客户端改用本文实现的新帧格式。逆向结论（2026-09-14，线缆级验证）：

- 无握手密钥、无加密：裸 TCP + 正确帧格式即返回数据；登录帧为静态
  字节串（80B），可跨连接重放；
- 响应帧：[4B 魔数 b1 cb 74 00][u16 方法回显][8B][u16 载荷长][u16 载荷长回显]，
  载荷长位于 offset 12（LE u16），总长 = 16 + 载荷长；
- 日K记录 36B：[u32 日期 YYYYMMDD][u32 时间][f32 开][f32 高][f32 低][f32 收]
  [f32 成交额(元)][f32 成交量(股)][f32 流通股本(万股)]；载荷尾部 120B 为 GBK 名称区。
  时间字段：日线为 0；分钟/小时线为距午夜的秒数（如 37500=10:25:00，为 K 线结束时刻）；
- 实时快照含五连价格块 [昨收][开][高][低][现价]（其后 +20 为成交量手数 u32、
  +28 为成交额 f32 元），块起点靠 OHLC 关系 + 量额互洽锚定（见 _find_price_block）；
- 分时记录 18B（记录区自载荷 offset 37 起）：[f32 价格][f32 均价][u32 成交量]
  [u32 保留][u16 距午夜分钟数]（571=09:31）；尾部为少量摘要字段；
- bars 请求帧：市场@12、代码@14、周期 u16@36（枚举与老协议一致）、
  起始偏移 u32@40（0=最新窗口向前）、条数 u16@44（服务端单次上限 700）；
- 市场字节：1=沪 0=深（与老协议一致）。

请求帧模板取自官方客户端抓包原文（见 research/），代码字段以哨兵
``C0DE00`` 占位，发送前替换为实际市场字节 + 6 位代码。
"""

from __future__ import annotations

import socket
import struct
from dataclasses import dataclass

MAGIC_RSP = b"\xb1\xcb\x74\x00"
HEADER_LEN = 16
CODE_SENTINEL = b"C0DE00"

MARKET_SH = 1
MARKET_SZ = 0

# 周期类别（2026-09-13 全枚举实测：与老协议 KLINE_* 同源同值）
KLINE_5MIN = 0
KLINE_15MIN = 1
KLINE_30MIN = 2
KLINE_1HOUR = 3
KLINE_DAY = 4
KLINE_WEEK = 5
KLINE_MONTH = 6
KLINE_3MONTH = 10
KLINE_YEAR = 11

# 服务端单次响应上限（payload u16 长度字段约束，实测 count>700 仍只回 700 条）
MAX_BARS_PER_REQUEST = 700

_DEFAULT_HOST = "121.36.248.138"

# 登录帧 80B（官方客户端建连后约 55ms 发出）
_BARS = bytes.fromhex(
    "000a00681301300030002e120000433044453030000000000000000000000000" + "000000000400010000000000bc02000001010001000000000000"
)  # 58B 代码@14 市场@12
_MINUTE = bytes.fromhex(
    "0007003a0101280028002d120000433044453030000000000000000000000000" + "000000000000000001000000000000000000"
)  # 50B 代码@14 市场@12
_QUOTE = bytes.fromhex(
    "000500290001300030002b12ffffffffff58ff07000000020000000000000000" + "0100000043304445303000000000000000000000000000000000"
)  # 58B 代码@36 市场@34
_SNAPSHOT = bytes.fromhex(
    "000d002c12012d002d002c12060000000000000000000000000e00360000001b" + "000100fffce1cc3f080302000000000000000000000001"
)  # 55B
# 榜单报价轮询帧：同 _SNAPSHOT 骨架，@25=u16 榜单 id、@27=u16 记录偏移、@31=页大小
_RANK_POLL = bytes.fromhex(
    "000d002c12012d002d002c12060000000000000000000000000e00360000001b000100fffce1cc3f080302000000000000000000000001"
)  # 55B
_LOGIN = bytes.fromhex(
    "000300010001460046000f1204002d3100000000000000000027100e00000000" + "0000000000000000000000000000000000000000000000000000000000000000" + "00000000000000000000000000000000"
)  # 80B

_BARS_CODE_OFF = 14
_MINUTE_CODE_OFF = 14
_QUOTE_CODE_OFF = 36
_BARS_CATEGORY_OFF = 36
_BARS_COUNT_OFF = 44


def _put_code(frame: bytes, code_off: int, market: int, code: str) -> bytes:
    f = bytearray(frame)
    f[code_off : code_off + 6] = code.encode()
    f[code_off - 2] = market
    return bytes(f)


@dataclass
class Bar:
    date: int
    open: float
    high: float
    low: float
    close: float
    amount: float
    vol: float
    float_share: float
    time_sec: int = 0  # 盘中K线结束时刻（距午夜秒数）；日线为 0


@dataclass
class MinutePoint:
    minute: int        # 距午夜分钟数（571=09:31）
    price: float       # 分钟末价
    avg_price: float   # 当日累计均价
    vol: float         # 分钟成交量（股）

    @property
    def time(self) -> str:
        return f"{self.minute // 60:02d}:{self.minute % 60:02d}"


@dataclass
class RankQuote:
    """榜单报价批的 196B 记录（字段偏移为 2026-09-14 抓包锚定）。"""
    code: str
    name: str
    last_close: float
    open: float
    high: float
    low: float
    price: float
    vol: float        # 股（记录内为 u32 手 ×100）
    amount: float     # 元


class TdxV2Error(Exception):
    pass


class TdxV2NoQuote(TdxV2Error):
    """停牌/无成交的退化快照：昨收有值，开/高/低/现/量/额全 0。

    是服务端对停牌股的合法应答而非格式变更；调用方应跳过该票，
    而不是当作通道故障整批回退。
    """


class TdxV2Client:
    """新一代 7709 协议客户端。线程不安全，一连接一用，连接内可连续多请求。"""

    def __init__(self, host: str = _DEFAULT_HOST, port: int = 7709, timeout: float = 6.0):
        self.host = host
        self.port = port
        self.timeout = timeout
        self._sock: socket.socket | None = None

    # ── 连接管理 ──
    def connect(self) -> None:
        self._sock = socket.create_connection((self.host, self.port), timeout=self.timeout)
        self._send(_LOGIN)
        self._recv_payload()  # 登录应答（203B 载荷）

    def close(self) -> None:
        if self._sock:
            try:
                self._sock.close()
            finally:
                self._sock = None

    def __enter__(self):
        self.connect()
        return self

    def __exit__(self, *_):
        self.close()

    # ── 收发原语 ──
    def _send(self, data: bytes) -> None:
        assert self._sock
        self._sock.sendall(data)

    def _recv_payload(self) -> bytes:
        assert self._sock
        self._sock.settimeout(self.timeout)
        header = b""
        while len(header) < HEADER_LEN:
            chunk = self._sock.recv(HEADER_LEN - len(header))
            if not chunk:
                raise TdxV2Error("连接被服务端关闭")
            header += chunk
        if not header.startswith(MAGIC_RSP):
            raise TdxV2Error(f"响应魔数不匹配: {header[:4].hex()}")
        payload_len = struct.unpack("<H", header[12:14])[0]
        payload = b""
        while len(payload) < payload_len:
            chunk = self._sock.recv(payload_len - len(payload))
            if not chunk:
                raise TdxV2Error("载荷不完整")
            payload += chunk
        return payload

    # ── 业务接口 ──
    def get_bars(
        self, market: int, code: str, count: int = 700, category: int = KLINE_DAY,
        start: int = 0,
    ) -> list[Bar]:
        """K 线。start=0 取最新 count 条，>0 向历史方向前移（u32@40 偏移字段，
        实测与最新窗口无缝衔接）。count 服务端上限 700。周期枚举见模块常量
        （5min/15min/30min/1h/日/周/月/季/年全可用）。"""
        f = bytearray(_BARS)
        f[_BARS_CATEGORY_OFF : _BARS_CATEGORY_OFF + 2] = struct.pack("<H", category)
        f[_BARS_COUNT_OFF : _BARS_COUNT_OFF + 2] = struct.pack("<H", count)
        f[40:44] = struct.pack("<I", start)
        self._send(_put_code(bytes(f), _BARS_CODE_OFF, market, code))
        return self._decode_bars(self._recv_payload(), code)

    @staticmethod
    def _decode_bars(payload: bytes, code: str) -> list[Bar]:
        # 载荷（16B 帧头已剥）：[0:2] 市场/填充回显，[2:8] 代码回显；36B 记录流
        # 自 offset 33 起；尾部 120B 为 GBK 名称区。
        if payload[2:8] != code.encode():
            raise TdxV2Error(f"日K响应代码回显不匹配: {payload[2:8]!r} != {code}")
        bars: list[Bar] = []
        pos = 33
        end = len(payload) - 120
        while pos + 36 <= end:
            rec = payload[pos : pos + 36]
            (date, tm) = struct.unpack("<II", rec[:8])
            o, h, l, c, amount, vol = struct.unpack("<6f", rec[8:32])
            (share,) = struct.unpack("<f", rec[32:36])
            bars.append(Bar(int(date), o, h, l, c, amount, float(vol), share, int(tm)))
            pos += 36
        return bars

    def get_quote(self, market: int, code: str) -> dict:
        """实时快照，返回解析后的字段 dict：
        last_close / open / high / low / price / vol(股) / amount(元)。"""
        self._send(_put_code(_QUOTE, _QUOTE_CODE_OFF, market, code))
        return _decode_quote(self._recv_payload(), code)

    def get_quote_raw(self, market: int, code: str) -> bytes:
        """实时快照原始载荷（供字段研究）。"""
        self._send(_put_code(_QUOTE, _QUOTE_CODE_OFF, market, code))
        return self._recv_payload()

    def get_minute_time_data_raw(self, market: int, code: str) -> bytes:
        """当日分时原始载荷（供字段研究）。"""
        self._send(_put_code(_MINUTE, _MINUTE_CODE_OFF, market, code))
        return self._recv_payload()

    def get_minute_time_data(self, market: int, code: str) -> list[MinutePoint]:
        """当日分时（已解码），按时间升序。"""
        self._send(_put_code(_MINUTE, _MINUTE_CODE_OFF, market, code))
        return decode_minute(self._recv_payload())

    def get_market_snapshot(self) -> bytes:
        """市场快照原始载荷（00 0d）。"""
        self._send(_SNAPSHOT)
        return self._recv_payload()

    def get_rank_page(self, list_id: int, offset: int, limit: int = 32) -> list["RankQuote"]:
        """榜单报价分页批（2026-09-14 逆向，官方客户端多票页面同款帧）。

        list_id 为服务端榜单编号（14=全市场榜，含北交所与停牌外全部票）；
        offset 为记录偏移（步进=limit）；响应为裸批载荷：
        [24B 前缀][u32 记录数@24][count×196B 记录@28]。同一连接可连续翻页，
        官方客户端单连接实测 24 次轮询。
        """
        f = bytearray(_RANK_POLL)
        f[25:27] = struct.pack("<H", list_id)
        f[27:29] = struct.pack("<H", offset)
        f[31] = limit
        self._send(bytes(f))
        return parse_rank_records(self._recv_payload())

    def rank_quote_total(self, list_id: int = 14) -> int:
        """榜单列表总条数（载荷前缀 u32@20）。"""
        f = bytearray(_RANK_POLL)
        f[25:27] = struct.pack("<H", list_id)
        f[27:29] = struct.pack("<H", 0)
        f[31] = 1
        self._send(bytes(f))
        body = self._recv_payload()
        return struct.unpack("<I", body[20:24])[0] if len(body) >= 24 else 0

    @staticmethod
    def decode_gbk_name(bars_payload: bytes) -> str:
        """从日K载荷尾部 120B 提取 GBK 证券名称。"""
        tail = bars_payload[-120:-88]
        return tail.split(b"\x00")[0].decode("gbk", errors="replace")


def decode_bars(payload: bytes, code: str) -> list[Bar]:
    """公共入口：解析日K载荷（与 TdxV2Client._decode_bars 同一实现）。"""
    return TdxV2Client._decode_bars(payload, code)


def decode_quote(payload: bytes, code: str) -> dict:
    """公共入口：解析实时快照载荷（与 get_quote 同一实现）。"""
    return _decode_quote(payload, code)


# 榜单批记录 196B：[6B 代码][16B 零][GBK 名称@22][..][昨收@66][开@70][高@74]
# [低@78][现价@82][u32 量(手)@86][..][f32 额@94]；响应前缀 28B 含 u32 记录数@24。
_RANK_REC_SIZE = 196
_RANK_REC_OFF = 28


def parse_rank_records(payload: bytes) -> list[RankQuote]:
    """解析榜单报价批载荷（get_rank_page 的公共入口）。"""
    if len(payload) < _RANK_REC_OFF:
        return []
    count = struct.unpack("<I", payload[24:28])[0]
    count = min(count, (len(payload) - _RANK_REC_OFF) // _RANK_REC_SIZE)
    out: list[RankQuote] = []
    for k in range(count):
        rec = payload[_RANK_REC_OFF + k * _RANK_REC_SIZE:
                      _RANK_REC_OFF + (k + 1) * _RANK_REC_SIZE]
        if len(rec) < 98:
            break
        code = rec[0:6].decode("ascii", "replace")
        name = rec[22:54].split(b"\x00")[0].decode("gbk", "replace")
        last_close, o, h, low, price = struct.unpack("<5f", rec[66:86])
        (vol_lots,) = struct.unpack("<I", rec[86:90])
        (amount,) = struct.unpack("<f", rec[94:98])
        out.append(RankQuote(
            code=code, name=name,
            last_close=float(last_close), open=float(o), high=float(h),
            low=float(low), price=float(price),
            vol=float(vol_lots) * 100.0, amount=float(amount),
        ))
    return out


# 分时记录 18B；记录区自载荷 offset 37 起（前 37B 为市场/代码回显 + 实时摘要，
# 尾部为摘要字段，均按时间字段合法性自动截停）
_MINUTE_REC_OFF = 37
_MINUTE_REC_SIZE = 18


def decode_minute(payload: bytes) -> list[MinutePoint]:
    """解析当日分时载荷：[f32 价格][f32 均价][u32 成交量(股)][u32 保留]
    [u16 距午夜分钟数]。以时间字段必须落在 A 股交易时刻表内截停记录流。"""
    points: list[MinutePoint] = []
    pos = _MINUTE_REC_OFF
    n = len(payload)
    while pos + _MINUTE_REC_SIZE <= n:
        price, avg = struct.unpack("<2f", payload[pos : pos + 8])
        vol, _rsv = struct.unpack("<2I", payload[pos + 8 : pos + 16])
        (minute,) = struct.unpack("<H", payload[pos + 16 : pos + 18])
        if not (570 <= minute <= 960) or not (0.0 < price <= 1e6):
            break
        points.append(MinutePoint(int(minute), float(price), float(avg), float(vol)))
        pos += _MINUTE_REC_SIZE
    if not points:
        raise TdxV2Error("分时载荷未解析出记录（休市或格式变更）")
    return points


def _decode_quote(payload: bytes, code: str) -> dict:
    """解析实时快照。价格块靠 OHLC 关系锚定（帧内含请求回显与变长名称区，
    无固定偏移）；返回字段：last_close/open/high/low/price、vol(股)、amount(元)。
    停牌股的退化快照（昨收>0、开高低现/量/额全 0）与无数据短帧
    （如 605081 退市清除后的 26B 应答）抛 TdxV2NoQuote。"""
    if len(payload) < 34:
        # 有效应答至少要放下代码回显（@28..34）；更短即服务端无此代码数据
        raise TdxV2NoQuote(f"服务端短帧无数据（{len(payload)}B）")
    if payload[28:34] != code.encode():
        raise TdxV2Error(f"快照响应代码回显不匹配: {payload[28:34]!r} != {code}")
    try:
        anchor = _find_price_block(payload)
    except TdxV2Error:
        last_close = _find_no_quote_block(payload)
        if last_close is None:
            raise
        raise TdxV2NoQuote(
            f"停牌/无成交快照（昨收 {last_close}，开高低/量/额全 0）"
        ) from None
    vol_lots = struct.unpack("<I", payload[anchor + 20 : anchor + 24])[0]
    amount = struct.unpack("<f", payload[anchor + 28 : anchor + 32])[0]
    return {
        "last_close": struct.unpack("<f", payload[anchor : anchor + 4])[0],
        "open": struct.unpack("<f", payload[anchor + 4 : anchor + 8])[0],
        "high": struct.unpack("<f", payload[anchor + 8 : anchor + 12])[0],
        "low": struct.unpack("<f", payload[anchor + 12 : anchor + 16])[0],
        "price": struct.unpack("<f", payload[anchor + 16 : anchor + 20])[0],
        "vol": float(vol_lots) * 100.0,  # 手 → 股
        "amount": float(amount),
    }


def _find_price_block(payload: bytes) -> int:
    """在载荷中定位 [昨收][开][高][低][现价] 五连 float 块的起点。

    除 OHLC 关系外加两条硬判据：价格须在 [0.01, 1e6]（排除请求回显区/
    GBK 名称字节被误读为次正规浮点）；量额互洽——成交额/成交量换算的
    均价必须落在 [low, high] 内（排除偶发的正浮点组合）。
    """
    for pos in range(32, len(payload) - 36):
        vals = struct.unpack("<5f", payload[pos : pos + 20])
        prev_close, o, h, l, price = vals
        if not all(0.01 <= v <= 1e6 for v in vals):
            continue
        if h >= max(o, l, price) and l <= min(o, price) and h >= l:
            vol_lots = struct.unpack("<I", payload[pos + 20 : pos + 24])[0]
            amount = struct.unpack("<f", payload[pos + 28 : pos + 32])[0]
            if vol_lots > 0 and amount > 0:
                avg = amount / (vol_lots * 100.0)
                if not (l * 0.98 <= avg <= h * 1.02):
                    continue
            return pos
    raise TdxV2Error("快照载荷未找到价格块（休市或格式变更）")


def _find_no_quote_block(payload: bytes) -> float | None:
    """识别停牌/无成交退化快照：五连价 [昨收][0][0][0][现] 中开/高/低必为
    精确 0，昨收>0，现价为 0 或等于昨收（无成交参考价），且量/额全 0。

    仅在正常价格块锚定失败后调用。自名称区前界（帧尾固定 120B 为 GBK
    名称，见 decode_gbk_name）向回扫：前置字段区/名称区的字节伪影
    （如 *ST康佳A 的 GBK 串恰被误读为正 float 26.2）都在真块之前，
    倒序首个命中即真昨收。实证形态（2026-09-14）：*ST康佳A 昨收 2.46
    现价 0；600301 昨收=现价 45.43，开高低 0，量额均 0。
    """
    for pos in range(len(payload) - 136, 32, -1):
        vals = struct.unpack("<5f", payload[pos : pos + 20])
        last_close, o, h, low, price = vals
        if not 0.01 < last_close <= 1e6:
            continue
        if o != 0.0 or h != 0.0 or low != 0.0:
            continue
        if price != 0.0 and price != last_close:
            continue
        (vol_lots,) = struct.unpack("<I", payload[pos + 20 : pos + 24])
        (amount,) = struct.unpack("<f", payload[pos + 28 : pos + 32])
        if vol_lots == 0 and amount == 0.0:
            return float(last_close)
    return None
