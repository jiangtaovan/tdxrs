# 变更日志

## v0.6.10 (未发布)

### 变更
- **握手默认模式 `Full` → `SkipCmd3`** — 基于 A/B 实测，最大化行情可用率
  - 背景: Issue #15 揭示 cmd3 (0x1899) 被多数服务器当作"引擎版本上报"，不认可即掐行情。
    原默认 `Full` (发 cmd3) 使日K/实时行情等高频接口大面积返空
  - A/B 实测 (2026-10-31, 46台 × 日K/实时行情 × 2模式): Full 供数 **2/72**、
    SkipCmd3 供数 **54/72**；且**无**"Full 可用而 SkipCmd3 不可用"的反例 (`full_only=0`)
  - 18/72 两组均失败为纯连接层不可达 (EX 专用端口/超时)，与握手无关
  - 影响范围: 全部客户端 (`TdxHqClient` / `TdxDirectClient` / `TdxF10Client` /
    `TdxFinanceClient` / `AsyncTdxHqClient`) 默认改为 `SkipCmd3`；
    `HandshakeMode::default()` 与 `from_u8` 无效值回退同步调整
  - 兼容性: 遇要求 cmd3 的服务器时显式 `set_handshake_mode("full")` 即可回退

### 修复
- **发布工作流完善** (`.github/workflows/publish.yml`)
  - 三个构建 job 统一 `needs: release`，Release 创建失败时全部 fail-fast
  - 构建产物 (wheel/sdist) 自动挂载到 GitHub Release 资产
  - CHANGELOG 版本段提取增加 `[^0-9]` 边界，防止版本号前缀误匹配
  - Release Notes fallback 改用 `printf` 正确解释换行

### 文档
- `examples/demo_kline.py` 按当前接口重写 — 新增握手模式与限流配置演示

## v0.6.9 (2026-10-31) — HandshakeMode 握手模式 + 服务器清单扩充 + 真实K线探测

> 注: 原计划以 v0.6.8 发布的改动（握手模式、CLI 修复、板块K线重试等）在版本推进时未拆分提交，
> 随本版本一并发布，故不再单列 v0.6.8。

