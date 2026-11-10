# tdxrs API 参考

> 本文档覆盖 Python 公开 API。Rust 侧 API 请参见源码文档注释。
>
> 版本: v0.6.5 | 更新日期: 2026-07-01

---

## 快速索引

| 功能 | 对应章节 |
|------|---------|
| CLI 命令行工具 | [CLI 使用指南](CLI.md) |
| K线数据 (核心) | [TdxHqClient — K线](#数据获取--k线) |
| K线种类 | [category 对照表](#k线种类-category) |
| 复权 | [fq 参数](#复权类型-fq) |
| 实时行情 | [TdxHqClient — 实时行情](#数据获取--实时行情) |
| 分时 / 逐笔 | [TdxHqClient — 分时与逐笔](#数据获取--分时与逐笔) |
| 财务 / 除权 / 板块 | [TdxHqClient — 财务与除权](#数据获取--财务与除权) |
| 连接池 / 服务器管理 | [TdxHqClient — 连接管理](#连接管理) |
| 异步客户端 (并发) | [AsyncTdxHqClient](#asynctdxhqclient) |
| 裸连接方案 | [TdxDirectClient](#tdxdirectclient) |
| 基金数据 (ETF/LOF/REITs) | [TdxHqFundClient](#tdxhqfundclient) |
| 板块查询 | [TdxBlockClient](#tdxblockclient) |
| F10 公司资料 | [TdxF10Client](#tdxf10client) |
| 本地文件 | [Reader 类](#reader-类) |
| 错误码 | [错误码体系](#错误码体系) |
| 常量 | [常量子模块](#常量-tdxrsconstants) |
| 最佳实践 / 限流 / 优化 | [Python 最佳实践](PYTHON_BEST_PRACTICES.md) |

---

## Reader 类

### DailyBarReader

日线数据解析器（`.day` 文件）。

```python
from tdxrs import DailyBarReader
reader = DailyBarReader(coefficient=0.01)
```

| 参数 | 类型 | 默认 | 说明 |
|------|------|:--:|------|
| `coefficient` | float | 0.01 | 价格系数: A股=0.01, B股=0.001, 指数=0.01 |

| 方法 | 返回 | 说明 |
|------|------|------|
| `parse_data(data: bytes)` | `list[dict]` | 解析二进制日线数据 |
| `parse_file(path: str)` | `list[dict]` | 从文件读取并解析 |
| `parse_data_tuples(data: bytes)` | `list[tuple]` | 解析为 tuple 列表 (高性能) |
| `parse_file_tuples(path: str)` | `list[tuple]` | 文件 → tuple 列表 |
| `to_dataframe(data: bytes)` | `DataFrame` | 解析为 pandas DataFrame |
| `to_dataframe_file(path: str)` | `DataFrame` | 文件 → DataFrame |

**返回字段 (dict 模式)**: `date`, `open`, `high`, `low`, `close`, `amount`, `volume`, `year`, `month`, `day`

**Tuple 顺序**: `(date, open, high, low, close, amount, volume, year, month, day)`

### MinBarReader / LcMinBarReader

分钟线解析器（`.lc5` / `.lc1` 文件）。

```python
from tdxrs import MinBarReader, LcMinBarReader
```

| Reader | 格式 | 适用 |
|--------|------|------|
| MinBarReader | 整数价格 (×1000) | 常规 5 分钟线 |
| LcMinBarReader | 浮点价格 | LC 格式分钟线 |

| 方法 | 返回 | 说明 |
|------|------|------|
| `parse_data(data: bytes)` | `list[dict]` | 解析分钟线二进制数据 |
| `parse_file(path: str)` | `list[dict]` | 从文件读取并解析 |
| `parse_data_tuples(data: bytes)` | `list[tuple]` | 高性能 tuple 模式 |
| `parse_file_tuples(path: str)` | `list[tuple]` | 文件 → tuple |
| `to_dataframe(data: bytes)` | `DataFrame` | → pandas DataFrame |

返回额外字段: `hour`, `minute`

### BlockReader

板块数据解析器（`.dat` 文件）。

```python
from tdxrs import BlockReader
reader = BlockReader()
```

| 方法 | 返回 | 说明 |
|------|------|------|
| `parse_data(data: bytes)` | `list[dict]` | 扁平模式 (每只股票一行) |
| `parse_data_group(data: bytes)` | `list[dict]` | 分组模式 (每板块一行) |
| `parse_file(path: str)` | `list[dict]` | 从文件读取并解析 |

### FinancialReader

财务 gpcw 数据解析器。

```python
from tdxrs import FinancialReader
reader = FinancialReader()
```

| 方法 | 返回 | 说明 |
|------|------|------|
| `parse_data(data: bytes)` | `list[dict]` | 解析 gpcw 二进制财务数据 |
| `parse_file(path: str)` | `list[dict]` | 从文件读取并解析 |

返回: `[{"code": str, "report_date": int, "fields": [float, ...]}, ...]`

> `fields` 为 f32 财务指标值数组。部分字段含义待验证。

---

## 客户端类

### TdxHqClient

连接池行情客户端（主力）。支持自动心跳、断线重连、数据缓存、自动分页。

```python
from tdxrs import TdxHqClient
client = TdxHqClient()
```

#### 连接管理

| 方法 | 返回 | 说明 |
|------|------|------|
| `connect(ip, port, timeout=None)` | `bool` | 连接到指定服务器 |
| `connect_to_any(timeout=None)` | `bool` | 自动探测可用服务器 (优先→主→全部三级遍历) |
| `disconnect()` | — | 断开连接、停止心跳、清空连接池 |
| `is_connected()` | `bool` | 是否已连接 |

```python
client = TdxHqClient()
# 自动选择可用服务器 (推荐)
client.connect_to_any()
# 或指定服务器
client.connect("119.147.212.81", 7709, timeout=5.0)
```

#### 服务器管理

| 方法 | 说明 |
|------|------|
| `set_servers(servers: list[(name, ip, port)])` | 替换自定义优先服务器列表 |
| `add_server(name, ip, port)` | 在优先列表头部插入一台 |
| `reorder_servers(results)` | 按 `probe_servers()` 排序结果重排优先列表 |
| `probe_servers(timeout=3.0)` → `list[(name, ip, port, tcp_ms, hs_ms, api_ms)]` | 探测全部已知服务器，返回延迟排序 |

#### 服务器黑名单 (v0.6.7)

| 方法 | 说明 |
|------|------|
| `block_server(ip, port)` | 将服务器加入黑名单，连接时自动跳过 |
| `unblock_server(ip, port)` | 从黑名单移除服务器 |
| `blocked_servers()` → `list[(ip, port)]` | 获取黑名单列表 |
| `clear_blocked_servers()` | 清空黑名单 |

```python
client = TdxHqClient()

# 屏蔽已知不可用的服务器
client.block_server("183.60.224.177", 7709)  # 广发13
client.block_server("202.96.138.90", 7709)   # 海通服务器

# 连接时自动跳过黑名单服务器
client.connect_to_any()

# 查看黑名单
print(client.blocked_servers())  # [('183.60.224.177', 7709), ('202.96.138.90', 7709)]

# 清空黑名单
client.clear_blocked_servers()
```

> **适用场景**: 用户已知某台服务器在当前网络环境下不可用时，可预先屏蔽，避免连接超时。

#### 日K空响应自动重试 (v0.6.7)

当 `get_security_bars` 或 `get_index_bars` 请求日K线 (category >= 4) 返回空数据时，自动切换服务器重试。

| 行为 | 说明 |
|------|------|
| 触发条件 | category >= 4 (日K、周K、月K、季K、年K) 且返回空数据 |
| 最大重试 | 3 次 (首次 + 2 次重试) |
| 重试策略 | 遍历 PRIMARY_SERVERS，跳过当前服务器和黑名单 |
| 分钟线 | 不重试 (category < 4)，避免不必要的开销 |

**日志级别**:

| 级别 | 环境变量 | 输出内容 |
|------|----------|----------|
| `warn` (默认) | 无 | 仅在全部重试失败时输出警告 |
| `info` | `TDXRS_LOG=info` | 重试成功后输出摘要 (如 `got 5 bars for 600519 after 2 server switch(es)`) |
| `debug` | `TDXRS_LOG=debug` | 每次重试尝试的详细信息 |

```bash
# 生产环境 (默认，无噪音)
python your_script.py

# 监控服务器健康
TDXRS_LOG=info python your_script.py

# 调试连接问题
TDXRS_LOG=debug python your_script.py
```

#### 配置

| 方法 | 说明 |
|------|------|
| `set_auto_retry(enabled: bool)` | 启用/禁用内置重试 (生产环境建议关闭，用上层重试) |
| `set_cache_ttl(secs: int)` | 缓存有效期 (秒)，影响 `get_security_count` / `get_security_list` |
| `set_connect_timeout(secs: float)` | 连接超时 (秒) |
| `set_rate_limit(rps: int)` | 默认限流 (req/s)，0=禁用，默认 50，上限 200 |
| `set_rate_limit_daily(rps: int)` | 日K 限流 (req/s)，0=禁用，默认 15，上限 200 |
| `rate_limit_minute()` → `int` | 分时限流 (固定 10 req/s，不可修改) |
| `set_handshake_mode(mode: str)` | 握手模式: `"skip_cmd3"`(默认) / `"full"` / `"cmd1_only"` / `"none"` |
| `handshake_mode()` → `str` | 获取当前握手模式 |
| `pool_stats()` → `{"idle": int, "active": int, "total": int, "max_size": int}` | 连接池状态 |

**握手模式说明** (Issue #15):

默认 `"skip_cmd3"` (仅发 cmd1+cmd2)。多数通达信服务器把 `SETUP_CMD3` (0x1899) 当作
"客户端引擎版本上报"，版本不被认可即掐掉行情权限 (K线/行情/逐笔返回空或 `0x2003`)。
A/B 实测 (46台) 显示 Full 供数 2/72、SkipCmd3 供数 54/72，故默认取 SkipCmd3。

仅当遇到要求 cmd3 的服务器时才需显式回退:

```python
client = tdxrs.TdxHqClient()
client.set_handshake_mode("full")  # 仅当服务器要求 cmd3 时
client.connect_to_any()
```

所有客户端均支持 `set_handshake_mode()`:
`TdxHqClient` / `TdxSmartClient` / `TdxDirectClient` / `TdxBlockClient` / `TdxHqFundClient` / `AsyncTdxHqClient`

---

#### 数据获取 — K线

K 线是 tdxrs 核心功能，提供个股和指数的 OHLCV 数据。

##### 方法签名

```python
# 个股 K 线
get_security_bars(category, market, code, start=0, count=800, fq=1) -> list[dict]
get_security_bars_all(category, market, code, count=800, fq=1) -> list[dict]

# 指数 K 线 — 指数不存在复权概念，fq 参数无论传什么值均强制为 0
get_index_bars(category, market, code, start=0, count=800, fq=0) -> list[dict]
get_index_bars_all(category, market, code, count=800, fq=0) -> list[dict]

# DataFrame 模式 (直出 pandas)
get_security_bars_dataframe(category, market, code, start=0, count=800, fq=1) -> DataFrame
get_index_bars_dataframe(category, market, code, start=0, count=800, fq=0) -> DataFrame

# Tuple 高性能模式
get_security_bars_tuples(category, market, code, start=0, count=800, fq=1) -> list[tuple]
get_index_bars_tuples(category, market, code, start=0, count=800, fq=0) -> list[tuple]
```

##### 参数说明

| 参数 | 类型 | 默认 | 说明 |
|------|------|:--:|------|
| `category` | int | — | K 线种类，见下方 [对照表](#k线种类-category) |
| `market` | int | — | 市场代码，见 [市场代码](#市场代码) |
| `code` | str | — | 股票代码或指数代码，如 `"600519"` `"300750"` `"000001"` |
| `start` | int | 0 | 起始偏移位置 (0=最新数据开始)。设为 N 则跳过最近 N 根 |
| `count` | int | 800 | 请求数量。单次上限受 `MAX_KLINE_COUNT`(800) 限制 |
| `fq` | int | 1 | 复权类型（指数 K 线忽略此参数，强制 fq=0），见下方 |

##### 分页机制

TDX 服务端单次最多返回 **800 根** K 线。如需更多：

| 方法 | 行为 |
|------|------|
| `get_security_bars(..., count=N)` | 单次请求，N > 800 会被服务端截断为 800 |
| `get_security_bars_all(..., count=N)` | 自动分页，内部循环请求直到获取 N 根或数据耗尽 |

```python
# 获取全部历史数据 (自动分页)
all_bars = client.get_security_bars_all(KLINE_DAILY, MARKET_SH, "600519", count=5000)

# 手动分页 (精确控制)
bars = client.get_security_bars(KLINE_DAILY, MARKET_SH, "600519", start=0, count=800)
more = client.get_security_bars(KLINE_DAILY, MARKET_SH, "600519", start=800, count=800)
```

##### K线种类 (category)

| 值 | Python 常量 | 含义 | 支持 fq=0 | 说明 |
|:--:|------|------|:--:|------|
| 0 | `KLINE_5MIN` | 5 分钟线 | ✅ | — |
| 1 | `KLINE_15MIN` | 15 分钟线 | ✅ | — |
| 2 | `KLINE_30MIN` | 30 分钟线 | ✅ | — |
| 3 | `KLINE_1HOUR` | 60 分钟线 | ✅ | — |
| 4 | `KLINE_DAILY` | 日 K 线 (完整) | ✅ | **推荐用于日线**。vol=股数 |
| 5 | `KLINE_WEEKLY` | 周 K 线 | ✅ | — |
| 6 | `KLINE_MONTHLY` | 月 K 线 | ✅ | — |
| 7 | `KLINE_EXHQ_1MIN` | 扩展 1 分钟线 | ✅ | — |
| 8 | `KLINE_1MIN` | 1 分钟线 | ✅ | — |
| 9 | `KLINE_RI_K` | 日 K 线 (精简) | ❌ | 不支持 fq=0。vol=**手数** (÷100)。含分钟字段 |
| 10 | `KLINE_3MONTH` | 季 K 线 | ✅ | — |
| 11 | `KLINE_YEARLY` | 年 K 线 | ✅ | — |

> **推荐**: 日线使用 `category=4` (`KLINE_DAILY`) 而非 `category=9`，因为前者支持未复权 (`fq=0`) 查询。

##### 复权类型 (fq)

| 值 | Python 常量 | 名称 | 行为 |
|:--:|------|------|------|
| 0 | `FQ_NONE` | 未复权 | 返回交易所原始价格。除权除息日会出现价格跳空。仅 `category=4` 支持。 |
| 1 | `FQ_QFQ` | **前复权** (默认) | 历史价格按后续除权因子向下调整，最新价保持不变。适合技术分析和回测。 |
| 2 | `FQ_HFQ` | 后复权 | 近期价格按历史除权因子向上调整，首日价保持不变。 |

```python
# 前复权 (默认)
bars = client.get_security_bars(4, 1, "600519", 0, 100, fq=1)

# 未复权 (原始数据)
raw = client.get_security_bars(4, 1, "600519", 0, 100, fq=0)

# 后复权
hfq = client.get_security_bars(4, 1, "600519", 0, 100, fq=2)
```

> **实现**: `fq>0` 时客户端自动拉取 `get_xdxr_info()` 获取除权除息历史，调用 `src/protocol/adjuster.rs` 在客户端侧完成复权计算。`fq=0` 不触发除权查询，零额外开销。详参 [复权算法文档](../ADJUSTER_ALGORITHM.md)。

##### 返回字段

**个股 K 线 (SecurityBar)**:

| 字段 (dict key) | 类型 | 说明 |
|------|:--:|------|
| `open` | float | 开盘价 |
| `close` | float | 收盘价 |
| `high` | float | 最高价 |
| `low` | float | 最低价 |
| `vol` | float | 成交量 (股数, category=9 时为手数) |
| `amount` | float | 成交额 (元) |
| `year` | int | 年份 |
| `month` | int | 月份 |
| `day` | int | 日 |
| `hour` | int | 时 (日线为 0) |
| `minute` | int | 分 (日线为 0) |
| `datetime` | str | ISO 格式日期时间 "YYYY-MM-DD HH:MM" |

**Tuple 顺序**: `(open, close, high, low, vol, amount, year, month, day, hour, minute, datetime)`

**DataFrame 列名**: `open`, `close`, `high`, `low`, `vol`, `amount`, `year`, `month`, `day`, `hour`, `minute`, `datetime`

**指数 K 线 (IndexBar)**: 在 SecurityBar 基础上增加 (vol 单位同样分 cat=4 股数 / cat=9 手数):

| 字段 (dict key) | 类型 | 说明 |
|------|:--:|------|
| `up_count` | int | 上涨家数 |
| `down_count` | int | 下跌家数 |

##### 批量获取示例

```python
from tdxrs import TdxHqClient
from tdxrs.constants import MARKET_SH, MARKET_SZ, KLINE_DAILY, KLINE_WEEKLY, FQ_QFQ, FQ_NONE

client = TdxHqClient()
client.connect_to_any()

# 1. 贵州茅台最近 200 条前复权日K线 → DataFrame
df = client.get_security_bars_dataframe(KLINE_DAILY, MARKET_SH, "600519", 0, 200, FQ_QFQ)

# 2. 多只股票批量获取 (tuple 高性能模式)
stocks = [(MARKET_SH, "600519"), (MARKET_SZ, "000858"), (MARKET_SZ, "300750")]
for mkt, code in stocks:
    bars = client.get_security_bars_tuples(KLINE_DAILY, mkt, code, 0, 100)
    print(f"{code}: {len(bars)} bars")

# 3. 指数周K线 + 全部历史 (指数不复权，fq 被忽略)
index_bars = client.get_index_bars_all(KLINE_WEEKLY, MARKET_SH, "000001", count=5000)

# 4. 未复权原始数据 (用于除权研究)
raw = client.get_security_bars(KLINE_DAILY, MARKET_SH, "600519", 0, 100, FQ_NONE)

# 5. 准确获取最后 N 条 — 用 start=0
latest_50 = client.get_security_bars(KLINE_DAILY, MARKET_SH, "600519", start=0, count=50)

client.disconnect()
```

##### 获取更多历史数据

```python
# count > 800 时使用 _all 方法
all_daily = client.get_security_bars_all(KLINE_DAILY, MARKET_SH, "600519", count=5000)

# start > 0 跳过最近 N 条，获取更早数据
older = client.get_security_bars(KLINE_DAILY, MARKET_SH, "600519", start=800, count=800)
```

---

#### 数据获取 — 实时行情

```python
get_security_quotes(stocks: list[(market, code)]) -> list[dict]
get_security_quotes_dataframe(stocks) -> DataFrame
get_security_quotes_tuples(stocks) -> list[tuple]
```

| 参数 | 类型 | 说明 |
|------|------|------|
| `stocks` | `list[(market, code)]` | 股票列表，**单次上限 60 只**，超出自动截断 |

**返回字段 (dict)**: `market`, `code`, `price`, `last_close`, `open`, `high`, `low`, `vol`, `cur_vol`, `amount`, `s_vol`, `b_vol`, `bid1`～`bid5`, `ask1`～`ask5`, `bid_vol1`～`bid_vol5`, `ask_vol1`～`ask_vol5`, `servertime`

> 行情无缓存，每次实时查询。**单次最多查询 60 只**，超出部分自动截断。如需查询更多，请自行分组调用后合并结果。
>
> **自动重试 (v0.6.7)**: 某些服务器不支持实时行情，首次查询返回空时自动切换服务器重试。最多重试 3 次。

```python
# 单只
q = client.get_security_quotes([(1, "600519")])
print(q[0]["price"], q[0]["bid1"])

# 批量 (≤60 只)
quotes = client.get_security_quotes([
    (1, "600519"), (0, "000858"), (0, "300750")
])
for q in quotes:
    print(f"{q['code']}: {q['price']} (昨收{q['last_close']})")

# 超过 60 只需分组
def batch_quotes(client, stocks, batch_size=60):
    results = []
    for i in range(0, len(stocks), batch_size):
        results.extend(client.get_security_quotes(stocks[i:i+batch_size]))
    return results

# DataFrame
df = client.get_security_quotes_dataframe([
    (1, "600519"), (0, "000858")
])
```

---

#### 数据获取 — 证券信息

```python
get_security_list(market, start=0) -> list[dict]
get_security_count(market) -> int
```

**缓存**: `start=0` 时 `get_security_list` 和 `get_security_count` 使用 TTL 缓存 (默认 30s)。非 0 偏移不缓存。

**get_security_list 返回字段**: `code`, `name`, `pre_close`, `volunit`, `decimal_point`

```python
# 上海市场证券数量
count = client.get_security_count(MARKET_SH)

# 深圳市场证券列表 (第一页)
stocks = client.get_security_list(MARKET_SZ, start=0)

# 翻页
page2 = client.get_security_list(MARKET_SZ, start=1000)
```

---

#### 数据获取 — 分时与逐笔

```python
# 分时数据
get_minute_time_data(market, code) -> list[dict]             # 当日
get_history_minute_time_data(market, code, date) -> list[dict]  # 历史

# 集合竞价（09:15~09:25 开盘竞价 + 14:57~15:00 收盘竞价，按时间升序）
get_call_auction(market, code) -> list[dict]   # time / price / match_vol / unmatched_vol / side

# 逐笔成交
get_transaction_data(market, code, start=0, count=2000) -> list[dict]             # 当日
get_history_transaction_data(market, code, start, count, date) -> list[dict]    # 历史
```

| 参数 | 类型 | 说明 |
|------|------|------|
| `market` | int | 市场代码 |
| `code` | str | 股票代码 |
| `start` | int | 起始位移 |
| `count` | int | 请求数量，上限 `MAX_TRANSACTION_COUNT`(2000) |
| `date` | int | 日期，格式 `YYYYMMDD` (如 `20260421`) |

**分时返回**: `[{"time": "HH:MM", "price": float, "avg_price": float, "vol": float}, ...]`

> `time` 字段基于数据索引推算，标准分布为:
> - 上午 120 点: index 0=`09:31` ~ index 119=`11:30`（不含集合竞价 09:30）
> - 下午 120 点: index 120=`13:01` ~ index 239=`15:00`（不含集合竞价 13:00）
> - 标准交易日共 240 点，上下开盘集合竞价视为无有效数据点
> - 数据默认按时间倒序排列（最新记录在前）
> - `vol` 单位为**手**（1手=100股）

**逐笔返回**: `[{"time": "HH:MM", "price": float, "vol": float, "num": int, "buyorsell": int, "reserved": int}, ...]`

| 字段 | 类型 | 说明 |
|------|------|------|
| `time` | str | 时间 `HH:MM`（协议仅存储分钟级精度） |
| `price` | float | 成交价格 |
| `vol` | float | 成交量（**手**，1手=100股） |
| `num` | int | 成交笔数 |
| `buyorsell` | int | 买卖方向: 0=买, 1=卖, 2=中性 |
| `reserved` | int | 保留字段（股票数据中始终为0） |

---

#### 数据获取 — 财务与除权

```python
# 实时财务 (34 字段)
get_finance_info(market, code) -> dict
get_finance_info_dataframe(stocks: list[(market, code)]) -> DataFrame

# 除权除息
get_xdxr_info(market, code) -> list[dict]

# 板块数据
get_and_parse_block_info(block_file: str) -> list[dict]
```

**场代码**:
| 值 | 常量 | 说明 |
|:--:|------|------|
| 0 | `MARKET_SZ` | 深圳 |
| 1 | `MARKET_SH` | 上海 |
| 2 | `MARKET_BJ` | 北京 |

**`get_finance_info` 返回字段** (34 项, TDX 原始值, 未做单位转换):

| 分类 | 字段 | 含义 | 典型数量级 |
|------|------|------|:--:|
| 股本 | `zongguben` | 总股本 | ~10⁵ (万股) |
| | `liutongguben` | 流通股本 | ~10⁵ (万股) |
| | `guojiagu` / `farengu` / `bgu` / `hgu` | 国家股/法人股/B股/H股 | ~10⁴-10⁵ |
| 资产 | `zongzichan` / `jingzichan` | 总资产 / 净资产 | ~10⁸ (万元) |
| | `liudongzichan` / `gudingzichan` / `wuxingzichan` | 流动/固定/无形资产 | ~10⁷-10⁸ |
| 负债 | `liudongfuzhai` / `changqifuzhai` | 流动/长期负债 | ~10⁷-10⁸ |
| 利润 | `zhuyingshouru` / `zhuyinglirun` | 主营收入 / 主营利润 | ~10⁷-10⁸ |
| | `yingyelirun` / `lirunzonghe` / `jinglirun` | 营业利润 / 利润总额 / 净利润 | ~10⁷ |
| 现金流 | `jingyingxianjinliu` / `zongxianjinliu` | 经营/总现金流 | ~10⁷-10⁸ |
| 其他 | `meigujingzichan` | 每股净资产 (元) | ~10¹-10² |
| | `gudongrenshu` | 股东人数 | ~10⁵ (户) |
| | `updated_date` / `ipo_date` | 更新日期 / 上市日期 | YYYYMMDD |
| | `province` / `industry` | 省份 / 行业代码 | — |

> **v0.5.1 变更**: 所有财务字段现返回 TDX **原始值**，不再做 ×10000 单位转换。不同字段单位可能不同（元/万元/千元），由用户根据实际数据自行判断。45 个英文命名指标请使用 `TdxFinanceClient.get_finance_indicators()` (Rust API)。

### 命名财务指标 (Rust TdxFinanceClient)

从 gpcw 数据中提取 45 个核心财务指标，英文命名，TDX 原始值。

```rust
use tdxrs::net::finance_client::TdxFinanceClient;

let fc = TdxFinanceClient::new("120.76.152.87", 7709, None);
// 获取命名指标 (英文 key)
let ind = fc.get_finance_indicators("gpcw20260331.dat", filesize, "600519")?;
// ind["eps"], ind["roe_weighted"], ind["total_assets"], ...

// 获取带中文标签 (适合校验)
let labeled = fc.get_finance_indicators_labeled("gpcw20260331.dat", filesize, "600519")?;
// (en_name, zh_name, value)
```

**45 个核心指标**:

| 分类 | 字段 (en) | 中文 | gpcw idx |
|------|------|------|:--:|
| 每股 | `eps` | 基本每股收益 | 1 |
| | `bvps` | 每股净资产 | 4 |
| | `ocf_ps` | 每股经营现金流 | 7 |
| 盈利 | `roe_weighted` | 加权净资产收益率 | 281 |
| | `roe_diluted` | 净资产收益率(摊薄) | 6 |
| | `gross_margin` | 销售毛利率 | 202 |
| | `net_margin` | 销售净利率 | 199 |
| | `roa` | 总资产净利率 | 200 |
| | `ebit` / `ebitda` | 息税前利润 / EBITDA | 207/208 |
| 成长 | `revenue_yoy` | 营收增长率 | 183 |
| | `net_profit_yoy` | 净利增长率 | 184 |
| 偿债 | `current_ratio` / `quick_ratio` | 流动/速动比率 | 159/160 |
| | `debt_ratio` | 资产负债率 | 210 |
| 营运 | `asset_turnover` | 总资产周转率 | 175 |
| | `inventory_turnover` | 存货周转率 | 173 |
| 规模 | `total_assets` | 资产总计 | 40 |
| | `revenue` | 营业收入 | 74 |
| | `net_profit_is` | 净利润(利润表) | 95 |
| | `parent_net_profit` | 归母净利润 | 96 |
| | `operating_cf` | 经营活动CF | 107 |
| 其他 | `total_shares` | 总股本 | 238 |
| | `employees` | 员工人数 | 320 |
| | `revenue_ttm` | 营收TTM | 319 |

> 完整 45 字段定义见 `src/protocol/finance_fields.rs::INDICATORS`。部分银行/非银字段可能为 0。未对齐字段（197/134/232）已标注在源码中。

---

### AsyncTdxHqClient

异步行情客户端。底层使用通道化连接池 (channel-based pool) 实现真正的并发请求，内部持有独立 tokio Runtime。

```python
from tdxrs import AsyncTdxHqClient
client = AsyncTdxHqClient()
```

**与 TdxHqClient 的核心差异**:

| 维度 | TdxHqClient | AsyncTdxHqClient |
|------|-------------|------------------|
| 连接模型 | 共享连接池 (Mutex) | 通道化连接池 (每连接独立 task) |
| 并发方式 | 连接级串行，池级并发 | 请求级并发 (tokio channel) |
| 心跳 | std::thread | tokio::spawn (async) |
| 限流 | std::sync::Mutex | tokio::sync::Mutex |
| Python 调用 | 同步阻塞 | 同步阻塞 (内部 block_on) |
| GIL 释放 | 否 | 否 (与 TdxHqClient 一致) |
| 空响应重试 | ✅ K线 + 行情 | ✅ K线 + 行情 (v0.6.7) |
| 适用场景 | 通用，首选 | 高并发批量请求、tokio 生态集成 |

> **Python 调用方式与 TdxHqClient 完全一致** — 所有方法同步返回，内部通过 `tokio::runtime::Runtime::block_on()` 执行异步操作。

**适用场景**:
- 批量并发请求 (多只股票同时查询，连接间真正并行)
- tokio 生态集成 (Rust 侧可直接 `await`)
- 需要更细粒度连接池控制的场景

#### 连接管理

| 方法 | 返回 | 说明 |
|------|------|------|
| `AsyncTdxHqClient()` | | 默认构造 (4 连接池) |
| `AsyncTdxHqClient.with_pool_size(n)` | | 指定连接池大小 (静态方法) |
| `connect(ip, port, timeout=None)` | `bool` | 连接到指定服务器 |
| `connect_to_any(timeout=None)` | `bool` | 连接到任意可用服务器 |
| `disconnect()` | `None` | 断开所有连接并停止心跳 |
| `connection_count()` | `int` | 当前连接数 |
| `is_connected()` | `bool` | 连接是否存活 |

```python
from tdxrs import AsyncTdxHqClient

# 默认 4 连接
client = AsyncTdxHqClient()
client.connect("180.153.18.170", 7709)

# 或指定连接数
client = AsyncTdxHqClient.with_pool_size(8)
client.connect_to_any()
```

#### 配置

| 方法 | 说明 |
|------|------|
| `set_rate_limit(rps)` | 设置限流 RPS (0=禁用, 上限 200) |
| `set_phase(phase)` | 设置交易阶段: `"trading"` / `"prepost"` / `"closed"` |
| `auto_detect_phase()` | 自动检测并设置限流，返回阶段名称 |

**限流规则** (与 TdxHqClient 一致):

| 阶段 | RPS | 触发条件 |
|------|-----|---------|
| Trading (盘中) | 15 | 工作日 9:30-15:00 |
| PrePost (盘前盘后) | 30 | 工作日其他时段 |
| Closed (休市) | 60 | 周末 / 节假日 |

> 每个连接独立限流。4 连接池实际吞吐 = RPS × 4。

#### 数据获取 — K线

API 签名与 TdxHqClient 完全一致，支持三种输出格式:

| 方法 | 返回 | 说明 |
|------|------|------|
| `get_security_bars(cat, mkt, code, start, count, fq)` | `list[dict]` | 个股K线 |
| `get_security_bars_tuples(...)` | `list[tuple]` | 高性能 tuple 模式 |
| `get_security_bars_dataframe(...)` | `DataFrame` | pandas DataFrame |
| `get_security_bars_all(cat, mkt, code, count, fq)` | `list[dict]` | 自动分页 |
| `get_index_bars(cat, mkt, code, start, count, fq)` | `list[dict]` | 指数K线 |
| `get_index_bars_tuples(...)` | `list[tuple]` | 指数 tuple 模式 |
| `get_index_bars_dataframe(...)` | `DataFrame` | 指数 DataFrame |
| `get_index_bars_all(cat, mkt, code, count, fq)` | `list[dict]` | 指数自动分页 |

#### 数据获取 — 实时行情

| 方法 | 返回 | 说明 |
|------|------|------|
| `get_security_quotes(stocks)` | `list[dict]` | 批量实时行情 (五档)，**单次上限 60 只** |
| `get_security_quotes_tuples(stocks)` | `list[tuple]` | 高性能 tuple 模式，**单次上限 60 只** |
| `get_security_quotes_dataframe(stocks)` | `DataFrame` | pandas DataFrame，**单次上限 60 只** |

#### 数据获取 — 其他

| 方法 | 返回 | 说明 |
|------|------|------|
| `get_security_count(market)` | `int` | 证券数量 |
| `get_security_list(market, start)` | `list[dict]` | 证券列表 |
| `get_minute_time_data(market, code)` | `list[dict]` | 分时数据 |
| `get_history_minute_time_data(market, code, date)` | `list[dict]` | 历史分时 |
| `get_call_auction(market, code)` | `list[dict]` | 集合竞价逐笔（开盘 09:15-09:25 + 收盘 14:57-15:00） |
| `get_transaction_data(market, code, start, count)` | `list[dict]` | 逐笔成交 |
| `get_history_transaction_data(market, code, start, count, date)` | `list[dict]` | 历史逐笔 |
| `get_finance_info(market, code)` | `dict` | 财务信息 |
| `get_xdxr_info(market, code)` | `list[dict]` | 除权除息 |
| `calc_fq_factors(market, code, start, count)` | `dict` | 复权因子计算 |

#### 复权因子计算 (`calc_fq_factors`)

独立计算复权因子，不修改 K 线数据。用于验证复权精度或导出因子表。

**参数**:
| 参数 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `market` | int | — | 市场代码 (0=深圳, 1=上海) |
| `code` | str | — | 股票代码 |
| `start` | int | 0 | 起始位置 (0=最新) |
| `count` | int | 800 | K 线数量 |

**返回值**:
```python
{
    "factors": [
        {
            "date": 20250626,           # 除权日期 (YYYYMMDD)
            "close_before": 1435.86,    # 前收盘价
            "qfq_factor": 0.980727,     # 前复权因子
            "hfq_factor": 1.019652,     # 后复权因子
            "div_per_share": 2.767,     # 分红 (元/股)
            "bonus_ratio": 0.0,         # 送股比例
            "rights_ratio": 0.0,        # 配股比例
            "rights_price": 0.0,        # 配股价
        },
        ...
    ],
    "cumulative_qfq": 0.117160,   # 累计前复权因子
    "cumulative_hfq": 8.535349,   # 累计后复权因子
}
```

**示例**:
```python
# 计算复权因子
result = client.calc_fq_factors(MARKET_SH, "600519", start=0, count=800)

print(f"累计前复权因子: {result['cumulative_qfq']:.6f}")
print(f"累计后复权因子: {result['cumulative_hfq']:.6f}")

for f in result['factors']:
    print(f"  {f['date']}: QFQ={f['qfq_factor']:.6f}")
```

#### 完整示例

```python
from tdxrs import AsyncTdxHqClient
from tdxrs.constants import (
    MARKET_SH, MARKET_SZ, KLINE_DAILY, KLINE_5MIN,
    FQ_QFQ, FQ_HFQ,
)

# 创建并连接 (4 连接池)
client = AsyncTdxHqClient()
client.connect("180.153.18.170", 7709)

# 自动检测交易阶段
phase = client.auto_detect_phase()
print(f"当前阶段: {phase}")  # "closed"

# 个股日K (前复权)
bars = client.get_security_bars(KLINE_DAILY, MARKET_SH, "600519", 0, 100, FQ_QFQ)
# [{'open': 1500.0, 'close': 1510.0, 'high': 1520.0, 'low': 1495.0, ...}, ...]

# 高性能 tuple 模式
tuples = client.get_security_bars_tuples(KLINE_DAILY, MARKET_SH, "600519", 0, 100)
# [(1500.0, 1510.0, 1520.0, 1495.0, 12345.0, 18000000.0, 2026, 6, 20, 0, 0, '2026-06-20'), ...]

# DataFrame 模式
df = client.get_security_bars_dataframe(KLINE_DAILY, MARKET_SH, "600519", 0, 100)

# 批量实时行情
quotes = client.get_security_quotes([(MARKET_SH, "600519"), (MARKET_SZ, "000858")])

# 自动分页获取全部日K
all_bars = client.get_security_bars_all(KLINE_DAILY, MARKET_SH, "600519", 800, FQ_QFQ)

client.disconnect()
```

---

### TdxDirectClient

裸连接客户端。每次 API 调用独立建立 TCP 连接 + 握手，无连接池、无重试、无心跳、无缓存。

```python
from tdxrs import TdxDirectClient
dc = TdxDirectClient(ip, port=7709, timeout=5.0)
```

| 方法 | 说明 |
|------|------|
| `TdxDirectClient(ip, port, timeout)` | 构造，设置服务器地址和超时 |
| `set_server(ip, port)` | 更新服务器地址 |
| `set_timeout(secs)` | 更新超时 |
| `get_security_bars(...)` | 个股K线 (同 TdxHqClient 签名) |
| `get_index_bars(...)` | 指数K线 |
| `get_security_quotes(...)` | 实时行情 |
| `get_security_list(...)` | 证券列表 |
| `get_security_count(...)` | 证券数量 |
| `get_minute_time_data(...)` | 分时 |
| `get_history_minute_time_data(...)` | 历史分时 |
| `get_transaction_data(...)` | 逐笔 |
| `get_history_transaction_data(...)` | 历史逐笔 |
| `get_finance_info(...)` | 财务信息 |
| `get_xdxr_info(...)` | 除权除息 |
| `get_and_parse_block_info(...)` | 板块数据 |

**适用场景**:
- 偶发请求 (<1次/分钟)，无需维护长连接
- 高并发 (多线程独立连接，天然并行，60 用户无退化)
- 不需要重试/缓存/心跳的场景

**与 TdxHqClient 差异**:
- ❌ 无 `_all` 自动分页方法
- ❌ 无 `_dataframe` / `_tuples` 直接输出
- ❌ 无 `__init__()` 无参数构造 (必须指定 IP)
- ❌ 无服务器管理、池状态等

```python
dc = TdxDirectClient("119.147.212.81", 7709, timeout=10.0)
bars = dc.get_security_bars(KLINE_DAILY, MARKET_SH, "600519", 0, 100, FQ_QFQ)
dc.set_server("180.153.18.17", 7709)  # 切换服务器
```

---

### TdxSmartClient (v0.6.7)

智能连接客户端。包装 `TdxHqClient`，增加惰性健康检查和服务器缓存功能。

```python
from tdxrs import TdxSmartClient
client = TdxSmartClient()
```

#### 核心特性

| 特性 | 说明 |
|------|------|
| 快速初始连接 | 仅验证 TCP + 握手，不做 K 线健康检查 |
| 惰性健康检查 | 首次 K 线请求返回空时触发，自动切换服务器 |
| 本地缓存 | `~/.tdxrs/server_cache.json` 记录成功/失败服务器 |
| 黑名单机制 | 连续失败的服务器自动加入黑名单 (24h 过期) |
| 真实K线探测 (v0.6.9) | `probe_and_cache` 拉取一根日K验证数据，握手成功但 K 线为空记为失败 |

#### 方法

| 方法 | 返回 | 说明 |
|------|------|------|
| `connect_to_any(timeout=None)` | `bool` | 连接到任意可用服务器 (优先使用缓存) |
| `get_security_bars(...)` | `list[dict]` | 获取 K 线数据 (带自动重试) |
| `get_security_quotes(...)` | `list[dict]` | 获取实时行情 (带自动重试) |
| `cache_stats()` | `str` | 获取缓存统计信息 |
| `clear_cache()` | — | 清除缓存 |
| `probe_and_cache(timeout)` | `list` | 探测所有服务器并更新缓存 (真实K线验证) |
| `disconnect()` | — | 断开连接 |
| `is_connected()` | `bool` | 是否已连接 |

#### 使用示例

```python
from tdxrs import TdxSmartClient

# 创建智能客户端
client = TdxSmartClient()

# 连接 (自动使用缓存，跳过黑名单服务器; 默认已用 SkipCmd3 握手)
client.connect_to_any(timeout=10.0)

# 获取 K 线数据 (如果返回空，自动切换服务器重试)
bars = client.get_security_bars(4, 1, '600519', 0, 10, 0)

# 查看缓存状态
print(client.cache_stats())

# 探测所有服务器并更新缓存 (真实K线验证: 拉一根日K，数据非空才算可用)
results = client.probe_and_cache(3.0)
for ip, port, name, latency in results:
    print(f"{name}: {latency}ms")

# 清除缓存
client.clear_cache()

# 断开连接
client.disconnect()
```

#### 与 TdxHqClient 对比

| 维度 | TdxHqClient | TdxSmartClient |
|------|-------------|----------------|
| 初始连接 | 无健康检查 | 无健康检查 |
| K 线请求 | 返回空时自动重试 (v0.6.7) | 返回空时自动重试 |
| 服务器缓存 | 无 | 本地 JSON 缓存 |
| 自动黑名单 | 无 | 24h 自动过期 |
| 手动黑名单 | ✅ `block_server` | 无 |
| 适用场景 | 通用 | 网络不稳定 |

**适用场景**:
- 网络环境不稳定，部分服务器对当前用户不可用
- 长期运行，需要自动适应服务器状态变化
- 需要快速初始化连接，首次 K 线请求可能需要重试

---

### TdxHqFundClient

基金行情客户端 (扩展模块)。封装 `TdxHqClient`，支持 ETF/LOF/REITs/分级基金等全部基金类型。

```python
from tdxrs import TdxHqFundClient
from tdxrs.constants import MARKET_SH, MARKET_SZ

client = TdxHqFundClient()
client.connect_to_any()

# 获取基金列表 (含类型信息)
funds = client.get_fund_list(MARKET_SH)
# [{'code': '510300', 'fund_type': 'ETF', 'fund_type_zh': '交易型开放式指数基金', ...}, ...]

# 分类基金类型
TdxHqFundClient.classify_fund(MARKET_SH, "510300")  # → "ETF"
TdxHqFundClient.classify_fund(MARKET_SH, "508000")  # → "REITs"
TdxHqFundClient.classify_fund(MARKET_SZ, "162006")  # → "Structured"
```

| 方法 | 返回 | 说明 |
|------|------|------|
| `get_fund_list(market)` | `list[dict]` | 基金列表 (含 fund_type) |
| `get_fund_bars(cat, mkt, code, start, count)` | `list[dict]` | 基金K线 |
| `get_fund_bars_all(cat, mkt, code, count)` | `list[dict]` | 自动分页 |
| `get_fund_quotes(stocks)` | `list[dict]` | 实时行情 (五档) |
| `get_fund_minute_time_data(mkt, code)` | `list[dict]` | 分时数据 |
| `get_fund_transaction_data(mkt, code, start, count)` | `list[dict]` | 逐笔成交 |
| `get_fund_xdxr_info(mkt, code)` | `list[dict]` | 除权除息 |
| `get_fund_finance_info(mkt, code)` | `dict` | 财务信息 |
| `is_fund(market, code)` | `bool` | 静态: 判断基金 |
| `is_etf(market, code)` | `bool` | 静态: 判断ETF |
| `classify_fund(market, code)` | `str` | 静态: 基金类型 |
| `auto_market_code(code)` | `int` | 静态: 自动市场 |

**基金类型 (FundType)**:

| 类型 | 代码前缀 | 说明 |
|------|---------|------|
| ETF | 510/512/513/515/516/159 | 交易型开放式指数基金 |
| LOF | 501/502/160/161 | 上市型开放式基金 |
| REITs | 508 | 不动产投资信托基金 |
| Structured | 162/163/164 | 分级基金 |
| Bond | 511 | 债券ETF |
| OpenEnd | 519 | 传统开放式基金 |

---

### TdxBlockClient

板块专用客户端，内置K线级别限制，禁用分时/逐笔。

```python
from tdxrs import TdxBlockClient

client = TdxBlockClient("58.63.254.191", 7709, 5.0)

# 板块K线 (自动限制 + 空响应自动重试)
bars = client.get_block_bars(4, "880001", 0, 100)  # 日K
bars = client.get_block_bars(0, "881001", 0, 0)    # 5min (默认50条)

# 实时行情 (空响应自动重试)
quotes = client.get_block_quotes(["880001", "881001"])

# 板块列表 (从服务器下载 .dat 文件)
industry = client.get_industry_blocks()   # 行业/筛选板块 (block_fg.dat)
concept = client.get_concept_blocks()     # 概念板块 (block_gn.dat)
index = client.get_index_blocks()         # 指数成分 (block_zs.dat)
custom = client.get_block_list("block.dat")  # 自定义文件
```

| 方法 | 返回 | 说明 |
|------|------|------|
| `get_block_bars(category, code, start, count)` | `list[dict]` | 板块K线 (带限制) |
| `get_block_quotes(codes)` | `list[dict]` | 板块实时行情 |
| `get_block_list(block_file)` | `list[dict]` | 下载并解析指定 `.dat` 板块文件 |
| `get_industry_blocks()` | `list[dict]` | 获取行业/筛选板块 (block_fg.dat) |
| `get_concept_blocks()` | `list[dict]` | 获取概念板块 (block_gn.dat) |
| `get_index_blocks()` | `list[dict]` | 获取指数成分 (block_zs.dat) |

**K线限制**:

| 级别 | 默认 | 上限 | 说明 |
|------|------|------|------|
| 日/周/月 | 100 | 800 | 无限制 |
| 60min | 200 | 800 | |
| 30min/15min/5min | 50 | 200 | |
| 1min | — | — | **禁用** |

**空响应自动重试**: `get_block_bars` (日K及以上) 和 `get_block_quotes` 返回空数据时，自动遍历 PRIMARY_SERVERS 切换服务器重试，最多 5 次。用户指定的服务器优先尝试，不在 PRIMARY_SERVERS 中的会插入到候选列表首位。

**禁用接口**: 分时数据、逐笔成交（板块数据无意义）

---

### TdxF10Client

> ⚠️ 源码编译专用 (`--features f10`)，pip 包不包含此模块。

F10 公司资料客户端。独立连接，不占用共享连接池。

```python
# 需从源码编译: maturin develop --release --features f10
from tdxrs.pro import TdxF10Client

client = TdxF10Client("180.153.18.170", 7709)
```

完整 API 详见 [F10 模块文档](F10.md)。

| 方法 | 返回 | 说明 |
|------|------|------|
| `get_category(market, code)` | `list[dict]` | 分类列表 |
| `get_content(market, code, category_dict)` | `str` | 指定分类内容 |
| `get_all_contents(market, code)` | `dict[str, str]` | 全部内容 |
| `get_all_data(market, code)` | `dict` | 结构化全量数据 |
| `parse_f10(text)` | `dict` | 静态: 解析 F10 文本 |
| `extract_basic_info(text)` | `dict` | 静态: 提取基本资料 |
| `is_valid_code(code)` | `bool` | 静态: 验证代码 |
| `auto_market_code(code)` | `int` | 静态: 自动市场 |

---

## 常量 (`tdxrs.constants`)

```python
from tdxrs.constants import (
    # ═══ 市场代码 ═══
    MARKET_SZ,   # 0 — 深圳
    MARKET_SH,   # 1 — 上海
    MARKET_BJ,   # 2 — 北京

    # ═══ K线种类 (category) ═══
    KLINE_5MIN,      # 0  — 5分钟线
    KLINE_15MIN,     # 1  — 15分钟线
    KLINE_30MIN,     # 2  — 30分钟线
    KLINE_1HOUR,     # 3  — 60分钟线
    KLINE_DAILY,     # 4  — 日K线 (完整, 推荐)
    KLINE_WEEKLY,    # 5  — 周K线
    KLINE_MONTHLY,   # 6  — 月K线
    KLINE_EXHQ_1MIN, # 7  — 扩展1分钟线
    KLINE_1MIN,      # 8  — 1分钟线
    KLINE_RI_K,      # 9  — 日K线 (精简, 不支持fq=0)
    KLINE_3MONTH,    # 10 — 季K线
    KLINE_YEARLY,    # 11 — 年K线

    # ═══ 复权类型 (fq) ═══
    FQ_NONE,  # 0 — 未复权
    FQ_QFQ,   # 1 — 前复权 (默认)
    FQ_HFQ,   # 2 — 后复权

    # ═══ 限制 ═══
    MAX_KLINE_COUNT,        # 800  — 单次K线最大条数
    MAX_TRANSACTION_COUNT,  # 2000 — 单次逐笔最大条数

    # ═══ 默认配置 ═══
    DEFAULT_PORT,          # 7709 — TDX 行情端口
    DEFAULT_POOL_SIZE,     # 5    — 连接池大小
    FQ_PRICE_PRECISION,    # 3    — 复权小数位数
)
```

---

## 输出格式对照

三种输出格式各有适用场景：

| 格式 | 方法后缀 | 速度 | 内存 | 适用 |
|------|:------:|:----:|:----:|------|
| **dict** | (默认) | 基准 | 中等 | 调试、少量数据 |
| **tuple** | `_tuples()` | **快 40-60%** | 低 | 遍历、批量处理 |
| **DataFrame** | `_dataframe()` | 慢 | 高 | 数据分析、回测 |

```python
# dict 模式 — 字段名访问
for bar in client.get_security_bars(4, 1, "600519", 0, 100):
    print(bar["datetime"], bar["close"])

# tuple 模式 — 位置访问, 更快
for t in client.get_security_bars_tuples(4, 1, "600519", 0, 100):
    open_, close, high, low, vol, amount, y, m, d, h, mi, dt = t

# DataFrame — pandas 分析
df = client.get_security_bars_dataframe(4, 1, "600519", 0, 500)
df["ma20"] = df["close"].rolling(20).mean()
```

---

## 最佳实践

### 客户端选择

| 使用场景 | 推荐客户端 | 理由 |
|----------|:--------:|------|
| 单个脚本顺序拉取数千条 K 线 | `TdxHqClient` | 连接复用免握手，4× 快于裸连；`_all` 自动分页 |
| 回测引擎，多线程并发跑不同品种 | `TdxDirectClient` | 每线程独立连接天然并行，60 线程 12× 快于池 |
| Web 服务，单请求单响应 | `TdxHqClient` | 连接复用 + 重试兜底 + 缓存减少网络往返 |
| 定时任务，每 N 分钟查一次行情 | `TdxHqClient` | 心跳保活，免去每次建连 200ms 开销 |
| 一次性查询，用完即弃 | `TdxDirectClient` | 无需管理连接生命周期 |
| 下载 gpcw 历史财务文件 | `TdxFinanceClient` | 独立超时 15s + 磁盘缓存，不与行情连接争抢 |

> 决策要点: 请求是**顺序的**还是**同时并发的**？顺序选池，并发选裸连。需要心跳/重试/缓存/分页 → 池。偶发 → 裸连。
> 详见 [性能基准](BENCHMARKS.md)。

### 高频 K 线调用建议

**1. 重用客户端实例，避免反复 `connect`**

```python
# ✅ 正确: 初始化一次，复用
client = TdxHqClient()
client.connect_to_any()
for code in stock_list:
    bars = client.get_security_bars(4, mkt, code, 0, 100)

# ❌ 错误: 每次循环重新连接
for code in stock_list:
    c = TdxHqClient()
    c.connect_to_any()
    bars = c.get_security_bars(...)
    c.disconnect()
```

**2. 批量请求使用 `_tuples()` 模式**

```python
# ✅ 典型回测场景 — 省 40-60% 时间
for code in watchlist:
    bars = client.get_security_bars_tuples(KLINE_DAILY, mkt, code, 0, 500)
    # bars 是 list[tuple], tuple 解构即用
```

**3. 需要分析时用 `_dataframe()`**

```python
# ✅ 分析场景 — 直出 DataFrame，免手动构造
df = client.get_security_bars_dataframe(KLINE_DAILY, MARKET_SH, "600519", 0, 500)
df["returns"] = df["close"].pct_change()
df["ma20"] = df["close"].rolling(20).mean()
```

**4. 选对 `category`，避免踩坑**

```python
# ✅ 日线 — 用 category=4, 支持未复权
bars = client.get_security_bars(KLINE_DAILY, mkt, code, 0, 800, FQ_QFQ)

# ❌ 日线精简 — category=9, 不支持 fq=0
# 当需要对比原始数据时此选项不可用
```

**5. 需要复权和原始数据对比时**

```python
# 一次获取，两份数据
adjusted = client.get_security_bars(KLINE_DAILY, mkt, code, 0, 800, FQ_QFQ)
raw = client.get_security_bars(KLINE_DAILY, mkt, code, 0, 800, FQ_NONE)
# 可用于验证除权因子: adjusted.close / raw.close = cumulative_factor
```

**6. 多股票行情 — 一次调用批量获取**

```python
# ✅ 批量: 一次网络往返取全部
quotes = client.get_security_quotes([
    (MARKET_SH, "600519"),
    (MARKET_SZ, "000858"),
    (MARKET_SZ, "300750"),
])

# ❌ 逐个: 3 次网络往返
for mkt, code in stocks:
    q = client.get_security_quotes([(mkt, code)])
```

**7. 翻页获取历史数据**

```python
# 方法 A: 自动分页 (简单)
all_bars = client.get_security_bars_all(KLINE_DAILY, mkt, code, count=3000)

# 方法 B: 手动翻页 (精确控制, 可中断)
bars = client.get_security_bars(KLINE_DAILY, mkt, code, start=0, count=800)
older = client.get_security_bars(KLINE_DAILY, mkt, code, start=800, count=800)
oldest = client.get_security_bars(KLINE_DAILY, mkt, code, start=1600, count=800)
```

### 异常处理

```python
from tdxrs import TdxHqClient

client = TdxHqClient()
client.set_auto_retry(False)  # 生产环境关闭内置重试

try:
    client.connect_to_any(timeout=5.0)
except ValueError as e:
    print(f"所有服务器不通: {e}")
    # fallback: 重试或告警
else:
    try:
        bars = client.get_security_bars(KLINE_DAILY, MARKET_SH, "600519", 0, 100)
    except ValueError as e:
        print(f"请求失败: {e}")
        # 可自行重试
finally:
    client.disconnect()
```

### 生产环境配置建议

```python
client = TdxHqClient()
client.set_auto_retry(False)       # 关闭内置重试, 由上层控制
client.set_cache_ttl(120)          # 证券列表缓存延长到 2 分钟
client.set_connect_timeout(3.0)    # 缩短连接超时, 快速失败
client.connect_to_any(timeout=5.0)
```

---

## 批量下载器 (`tdxrs.downloader`)

多服务器分发 + 自动翻页 + 增量更新 + 断点续传。

```python
from tdxrs.downloader import Downloader
```

### Downloader

| 参数 | 类型 | 默认 | 说明 |
|------|------|:--:|------|
| `data_dir` | str | `"./data"` | 数据存储根目录 (支持 `~` 展开) |
| `servers` | list[tuple] | 内置 5 台 | 服务器列表 `[(名称, IP, 端口), ...]` |
| `rate_limit` | int | 15 | 每服务器每秒请求数 |
| `format` | str | `"tdx"` | 输出格式: `"tdx"` / `"csv"` / `"parquet"` |
| `fq` | int | 0 | 复权类型: 0=不复权, 1=前复权, 2=后复权 |

| 方法 | 说明 |
|------|------|
| `run(markets, categories, codes)` | 全量下载 K线 |
| `update(markets, categories)` | 增量更新 (仅下载新数据) |
| `download_minute(dates, codes, markets)` | 按日下载分时数据 (codes 必填) |
| `download_ticks(dates, codes, markets)` | 按日下载逐笔成交 (codes 必填) |
| `run_xdxr(markets, codes)` | 下载除权除息数据 |
| `progress()` | 返回统计 `{"done": int, "skipped": int, "failed": int}` |

### 按日下载 (分时 / 逐笔)

分时和逐笔数据支持按日期下载，使用协议原生日期参数，无需计算交易日历。

```python
dl = Downloader(data_dir="./data")

# 分时数据: 单日多只
dl.download_minute("2026-06-25", codes=["600519", "000858"])

# 分时数据: 多日多只
dl.download_minute(["2026-06-25", "2026-06-24"], codes=["600519"])

# 逐笔成交: 自动翻页 (2000条/页)
dl.download_ticks(20260625, codes=["600519"])
```

| 参数 | 类型 | 说明 |
|------|------|------|
| `dates` | str/int/list | 日期: `"2026-06-25"`、`20260625`、或列表 |
| `codes` | list[str] | **必填**，股票代码列表 |
| `markets` | list[str] | `["sh", "sz"]` (默认) |

> **注意**: `codes` 为必填参数，不支持全市场模式。分时/逐笔为单品种接口，每只股票独立请求。

### 输出格式

| format | 扩展名 | DailyBarReader 可读 | 说明 |
|:------:|:------:|:---:|------|
| `"tdx"` | `.day` | ✅ | 默认，与通达信格式兼容 |
| `"csv"` | `.csv` | ❌ | 标准 CSV |
| `"parquet"` | `.parquet` | ❌ | 需要 pyarrow |

### 数据目录结构

```
data/
├── .tdxrs_meta/
│   ├── checkpoint.json     # 断点续传进度
│   └── last_sync.json      # 增量同步记录
├── sh/
│   ├── daily/600519.day
│   ├── weekly/600519.day
│   ├── minute/600519_20260625.csv   # 分时数据
│   └── ticks/600519_20260625.csv    # 逐笔成交
└── sz/
    └── daily/000858.day
```

---

## 错误码体系

所有错误码按模块分段，便于识别和处理：

| 范围 | 模块 | 说明 |
|------|------|------|
| 1000-1099 | 通用 | 参数校验、输入错误 |
| 1100-1199 | 代码分类 | 股票/指数/板块/债券/基金 |
| 1200-1299 | 限流 | 请求频率限制 |
| 2000-2099 | 连接 | 网络连接错误 |
| 2100-2199 | 协议 | TDX 协议错误 |
| 3000-3099 | 解析 | 数据解析错误 |
| 4000-4099 | 文件 | 本地文件错误 |

### 常用错误码

| 错误码 | 常量 | 说明 |
|:------:|------|------|
| 1101 | `ERR_BLOCK_CODE_IN_GENERAL_CLIENT` | 板块代码在通用客户端被拒绝 |
| 1201 | `ERR_RATE_LIMIT_EXCEEDED` | 通用请求限流 |
| 1202 | `ERR_RATE_LIMIT_DAILY_EXCEEDED` | 日K级别限流 |
| 1203 | `ERR_RATE_LIMIT_MINUTE_EXCEEDED` | 分时限流 (不可禁用) |
| 1204 | `ERR_BLOCK_KLINE_CATEGORY_NOT_ALLOWED` | 板块K线级别不支持 |
| 2001 | `ERR_CONNECTION_FAILED` | 连接失败 |
| 2002 | `ERR_CONNECTION_TIMEOUT` | 连接超时 |
| 3001 | `ERR_INVALID_DATE` | 日期格式无效 |
| 3002 | `ERR_DATE_OUT_OF_RANGE` | 日期超出范围 |

### 错误信息格式

```
[E1101] block code (88xxxx) not allowed in general client, use TdxBlockClient: code=880001
```

格式: `[E错误码] 英文描述: 具体信息`

Python 端可通过异常消息获取错误码：

```python
try:
    bars = client.get_security_bars(KLINE_DAILY, MARKET_SH, "880001", 0, 100)
except ValueError as e:
    if "[E1101]" in str(e):
        print("板块代码请使用 TdxBlockClient")
```

---

## 注意事项

1. **K线种类选择**: 日线推荐 `category=4` (KLINE_DAILY) 而非 `category=9` (KLINE_RI_K)，前者支持未复权查询。
2. **复权精度**: 价格四舍五入到 `FQ_PRICE_PRECISION`(3) 位小数，成交量不调整。
3. **分页上限**: 单次 `count > 800` 请求会被截断，使用 `_all` 方法自动分页。
4. **连接管理**: 长时间不用连接池会自动心跳保活 (10s)，生产环境建议关闭 `set_auto_retry(False)` 用上层重试逻辑。
5. **并发**: 高并发场景推荐 `TdxDirectClient` (每线程独立连接)，连接池在高并发下退化严重。
6. **限流**: 分时限流固定 10 req/s 不可禁用；全局限流上限 200 req/s。
7. **批量下载**: 下载器默认输出 `.day` 格式，可被 `DailyBarReader` 直接读取；多服务器分发自动限流。

---

## 安装

```bash
pip install maturin
git clone <repo-url> && cd tdxrs
maturin develop --release
```

详细安装说明见 [INSTALL.md](../INSTALL.md)。