### 新增
- **`HandshakeMode` 握手模式枚举** — 解决 [Issue #15]: SETUP_CMD3 导致服务器拒绝行情数据
  - 背景: 部分通达信服务器在收到 `SETUP_CMD3` (0x1899) 后，将该包视为"客户端引擎版本上报"，
    版本不被认可时掐掉整个会话的行情权限，K线/行情/逐笔接口一律返回 2 字节 `0x2003`
  - 验证: 8台测试服务器中 6台不发 cmd3 时正常供数 (count=30)，发 cmd3 后全部返回空
  - 模式:
    - `Full` (默认) — cmd1+cmd2+cmd3，保持向后兼容
    - `SkipCmd3` — cmd1+cmd2，解决服务器拒绝行情问题 (v0.6.10 起为默认)
    - `Cmd1Only` — 仅 cmd1
    - `None` — 不发送握手命令
  - Rust API: `client.set_handshake_mode(HandshakeMode::SkipCmd3)`
  - Python API: `client.set_handshake_mode("skip_cmd3")`
  - 支持客户端: `TdxHqClient` / `TdxDirectClient` / `TdxF10Client` / `TdxFinanceClient`
    + `TdxSmartClient` / `TdxBlockClient` / `TdxHqFundClient` 透传 + `AsyncTdxHqClient`
  - Rust 单元测试: 5 个新增测试覆盖模式切换和命令列表

### 变更
- **PRIMARY_SERVERS 重选** — 基于 SkipCmd3 真实K线探测 (600519 日K验证)
  - 历史注记: 2026-09-10 服务端策略变更曾致大面积K线返空，当时应急将安信15/国泰君安9
    提至优先组前两位；本次按实测延迟整体重选
  - 背景: 合并其他通达信接口项目的公开服务器清单 (36台候选) 后逐台实测
  - 结果: 27/36 台在 SkipCmd3 握手下 K线可用 (此前 Full 握手多数被掐行情, Issue #15)
  - 优先组按探测延迟排序重选 (跨IP段 + 跨运营商)，新增:
    - 通达信深圳9 (159.75.55.232, 448ms)
    - MAC专用165 (121.37.207.165, 527ms)
    - 中信短线宝21 (58.67.221.146, 898ms)
    - MAC专用136 (123.60.47.136, 1465ms)
    - 国泰君安13 (117.34.114.20, 1569ms)
    - 华林6 (218.106.92.183, 1636ms)
  - 移出优先组: 杭州电信J3 (3383ms) / 华林7 (3729ms)，降入 ALL_KNOWN
- **ALL_KNOWN_SERVERS 扩充** — 新增 10 台 (来自公开服务器清单)
  - K线可用 8 台: 中信短线宝22/23、MAC专用138、国泰君安上海 等
  - 探测不可达 2 台: 国泰君安江苏/深圳 (保留供其他网络环境重试)

### 改进
- **`TdxSmartClient::probe_and_cache` 升级为真实K线探测** (Issue #15 对齐)
  - 原实现: 仅验证 TCP + 握手成功即记为可用
  - 新实现: 连接成功后拉取一根 600519 日K，数据非空才算可用
  - 握手成功但 K线为空的服务器记为失败并写入缓存 (Issue #15 典型症状)
  - 探测使用客户端当前握手模式，建议先 `set_handshake_mode("skip_cmd3")`

### 修复
- **CLI 日线/分钟线输出字段对调** — 成交量与成交额标签修正
  - 日线: d[5] 是成交额、d[6] 是成交量，原代码标签对调
  - 分钟线: tuple 布局为 (datetime, open, high, low, close, amount, volume, ...)，原代码假设了错误的 7 字段布局导致整体错位
  - 修复: CLI 输出列名与 tuple 索引正确对应
- **`TdxBlockClient` 板块K线返回空数据** — 新增空响应自动重试
  - 问题: 部分服务器 (如海通8、广发13) 握手正常但板块K线返回空数据
  - 原因: `TdxBlockClient` 使用 `TdxDirectClient` (裸连接)，无自动切换能力
  - 修复: `get_block_bars` (category >= 4) 和 `get_block_quotes` 返回空数据时，自动遍历 PRIMARY_SERVERS 切换服务器重试
  - 候选列表: 用户指定服务器 + PRIMARY_SERVERS (去重)，最多重试 5 次
  - 日志: 切换时 `debug`，成功时 `info`，全部失败时 `warn`

### 文档
- SETUP_CMD3 握手问题根因分析与最小复现 (源自社区报告
  [Issue #15](https://github.com/jiangtaovan/tdxrs/issues/15)，结合代码路径与实测整理)

### 社区贡献 (随本版本合入 main 的社区 PR)
- **PR #11** [@gclm](https://github.com/gclm) (2026-09-06): `fix(cli)` —
  修正 parse 日线/分钟线表格的列标注错位
  - 与本地的同项修复独立完成；rebase 合并时校验两侧语义等价
    (成交额=d[5]、成交量=d[6]，与 `py_reader.rs` tuple 构造一致)，统一为单一实现
- **PR #10** [@OctoBored](https://github.com/OctoBored) (2026-08-20): `docs` —
  修复失效的 Star History 图表链接

## v0.6.7 (2026-07-21) — 日K空响应自动重试 + SmartClient + 服务器黑名单

### 新增
- **`TdxSmartClient` 智能连接客户端** — 分层健康检查 + 本地缓存
  - 快速初始连接: 仅验证 TCP + 握手，不做 K 线健康检查
  - 惰性健康检查: 首次 K 线请求返回空时触发，自动切换服务器
  - 本地缓存: `~/.tdxrs/server_cache.json` 记录成功/失败服务器
  - 黑名单机制: 连续失败的服务器自动加入黑名单 (24h 过期)
  - Python 绑定：`client = tdxrs.TdxSmartClient()`
- **`TdxHqClient` 服务器黑名单** — 用户可手动屏蔽特定服务器
  - `block_server(ip, port)` — 将服务器加入黑名单
  - `unblock_server(ip, port)` — 从黑名单移除
  - `blocked_servers()` — 获取黑名单列表
  - `clear_blocked_servers()` — 清空黑名单
  - 黑名单在 `connect_to_any` 和自动重试中生效
  - 适用场景: 用户已知某台服务器在当前网络环境下不可用

### 优化
- **空响应自动重试** — 单台服务器故障不影响数据获取
  - `get_security_bars`、`get_index_bars` (category >= 4)、`get_security_quotes` 返回空数据时，自动切换服务器重试
  - 最多重试 2 次 (共 3 次请求)，遍历 PRIMARY_SERVERS
  - 解决部分服务器 (如海通、广发) 握手正常但 K 线/行情返回空的问题
  - 分钟线 (category < 4) 不重试，避免不必要的开销
  - **同步版和异步版均已支持** (v0.6.7 修复)
- **`AsyncTdxHqClient` 空响应重试修复** — 与同步版行为一致
  - `get_security_bars` 和 `get_index_bars` 新增空响应重试逻辑
  - 新增 `reconnect_to_another_server()` 方法，跳过当前服务器
  - 新增 `last_server` 跟踪，避免反复连接同一服务器
- **`get_security_quotes` 空响应重试** — 实时行情也支持自动切换服务器
  - 某些服务器不支持实时行情，首次查询返回空时自动切换
  - 解决"先查K线再查行情正常，直接查行情返回空"的问题
- **日志级别优化** — 减少重试日志噪音
  - 单次重试尝试: `debug` 级别 (仅 `TDXRS_LOG=debug` 时显示)
  - 重试成功摘要: `info` 级别 (如 `got 5 bars for 600519 after 2 server switch(es)`)
  - 全部重试失败: `warn` 级别 (如 `all 3 attempts returned empty K-line for 600519`)
  - 默认模式 (`TDXRS_LOG=warn`) 下不会显示重试消息
- **连接池复用修复** — 修复心跳线程引用失效问题
  - `connect_internal` 不再创建新连接池，改为复用现有池 (关闭旧连接 + 推入新连接)
  - 解决: 心跳线程持有旧池引用 → 检测失败 → 错误标记 `connected=false`
  - 效果: 消除 `request failed, retry 1/4` 警告，查询耗时降低约 55%
- **`connect_to_any` 优化** — 避免重复连接
  - 已连接时直接返回 (< 1ms)，不再遍历服务器列表
  - 优先尝试上次成功的服务器 (`last_server`)
- **`TDXRS_LOG` 环境变量修复** — 现在正确读取并生效
  - `logging::init()` 在模块初始化时调用
  - 支持: `off` / `error` / `warn` / `info` / `debug`

## v0.6.6 (2026-07-06) — 复权因子接口 + 连接管理优化

### 新增
- **`calc_fq_factors` 复权因子计算接口** — 新增独立的复权因子计算方法，不修改 K 线数据
  - 返回每个除权事件的详细因子信息（前复权/后复权因子、前收盘价、分红送股参数）
  - 返回累计因子（`cumulative_qfq` / `cumulative_hfq`）
  - 支持所有客户端：`TdxHqClient` / `TdxDirectClient` / `AsyncTdxHqClient`
  - Python 绑定：`client.calc_fq_factors(market, code, start=0, count=800)`
  - 用途：验证复权精度、导出因子表、与其他平台数据对比
- **`fetch_context_for_factors` 上下文追溯方法** — 自动追溯到上市时间
  - 最多拉取 30 页 (24000 根 ≈ 96 年)，确保覆盖所有除权事件
  - 独立于 `fetch_context_bars_for_adjust`，不影响现有复权算法
- **`FqContextTier` 复权上下文档位配置** — 支持三档配置
  - `Low`: 约 10 年 (2400 根) `Mid`: 约 20 年 (4800 根，默认) `High`: 约 30 年 (7200 根)
  - 客户端方法：`set_fq_context_tier()` / `fq_context_tier()`
- **`TdxSmartClient` 智能连接客户端** — 分层健康检查 + 本地缓存 (2026-07-21)
  - 快速初始连接: 仅验证 TCP + 握手，不做 K 线健康检查
  - 惰性健康检查: 首次 K 线请求返回空时触发，自动切换服务器
  - 本地缓存: 记录成功/失败服务器，下次连接优先使用缓存
  - 黑名单机制: 连续失败的服务器自动加入黑名单 (24h 过期)
  - Python 绑定：`client = tdxrs.TdxSmartClient()`

### 优化
- **复权上下文获取优化** — `fetch_context_bars_for_adjust` 支持可配置的翻页数
  - 新增 `fetch_context_bars_for_adjust_with_tier` 函数
  - 原有 `fetch_context_bars_for_adjust` 保持默认 Mid 档位
- **连接管理优化** — 修复服务器选择和心跳重连问题
  - **PRIMARY_SERVERS 修正** — 移除不可靠服务器，补入验证可用服务器
    - 2026-07-08: 移除华林4/海通8/海通3/海通2/海通4，补入国信1/华林7/杭州电信J2/J1/J4
    - 2026-07-17: 移除广发1 (119.29.19.242)，补入杭州电信J4 (115.238.90.165)
    - 已验证全部 10 台 PRIMARY 服务器 K 线/行情/逐笔数据正常
  - **心跳失败自动重连** — 心跳检测到断线后立即尝试连接替代服务器
    - 旧逻辑: 仅标记 `connected=false`，等待下次请求才触发重连
    - 新逻辑: 关闭池中空闲连接 → 遍历 PRIMARY 跳过失败服务器 → 成功则更新连接池
  - **重连跳过失败服务器** — `reconnect_if_needed` 不再重复尝试已确认失败的服务器
  - **`last_server` 线程安全** — 改为 `Arc<Mutex<>>`，心跳线程可同步更新服务器信息


## v0.6.5 (2026-07-02) — 逐笔成交精度修正 + CLI 增强 + 板块模块扩展

### 新增
- **TdxBlockClient 板块列表获取** — 新增 `get_block_list`、`get_industry_blocks`、`get_concept_blocks`、`get_index_blocks` 方法
  - 从服务器下载并解析 `.dat` 板块文件
  - Python 绑定同步暴露，返回 `list[dict]`
- **板块解析器无效数据过滤** — `parse_block` / `parse_block_group` 现在过滤 `block_type != 2` 的伪数据
  - 此前解析器会将对齐错误产生的股票代码片段混入板块名称
  - 过滤后仅返回有效板块记录
- **逐笔成交增加 reserved 字段** — `get_transaction_data` / `get_history_transaction_data` 返回值新增 `"reserved"` 字段
  - 原为被跳过的 extra field，现已解析
  - 股票数据中该字段始终为 0（保留字段）
- **CLI `trades` 命令增加笔数列** — 显示成交笔数 (`num` 字段)
- **CLI 下载命令显示保存位置** — `download`、`update`、`download-xdxr` 命令现在会显示数据保存路径
- **下载命令支持日期范围** — `download` 和 `update` 命令新增 `--start` 和 `--end` 参数
  - `tdxrs download 600519 --start 2024-01-01 --end 2024-12-31`
  - `tdxrs update --start 2024-06-01` — 从指定日期开始增量更新
- **update 命令支持指定股票** — `update` 命令新增 `--code` 参数
  - `tdxrs update --code 600519` — 增量更新指定股票
  - 默认行为：只更新已下载的股票（不会下载新股票）

### 修复
- **分时时间映射统一修正** — 上下开盘集合竞价均视为无有效数据点
  - 上午: 09:31 ~ 11:30 (index 0-119)，不含 09:30
  - 下午: 13:01 ~ 15:00 (index 120-239)，不含 13:00
  - 共 240 个数据点
- **场内基金证券类型扩展** — 58xxxx (科创板ETF) 现在正确识别为场内基金
- **逐笔成交价格精度修正** — 场内基金 (ETF/LOF/REITs) 逐笔成交价格现在使用正确的系数 (0.001)
- **分时数据价格系数修正** — `get_minute_time_data` 改为委托给历史分时 API，修复基金类价格 1000x 偏高问题
  - 影响: 所有客户端 (TdxHqClient / TdxDirectClient / AsyncTdxHqClient / TdxHqFundClient)
  - 原因: 实时分时 API (0x051d) 的价格编码与历史分时 API (0x0fb4) 不同
- **无效代码容错** — `parse_security_bars` / `parse_index_bars` 遇到无效日期时截断返回，而非报错
- **TdxBlockClient Python 绑定** — 新增 `TdxBlockClient` Python 类，支持板块 K 线和行情查询

### 文档
- **明确成交量单位** — 分时数据和逐笔成交的 `vol` 字段单位为**手**（1手=100股）
- **CLI 文档补充** — 添加默认下载位置和文件结构说明，更新 download/update 命令文档

## v0.6.4 (2026-06-30) — 分时数据增加均价字段

### 新增
- **分时数据增加 avg_price 字段** — `get_minute_time_data` / `get_history_minute_time_data` 返回值新增 `"avg_price"` 字段（成交均价）
  - 计算方式: 累计金额 / 累计成交量
  - 所有客户端 (TdxHqClient / TdxDirectClient / AsyncTdxHqClient / TdxHqFundClient) 同步生效
  - CLI `minutes` 命令同步增加"均价"列
- **分时数据默认倒序** — `get_history_minute_time_data` 返回数据默认按时间倒序排列（最新记录在前）
  - 在 Rust 解析层实现，Python 层无需额外处理
  - 便于查看最新数据，`--count N` 取最新 N 条
- **`minute_time_from_index` 公共方法** — 从 `protocol::parsers` 导出，供其他模块调用

### 修复
- **分时数据格式修正** — `get_minute_time_data` 内部改为调用历史分时 API，修复数据格式异常问题
  - 命令码 0x051d (实时) 存在差分编码异常，改用 0x0fb4 (历史)
  - 传入今日日期即可获取当日数据，格式与历史数据一致
- **分时时间映射修正** — 修正时间计算逻辑
  - 上午: 09:31 ~ 11:30 (index 0-119)，不含集合竞价 09:30
  - 下午: 13:00 ~ 14:59 (index 120-239)，不含收盘 15:00
  - 共 240 个数据点

## v0.6.3 (2026-06-25) — 异步客户端 + 限流 + 下载增强 + ErrorCode 迁移

### 新增
- **分时数据增加 time 字段** — `get_minute_time_data` / `get_history_minute_time_data` 返回值新增 `"time"` 字段（格式 `"HH:MM"`）
  - 基于数据索引推算: 上午 09:31~11:30 (index 0-119), 下午 13:00~14:59 (index 120-239)
  - 所有客户端 (TdxHqClient / TdxDirectClient / AsyncTdxHqClient / TdxHqFundClient) 同步生效
- **常量别名** — `PORT` (= `DEFAULT_PORT`), `POOL_SIZE` (= `DEFAULT_POOL_SIZE`)
  - 从 `tdxrs.constants` 导入即可使用
- **AsyncTdxHqClient 心跳** — tokio async 心跳任务
  - 每 10s 通过连接通道发送 keepalive 包 (`get_security_count(market=0)`)
  - 失败标记 `connected=false`，不触发重连 (由下次请求懒重连)
  - `connect()` 自动启动，`disconnect()` 自动停止
- **AsyncTdxHqClient Python 绑定** (`PyAsyncTdxHqClient`)
  - 内部持有独立 `tokio::runtime::Runtime`，通过 `block_on()` 同步调用
  - API 与 `TdxHqClient` 完全一致: 13 个数据方法 × 3 种输出格式 (dict/tuple/DataFrame)
  - 已注册到 `__init__.py`，`from tdxrs import AsyncTdxHqClient` 直接可用
- **AsyncTdxHqClient 集成测试** — 14 个真实服务器测试
  - 覆盖: 连接管理、全部 API、并发 `tokio::join!`、交易阶段检测、断线重连
  - 运行: `cargo test --features integration --test test_async_client`
- **交易时段限流** (`TradingPhase`) — 3 档自适应限流
  - Trading (盘中 9:30-15:00): 15 req/s
  - PrePost (盘前盘后): 30 req/s
  - Closed (休市): 60 req/s
  - 每连接独立限流，4 连接池实际吞吐 ×4
  - `auto_detect_phase()` 自动检测，基于 UTC+8 本地时间
  - Python: `client.set_phase("trading")` / `client.auto_detect_phase()`
- **批量行情查询上限** — `MAX_QUOTES_COUNT = 60`
  - `get_security_quotes` / `get_fund_quotes` 单次上限 60 只，超出自动截断并打印警告
  - 客户端侧截断 + 日志提醒，避免服务端静默丢弃数据
- **Downloader 按日下载** — 分时/逐笔数据按日期下载
  - `download_minute(dates, codes)` — 分时数据，协议原生日期查询
  - `download_ticks(dates, codes)` — 逐笔成交，自动翻页 (2000条/页)
  - `codes` 为必填参数，不支持全市场模式
  - 日期参数支持 `int`/`str`/`list`，自动去重排序
- **ErrorCode 体系全面迁移** — 49 处错误从原始变体迁移到结构化错误码
  - `error_codes.rs`: 新增 `ErrorCode::err()` 便捷方法
  - `fund/client.rs`: 9 处 `ResponseParse` → `FUND_CODE_NOT_SUPPORTED` / `INVALID_FORMAT`
  - `protocol/parsers.rs`: 15 处 → `RESPONSE_LENGTH_MISMATCH` / `INVALID_DATE`
  - `net/` 各模块: `CONNECTION_FAILED` / `RETRY_EXHAUSTED` / `DISCONNECTED` / `DECOMPRESS_FAILED` / `POOL_EXHAUSTED` 等
  - `net/`、`fund/`、`protocol/` 三个目录中 `ResponseParse` 和 `InvalidData` 已清零

### 变更
- **ETF 向后兼容代码清理** — 移除 ~300 行冗余代码
  - `src/fund/`: 移除 10 个 ETF 别名方法、5 个 ETF 常量别名、6 个 ETF 类型别名
  - `src/python/py_fund.rs`: 移除 10 个 ETF 兼容方法 + `is_etf` 静态方法
  - `src/python/py_etf.rs`: 删除 (444 行死代码，引用不存在的 `crate::etf`)
  - `TdxHqEtfClient` 类型别名、`EtfError` 类型别名均已移除
- **`pro.py` 弃用** — 替换为 `__getattr__` 懒加载弃用提示
  - `TdxHqEtfClient` → 重定向到 `TdxHqFundClient`
  - `TdxF10Client` → 提示需源码编译
- **版本号对齐** — `pyproject.toml` 同步更新至 0.6.3

### 修复
- **基金价格精度修复**
  - 修复场外基金 (519xxx) 价格系数错误的问题
  - 场内基金 (ETF/LOF/REITs): 系数 0.001 (3位小数)
  - 场外基金 (传统开放式基金): 系数 0.00001 (5位小数)
  - 修复前: 519003 显示 390.50 (错误)
  - 修复后: 519003 显示 3.9050 (正确，单位净值)
- **证券类型分类修复**
  - 修复沪市代码分类顺序，避免基金被误判为指数
  - 新增场外基金类型 (type=5)，区分场内/场外基金

### 文档
- **Python 最佳实践** (`docs/public/PYTHON_BEST_PRACTICES.md`) — 限流规则、客户端选择、输出格式、批量优化、反模式
- **API 参考** — 新增 AsyncTdxHqClient 完整文档 + Downloader 按日下载说明
- **架构说明** — 扩展 Net 模块描述，补充通道化连接池架构
- **F10 文档** — 新增 F10 公司资料使用指南
- **基金文档** — 新增数据核对注意事项 (OpenEnd K线100x、债券ETF净值差异)
- **变更日志** — 合并 v0.6.3 全部变更

### 测试
- 单元测试: 193 → 196 (+3 心跳测试)
- 集成测试: 0 → 14 (AsyncTdxHqClient 真实服务器)
- 总计: 210 tests passed

---

## v0.6.2 (2026-06-23) — 基金模块 + 板块查询 + 错误码

### 新增
- **基金模块** (`tdxrs.fund`)
  - ETF 模块重构为基金模块，覆盖 ETF/LOF/REITs/分级基金等全部基金类型
  - 新增 `FundType` 枚举: Etf / Lof / Reits / Structured / OpenEnd / Bond / Money / Other
  - `classify_fund(market, code)` 自动分类基金类型
  - 向后兼容: `TdxHqEtfClient` / `get_etf_list()` 等旧接口保留
- **板块查询模块** (`tdxrs.block`)
  - `TdxBlockClient` 板块专用客户端，内置K线级别限制
  - `BlockQuery` 查询引擎: 搜索/列表/成分查询
  - 指数成分精确匹配 (000300→沪深300)，行业/概念模糊搜索
  - 板块K线限制: 日/周/月无限制，分钟级默认50条，1min禁用
- **统一错误码体系** (`error_codes.rs`)
  - 30+ 错误码，按模块分段: 通用(1000)/代码分类(1100)/限流(1200)/连接(2000)/解析(3000)/文件(4000)
  - 板块代码(88xxxx)在通用客户端中自动拦截，返回 `[E1101]`
  - Python 端暴露错误码常量: `ERR_BLOCK_CODE_IN_GENERAL_CLIENT` 等
- **代码分类检测**
  - `classify_code()` / `is_block_code()` / `is_stock_code()` / `is_index_code()`
  - 通用客户端自动检测板块代码并拒绝

### 变更
- `src/etf/` 模块已移除，`src/fund/` 为唯一基金模块
- 通用客户端 (`TdxHqClient` / `TdxDirectClient`) 新增板块代码拦截
- Python 错误处理统一使用带错误码格式

---

## v0.6.1 (2026-06-23) — 限流 + 批量下载 + 日志

### 新增
- **请求限流** — 分级限流保护 TDX 服务器
  - 默认 50 req/s (通用), 日K 15 req/s, 分时 10 req/s (不可禁用)
  - 分时限流硬锁定，防止高频请求影响服务器
  - 全局上限 200 req/s，超过自动降级
  - Python API: `set_rate_limit(rps)` / `set_rate_limit_daily(rps)`
- **批量下载器** (`tdxrs.downloader.Downloader`)
  - 多服务器轮转分发，总吞吐量倍增
  - 默认 `.day` 格式，可被 `DailyBarReader` 直接读取
  - 支持 CSV / Parquet 格式 (需用户设置)
  - 增量更新 + 断点续传
  - 自动翻页 (每页 800 条)
- **日志系统激活** — 所有网络模块添加结构化日志
  - 连接/断开/重试/重连/心跳失败/连接池耗尽
  - 模块前缀: `hq` / `direct` / `f10` / `finance` / `pool` / `profile`
  - release 默认 WARN 级别，零性能影响
- **CLI 命令行工具** (`tdxrs.cli`)
  - 11 个子命令: quote / bars / minutes / trades / stocks / index / download / update / parse / servers / version
  - 参数锁上限 (quote 20 只, bars 800 条, download 50 req/s)
  - 三种输出格式: table / json / csv
  - 安装后直接使用: `tdxrs quote 600519`
- **日期校验** — `parse_security_bars` / `parse_index_bars` 增加年份范围检查
  - `max_valid_year()` 动态计算 (当前年份+10)，自动适应
  - 服务器返回损坏数据时返回解析错误，不再让 Python datetime 抛异常

### 变更
- `net/utils.rs` 新增 `RateLimiter` 结构体
- `constants.rs` 新增 `max_valid_year()` 函数
- `eprintln!` 替换为 `loge!` (受 `TDXRS_LOG` 级别控制)
- 测试: 128 → 139 (新增限流/日期/常量测试)

---

## v0.6.0 (2026-06-21) — 扩展模块: ETF + F10

### 新增
- **ETF 模块** (`tdxrs.pro.TdxHqEtfClient`) — ETF 专用行情客户端
  - K线 (12 周期)、实时行情 (五档)、分时、逐笔、除权除息、财务
  - ETF 列表自动筛选 (沪市 50/51, 深市 15/16)
  - ETF 代码验证 + 市场自动识别
  - 共享连接池，与股票行情相同性能
- **F10 模块** (`tdxrs.pro.TdxF10Client`) — F10 公司资料客户端 (源码编译, `--features f10`)
  - 16 分类数据获取 (公司概况/财务分析/股东研究等)
  - 独立 TCP 连接，不影响行情数据
  - Rust 内置 F10 文本解析器 (267K 字符 4.6ms)
  - 结构化提取: basic_info (15 字段) + listing_info (8 字段)
  - GBK 原始字节保留，避免编码损耗
  - ⚠️ 因数据合规考虑，未包含在 pip 包中，需从源码编译启用
- **模块分层** — `tdxrs` (核心) + `tdxrs.pro` (扩展)
- **共享工具** — `net/utils.rs` 新增 `auto_market`, `encode_gbk`, `encode_gbk_padded`
- **性能测试** — `tests/bench_etf_f10.py`

### 变更
- `profile/constants.rs` 移除重复的 `MARKET_SZ`/`MARKET_SH` (复用 `protocol::constants`)
- `etf/constants.rs` 移除重复常量，`is_sh_etf`/`is_sz_etf` 改为私有
- `etf/utils.rs` 移除死代码 (未使用的 `EtfError` 变体和工具函数)
- `profile/parser_f10.rs` 移除调试输出 (`eprintln!`)、`F10Parsed.raw` 字段、重复键
- `profile/types.rs` `F10Category` 新增 `filename_raw` 字段

### 文档
- 新增 [ETF 模块文档](../dev/ETF.md)
- 新增 [F10 模块文档](F10.md)
- 更新 README: 新增 PyPI/Stars 徽章、Star History、扩展模块介绍

---

## v0.5.0 (2026-05-13) — 首次 PyPI 发布

### 新增
- **TdxFinanceClient** — 独立财务客户端，分片下载 + 磁盘缓存 (24h TTL)
- **命名财务指标** — `get_finance_indicators()` 从 gpcw 提取 45 个核心指标（英文 key，原始值）
- **财务字段映射** — `src/protocol/finance_fields.rs`
- **轻量日志** — `logd!/logi!/logw!/loge!` 宏，`TDXRS_LOG` 环境变量控制级别
- **Net/Utils** — 提取三客户端共享逻辑，消除 ~360 行重复代码
- **连接池磁盘缓存** — TdxFinanceClient 24h 本地缓存 gpcw 文件
- **CI/CD** — GitHub Actions 多平台构建 (Linux/macOS/Windows) → PyPI 自动发布
- **文档** — 12 篇维护文档 + 3 个 demo 程序

### 变更
- `client.rs` / `direct_client.rs` / `async_client.rs` 精简去重
- `perform_handshake` 移至 `utils.rs`

### 修复
- **财务单位** — `get_finance_info` 移除所有 `×10000` 自动转换，返回 TDX 原始值
- **指数 K 线 fq** — 客户端强制 `fq=0`，不再透传用户值到服务端
- **自动分页排序** — `get_xxx_all` 改为 prepend，修复跨页乱序
- **quotes 边界检查** — `parse_security_quotes` 增加 `pos + 30` 保护
- 测试断言更新 (93 passed)

---

## v0.4.0 (2026-05-05)

### 新增
- **TdxDirectClient** Python 绑定 — 裸连接客户端 (无池/无重试)
- **DataFrame 输出** — Reader 和 Client 均支持 `to_dataframe()` / `*_dataframe()` 方法
- **协议常量子模块** (`tdxrs.constants`) — 市场代码、K线种类、复权类型等 26 个常量
- **复权参数** — K线 API 新增 `fq` 参数 (0=未复权, 1=前复权, 2=后复权)
- **context_bars 支持** — 除权日在 K 线范围之外时自动拉取历史数据补全因子计算
- 服务器列表扩展至 18 台 (优先组) + 101 台 (完整列表)
- 连接池服务管理 API: `set_servers` / `add_server` / `probe_servers` / `reorder_servers`
- 连接池大小默认提升至 5

### 变更
- pyo3 0.23 → 0.28 API 迁移
- Python 返回类型统一为 `list[dict]`
- 异步模块 (tokio) 启用编译
- 复权算法增加 context_bars 参数 (`adjust_security_bars` 签名变更)

### 修复
- Windows GNU 工具链 dlltool 编译支持
- `#[pymodule]` 命名与 maturin 配置对齐
- 早期除权事件 (早于 K 线数据范围) 静默丢弃

---

## v0.3.0 (2026-05-02)

### 新增
- 连接池 (`ConnectionPool` + RAII guard)
- 内置心跳线程 (10s keepalive)
- 断线自动重连 + 智能重试 (0.1~2.0s)
- 数据缓存 (security_count / security_list, TTL 30s)
- K线自动分页 (`get_security_bars_all` / `get_index_bars_all`)
- Tuple 高性能模式 (3 个 `*_tuples` 方法)

---

## v0.2.0 (2026-05-01)

### 新增
- 网络协议客户端: TCP 连接 + 13 个 API + 11 个响应解析器
- Python 绑定 (`PyTdxHqClient`)

---

## v0.1.0 (2026-05-01)

### 新增
- 本地文件解析: 日线 / 分钟线 / 板块 / 财务 (4 个 Reader)
- Python 绑定 (5 个 Reader 类)
- 单元测试 + 性能基准
