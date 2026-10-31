//! 异步 TDX 行情客户端
//!
//! 基于 tokio 的异步行情客户端，使用通道化连接池实现真正的并发请求。
//!
//! ## 架构
//!
//! ```text
//! AsyncTdxHqClient
//!   ├─ connections: Vec<ConnectionHandle>
//!   │    ├─ tx: mpsc::Sender<Request>   ← 发送请求
//!   │    └─ server: (String, u16)
//!   ├─ next_idx: AtomicUsize            ← 轮转分发
//!   └─ rate_limiter: AsyncRateLimiter
//!
//! 每个连接是独立的 tokio task，通过 mpsc 通道接收请求。
//! 请求在连接 task 内串行执行，多个连接之间真正并发。
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc};
use std::time::{Duration, Instant};

use flate2::read::ZlibDecoder;
use std::io::Read;
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::error::{Result, TdxError};
use crate::{logd, logi, logw};
use crate::net::packet::{ResponseHeader, RSP_HEADER_LEN};
use crate::net::utils::{self, TradingPhase};
use crate::protocol::constants::*;
use crate::protocol::parsers::*;
use crate::protocol::types::*;

// ================================================================
// 连接 task 内部结构
// ================================================================

/// 请求: (packet, oneshot 回复通道)
struct Request {
    data: Vec<u8>,
    reply: oneshot::Sender<Result<Vec<u8>>>,
}

/// 连接句柄: 持有发送通道
struct ConnectionHandle {
    tx: mpsc::Sender<Request>,
}

/// 单个连接 task: 管理一条 TCP 连接，通过通道接收并处理请求
struct ConnectionTask {
    stream: tokio::net::TcpStream,
}

impl ConnectionTask {
    /// 连接到服务器并完成握手 (默认 Full 模式)
    #[allow(dead_code)]
    async fn connect(ip: &str, port: u16, timeout_secs: f64) -> Result<Self> {
        Self::connect_with_mode(ip, port, timeout_secs, utils::HandshakeMode::Full).await
    }

    /// 连接到服务器并按指定模式完成握手
    async fn connect_with_mode(
        ip: &str,
        port: u16,
        timeout_secs: f64,
        mode: utils::HandshakeMode,
    ) -> Result<Self> {
        let addr = format!("{}:{}", ip, port);
        let stream = tokio::time::timeout(
            Duration::from_secs_f64(timeout_secs),
            tokio::net::TcpStream::connect(&addr),
        )
        .await
        .map_err(|_| crate::error_codes::ErrorCode::CONNECTION_TIMEOUT.err(&addr))?
        .map_err(|e| TdxError::Connection(format!("connect {}: {}", addr, e)))?;

        stream.set_nodelay(true)
            .map_err(|e| TdxError::Connection(format!("set_nodelay: {}", e)))?;

        let mut task = Self { stream };

        // 按模式执行握手
        for cmd in mode.commands() {
            task.send(cmd).await?;
            let head = task.recv(RSP_HEADER_LEN).await?;
            let header = ResponseHeader::parse(&head)?;
            let zip_size = header.zip_size as usize;
            let mut body = Vec::with_capacity(zip_size);
            while body.len() < zip_size {
                let chunk = task.recv(zip_size - body.len()).await?;
                body.extend_from_slice(&chunk);
            }
            if header.zip_size != header.unzip_size {
                let _ = decompress_zlib(&body)?;
            }
        }

        Ok(task)
    }

    /// 从已建立的流创建 (跳过握手，用于测试)
    #[allow(dead_code)]
    fn from_stream(stream: tokio::net::TcpStream) -> Self {
        Self { stream }
    }

    /// 运行连接 task: 循环处理请求直到通道关闭
    async fn run(mut self, mut rx: mpsc::Receiver<Request>) {
        while let Some(req) = rx.recv().await {
            let result = self.send_and_recv(&req.data).await;
            let _ = req.reply.send(result);
        }
    }

    async fn send(&mut self, data: &[u8]) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        self.stream.write_all(data).await
            .map_err(|e| TdxError::Connection(format!("send: {}", e)))
    }

    async fn recv(&mut self, len: usize) -> Result<Vec<u8>> {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; len];
        let mut total = 0;
        while total < len {
            let n = self.stream.read(&mut buf[total..]).await
                .map_err(|e| TdxError::Connection(format!("recv: {}", e)))?;
            if n == 0 {
                return Err(TdxError::Disconnected);
            }
            total += n;
        }
        Ok(buf)
    }

    async fn send_and_recv(&mut self, packet: &[u8]) -> Result<Vec<u8>> {
        self.send(packet).await?;

        let head_buf = self.recv(RSP_HEADER_LEN).await?;
        let header = ResponseHeader::parse(&head_buf)?;

        let zip_size = header.zip_size as usize;
        let mut body = Vec::with_capacity(zip_size);
        while body.len() < zip_size {
            let chunk = self.recv(zip_size - body.len()).await?;
            body.extend_from_slice(&chunk);
        }

        if body.is_empty() {
            return Err(crate::error_codes::ErrorCode::DISCONNECTED.err("empty response body"));
        }

        if header.zip_size != header.unzip_size {
            decompress_zlib(&body)
        } else {
            Ok(body)
        }
    }
}

// ================================================================
// 异步限流器
// ================================================================

/// tokio 兼容的异步限流器
struct AsyncRateLimiter {
    min_interval: Duration,
    last_request: tokio::sync::Mutex<Option<Instant>>,
}

impl AsyncRateLimiter {
    fn new(min_interval_ms: u64) -> Self {
        Self {
            min_interval: Duration::from_millis(min_interval_ms),
            last_request: tokio::sync::Mutex::new(None),
        }
    }

    async fn wait(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(t) = *last {
            let elapsed = t.elapsed();
            if elapsed < self.min_interval {
                tokio::time::sleep(self.min_interval - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }

    fn set_rps(&mut self, rps: u32) {
        if rps == 0 {
            self.min_interval = Duration::from_millis(0);
        } else {
            let capped = rps.min(200);
            self.min_interval = Duration::from_millis(1000 / capped as u64);
        }
    }
}

// ================================================================
// 缓存
// ================================================================

struct CacheEntry<T> {
    data: T,
    expires_at: Instant,
}

// ================================================================
// 异步 TDX 行情客户端
// ================================================================

/// 默认连接池大小
const DEFAULT_POOL_SIZE: usize = 4;

/// 异步 TDX 行情客户端
///
/// 使用通道化连接池，支持真正的并发请求。
/// API 与同步版 `TdxHqClient` 相同，但所有方法都是 async。
///
/// # 示例
///
/// ```no_run
/// # async fn example() -> tdxrs::error::Result<()> {
/// use tdxrs::net::async_client::AsyncTdxHqClient;
///
/// let client = AsyncTdxHqClient::new();
/// client.connect("180.153.18.170", 7709, None).await?;
///
/// // 并发请求: 两个查询真正并行执行
/// let (bars, quotes) = tokio::join!(
///     client.get_security_bars(4, 1, "600519", 0, 100, 0),
///     client.get_security_quotes(&[(1, "600519"), (0, "000858")]),
/// );
/// # Ok(())
/// # }
/// ```
pub struct AsyncTdxHqClient {
    connections: Arc<Mutex<Vec<ConnectionHandle>>>,
    next_idx: AtomicUsize,
    rate_limiter: AsyncRateLimiter,
    count_cache: Mutex<HashMap<u8, CacheEntry<u16>>>,
    list_cache: Mutex<HashMap<u8, CacheEntry<Vec<SecurityInfo>>>>,
    cache_ttl: Duration,
    pool_size: usize,
    /// 心跳停止信号: 发送 `()` 表示停止心跳 task
    heartbeat_stop: tokio::sync::Mutex<Option<oneshot::Sender<()>>>,
    /// 连接是否存活 (心跳检测)
    connected: Arc<std::sync::atomic::AtomicBool>,
    /// 上次连接的服务器 (用于重连时跳过)
    last_server: tokio::sync::Mutex<Option<(String, u16)>>,
    /// 复权上下文数据量档位 (默认 Mid ≈ 20 年), 以 u8 存储
    fq_context_tier: AtomicU8,
    /// 握手模式 (Issue #12)
    handshake_mode: AtomicU8,
}

impl AsyncTdxHqClient {
    pub fn new() -> Self {
        Self::with_pool_size(DEFAULT_POOL_SIZE)
    }

    /// 指定连接池大小
    pub fn with_pool_size(pool_size: usize) -> Self {
        Self {
            connections: Arc::new(Mutex::new(Vec::new())),
            next_idx: AtomicUsize::new(0),
            rate_limiter: AsyncRateLimiter::new(20), // 50 req/s
            count_cache: Mutex::new(HashMap::new()),
            list_cache: Mutex::new(HashMap::new()),
            cache_ttl: Duration::from_secs(30),
            pool_size: pool_size.max(1),
            heartbeat_stop: tokio::sync::Mutex::new(None),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_server: tokio::sync::Mutex::new(None),
            fq_context_tier: AtomicU8::new(utils::FqContextTier::default() as u8),
            handshake_mode: AtomicU8::new(utils::HandshakeMode::Full as u8),
        }
    }

    /// 获取当前握手模式
    pub fn handshake_mode(&self) -> utils::HandshakeMode {
        utils::HandshakeMode::from_u8(self.handshake_mode.load(Ordering::Relaxed))
    }

    /// 设置握手模式
    pub fn set_handshake_mode(&self, mode: utils::HandshakeMode) {
        self.handshake_mode.store(mode as u8, Ordering::Relaxed);
    }

    /// 连接到指定服务器 (建立 N 个连接)
    pub async fn connect(&self, ip: &str, port: u16, timeout: Option<f64>) -> Result<bool> {
        let timeout_secs = timeout.unwrap_or(CONNECT_TIMEOUT);
        let mut conns = self.connections.lock().await;

        // 清理旧连接
        conns.clear();

        for _ in 0..self.pool_size {
            let task = ConnectionTask::connect_with_mode(
                ip, port, timeout_secs, self.handshake_mode(),
            ).await?;
            let (tx, rx) = mpsc::channel::<Request>(64);
            tokio::spawn(task.run(rx));
            conns.push(ConnectionHandle { tx });
        }

        self.count_cache.lock().await.clear();
        self.list_cache.lock().await.clear();
        drop(conns);

        // 记录当前服务器
        *self.last_server.lock().await = Some((ip.to_string(), port));

        // 启动心跳
        self.start_heartbeat().await;

        Ok(true)
    }

    /// 连接到任意可用服务器
    pub async fn connect_to_any(&self, timeout: Option<f64>) -> Result<bool> {
        for &(_, ip, port) in DEFAULT_SERVERS {
            match self.connect(ip, port, timeout).await {
                Ok(true) => return Ok(true),
                _ => continue,
            }
        }
        Err(crate::error_codes::ErrorCode::CONNECTION_FAILED.err("all servers unreachable"))
    }

    /// 断开所有连接
    pub async fn disconnect(&self) {
        self.stop_heartbeat().await;
        self.connections.lock().await.clear();
        self.connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// 当前连接数
    pub async fn connection_count(&self) -> usize {
        self.connections.lock().await.len()
    }

    /// 设置限流 RPS
    pub fn set_rate_limit(&mut self, rps: u32) {
        self.rate_limiter.set_rps(rps);
    }

    /// 设置交易阶段限流
    pub fn set_phase(&mut self, phase: TradingPhase) {
        let rps = match phase {
            TradingPhase::Trading => 50,
            TradingPhase::PrePost => 100,
            TradingPhase::Closed => 200,
        };
        self.rate_limiter.set_rps(rps);
    }

    /// 自动检测交易阶段并设置限流
    pub fn auto_detect_phase(&mut self) -> TradingPhase {
        let phase = utils::detect_trading_phase();
        self.set_phase(phase);
        phase
    }

    /// 连接是否存活
    pub fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 尝试切换到另一台服务器
    ///
    /// 遍历 PRIMARY_SERVERS，跳过当前服务器，连接到第一台可用的。
    async fn reconnect_to_another_server(&self) {
        let current = self.last_server.lock().await.clone();

        for &(_, ip, port) in PRIMARY_SERVERS {
            // 跳过当前服务器
            if let Some((ref cur_ip, cur_port)) = current {
                if ip == cur_ip.as_str() && port == cur_port {
                    continue;
                }
            }

            match self.connect(ip, port, Some(CONNECT_TIMEOUT)).await {
                Ok(true) => {
                    logi!("async", "switched to server {}:{}", ip, port);
                    return;
                }
                _ => continue,
            }
        }

        logw!("async", "no alternative server available");
    }

    // ================================================================
    // 心跳
    // ================================================================

    /// 启动心跳任务: 每 10 秒发送 keepalive 包检测连接存活
    ///
    /// 内部使用 `get_security_count(market=0)` 作为轻量探测包。
    /// 心跳失败仅标记 `connected=false`，不触发重连 (由下次请求触发)。
    async fn start_heartbeat(&self) {
        self.stop_heartbeat().await;

        let conns = Arc::clone(&self.connections);
        let connected = Arc::clone(&self.connected);
        let (tx_stop, mut rx_stop) = oneshot::channel::<()>();

        tokio::spawn(async move {
            let interval = Duration::from_secs_f64(
                crate::protocol::constants::DEFAULT_HEARTBEAT_INTERVAL,
            );
            let mut tick: usize = 0;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = &mut rx_stop => break,
                }

                // 构造 keepalive 包 (get_security_count, market=0)
                let mut packet = Vec::with_capacity(18);
                packet.extend_from_slice(&[
                    0x0c, 0x0c, 0x18, 0x6c, 0x00, 0x01, 0x08, 0x00, 0x08, 0x00, 0x4e, 0x04,
                ]);
                packet.extend_from_slice(&0u16.to_le_bytes());
                packet.extend_from_slice(&[0x75, 0xc7, 0x33, 0x01]);

                // 轮转选择一个连接发送心跳
                let alive = {
                    let conns_guard = conns.lock().await;
                    if conns_guard.is_empty() {
                        false
                    } else {
                        let idx = tick % conns_guard.len();
                        tick = tick.wrapping_add(1);
                        // 使用 try_send 避免阻塞 (通道满 = 连接忙 = 跳过)
                        let (reply_tx, reply_rx) = oneshot::channel();
                        let req = Request {
                            data: packet,
                            reply: reply_tx,
                        };
                        if conns_guard[idx].tx.try_send(req).is_err() {
                            false
                        } else {
                            // 等待响应 (带超时)
                            match tokio::time::timeout(Duration::from_secs(5), reply_rx).await {
                                Ok(Ok(Ok(_))) => true,
                                _ => false,
                            }
                        }
                    }
                };

                if !alive {
                    connected.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            }
        });

        *self.heartbeat_stop.lock().await = Some(tx_stop);
        self.connected
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// 停止心跳任务
    async fn stop_heartbeat(&self) {
        if let Some(stop) = self.heartbeat_stop.lock().await.take() {
            let _ = stop.send(());
        }
    }

    // ================================================================
    // 内部: 发送请求 (轮转 + 限流 + 重试)
    // ================================================================

    /// 发送请求并接收响应 (限流 + 重试)
    async fn send_and_recv(&self, packet: &[u8]) -> Result<Vec<u8>> {
        self.rate_limiter.wait().await;

        // 第一次尝试
        match self.try_send(packet).await {
            Ok(body) => return Ok(body),
            Err(e) => {
                // 重试
                for (i, &interval) in RETRY_INTERVALS.iter().enumerate() {
                    tokio::time::sleep(Duration::from_secs_f64(interval)).await;
                    match self.try_send(packet).await {
                        Ok(body) => return Ok(body),
                        Err(_) if i + 1 == RETRY_INTERVALS.len() => return Err(e),
                        Err(_) => continue,
                    }
                }
                Err(e)
            }
        }
    }

    /// 单次尝试: 通过通道发送请求到连接 task
    async fn try_send(&self, packet: &[u8]) -> Result<Vec<u8>> {
        let conns = self.connections.lock().await;
        if conns.is_empty() {
            return Err(TdxError::Disconnected);
        }

        // 轮转选择连接
        let idx = self.next_idx.fetch_add(1, Ordering::Relaxed) % conns.len();
        let handle = &conns[idx];

        let (tx_reply, rx_reply) = oneshot::channel();
        let req = Request {
            data: packet.to_vec(),
            reply: tx_reply,
        };

        // 发送请求 (如果通道满则尝试下一个连接)
        if handle.tx.try_send(req).is_err() {
            // 通道满或已关闭，尝试下一个
            let next = (idx + 1) % conns.len();
            if next != idx {
                let (tx_reply2, rx_reply2) = oneshot::channel();
                let req2 = Request {
                    data: packet.to_vec(),
                    reply: tx_reply2,
                };
                conns[next].tx.try_send(req2)
                    .map_err(|_| TdxError::Connection("all connections busy".into()))?;
                return rx_reply2.await
                    .map_err(|_| TdxError::Disconnected)?;
            }
            return Err(TdxError::Connection("connection busy".into()));
        }

        rx_reply.await
            .map_err(|_| TdxError::Disconnected)?
    }

    // ================================================================
    // 复权上下文拉取
    // ================================================================

    async fn fetch_context_bars_for_adjust(
        &self,
        category: u8,
        market: u8,
        code: &str,
        bars: &[SecurityBar],
        xdxr: &[XdXrInfo],
    ) -> Vec<SecurityBar> {
        if bars.is_empty() || xdxr.is_empty() {
            return Vec::new();
        }

        let earliest_event = xdxr
            .iter()
            .filter(|x| x.category == 1)
            .map(|x| x.year as u32 * 10000 + x.month as u32 * 100 + x.day as u32)
            .min();

        let Some(ee_date) = earliest_event else { return Vec::new() };

        let first_bar_date =
            bars[0].year as u32 * 10000 + bars[0].month as u32 * 100 + bars[0].day as u32;

        if first_bar_date <= ee_date {
            return Vec::new();
        }

        let max_per_page = MAX_KLINE_COUNT as u32;
        let max_pages = self.fq_context_tier().pages();
        let mut context = Vec::new();
        let mut offset = max_per_page;

        for _page in 0..max_pages {
            let pkt = utils::build_security_bars_packet(
                category, market, code, offset, MAX_KLINE_COUNT, 0,
            );
            let body = match self.send_and_recv(&pkt).await {
                Ok(b) => b,
                Err(_) => break,
            };
            let batch = match parse_security_bars(&body, category) {
                Ok(b) => b,
                Err(_) => break,
            };
            if batch.is_empty() {
                break;
            }

            let batch_first_date =
                batch[0].year as u32 * 10000 + batch[0].month as u32 * 100 + batch[0].day as u32;

            let len_before = context.len();
            context.splice(0..0, batch);

            if batch_first_date <= ee_date {
                break;
            }

            offset += max_per_page;
            if context.len() == len_before {
                break;
            }
        }

        context
    }

    /// 设置复权上下文数据量档位
    pub fn set_fq_context_tier(&self, tier: utils::FqContextTier) {
        let val: u8 = match tier {
            utils::FqContextTier::Low => 0,
            utils::FqContextTier::Mid => 1,
            utils::FqContextTier::High => 2,
        };
        self.fq_context_tier.store(val, Ordering::SeqCst);
    }

    /// 获取当前复权上下文档位
    pub fn fq_context_tier(&self) -> utils::FqContextTier {
        let val = self.fq_context_tier.load(Ordering::SeqCst);
        match val {
            0 => utils::FqContextTier::Low,
            2 => utils::FqContextTier::High,
            _ => utils::FqContextTier::Mid,
        }
    }

    // ================================================================
    // API 方法
    // ================================================================

    /// 获取 K 线数据
    pub async fn get_security_bars(
        &self,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
        fq: u8,
    ) -> Result<Vec<SecurityBar>> {
        // 日K线空响应自动重试 (与同步版一致)
        let should_retry_empty = category >= 4;
        let max_retry = if should_retry_empty { 3 } else { 1 };

        let mut bars = Vec::new();
        let mut retry_count = 0;

        for attempt in 0..max_retry {
            let packet = utils::build_security_bars_packet(category, market, code, start, count, fq);
            let body = self.send_and_recv(&packet).await?;
            let parsed = parse_security_bars(&body, category)?;

            if !parsed.is_empty() || !should_retry_empty {
                bars = parsed;
                break;
            }

            retry_count += 1;
            logd!("async", "attempt {}/{}: empty K-line for {}, switching server",
                  attempt + 1, max_retry, code);

            if attempt < max_retry - 1 {
                self.reconnect_to_another_server().await;
            }
        }

        if bars.is_empty() && should_retry_empty && retry_count > 0 {
            logw!("async", "all {} attempts returned empty K-line for {}", max_retry, code);
        } else if retry_count > 0 && !bars.is_empty() {
            logi!("async", "got {} bars for {} after {} server switch(es)", bars.len(), code, retry_count);
        }

        if fq != 0 && !bars.is_empty() {
            if let Ok(xdxr) = self.get_xdxr_info(market, code).await {
                use crate::protocol::adjuster::{adjust_security_bars, FqType};
                let fq_enum = if fq == 2 { FqType::Hfq } else { FqType::Qfq };
                let context = self
                    .fetch_context_bars_for_adjust(category, market, code, &bars, &xdxr)
                    .await;
                adjust_security_bars(&mut bars, &context, &xdxr, fq_enum);
            }
        }
        Ok(bars)
    }

    /// 自动分页获取 K 线
    pub async fn get_security_bars_all(
        &self,
        category: u8,
        market: u8,
        code: &str,
        count: u16,
        fq: u8,
    ) -> Result<Vec<SecurityBar>> {
        let mut all_bars = Vec::new();
        let mut offset = 0u32;
        let mut remaining = count;

        while remaining > 0 {
            let batch = remaining.min(MAX_KLINE_COUNT);
            let bars = self
                .get_security_bars(category, market, code, offset, batch, fq)
                .await?;
            if bars.is_empty() {
                break;
            }
            let fetched = bars.len() as u16;
            all_bars.extend(bars);
            remaining = remaining.saturating_sub(fetched);
            offset += fetched as u32;
            if fetched < batch {
                break;
            }
        }

        Ok(all_bars)
    }

    /// 获取指数 K 线
    pub async fn get_index_bars(
        &self,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
        fq: u8,
    ) -> Result<Vec<IndexBar>> {
        let _ = fq;

        // 日K线空响应自动重试 (与同步版一致)
        let should_retry_empty = category >= 4;
        let max_retry = if should_retry_empty { 3 } else { 1 };

        let mut bars = Vec::new();
        let mut retry_count = 0;

        for attempt in 0..max_retry {
            let packet = utils::build_index_bars_packet(category, market, code, start, count, 0);
            let body = self.send_and_recv(&packet).await?;
            let parsed = parse_index_bars(&body, category)?;

            if !parsed.is_empty() || !should_retry_empty {
                bars = parsed;
                break;
            }

            retry_count += 1;
            logd!("async", "attempt {}/{}: empty index K-line for {}, switching server",
                  attempt + 1, max_retry, code);

            if attempt < max_retry - 1 {
                self.reconnect_to_another_server().await;
            }
        }

        if bars.is_empty() && should_retry_empty && retry_count > 0 {
            logw!("async", "all {} attempts returned empty index K-line for {}", max_retry, code);
        } else if retry_count > 0 && !bars.is_empty() {
            logi!("async", "got {} index bars for {} after {} server switch(es)", bars.len(), code, retry_count);
        }

        Ok(bars)
    }

    /// 获取实时行情 (批量)
    ///
    /// 单次查询上限 60 只 (TDX 服务端硬限制)，超出自动截断并打印警告。
    pub async fn get_security_quotes(
        &self,
        all_stock: &[(u8, &str)],
    ) -> Result<Vec<SecurityQuote>> {
        // 服务端上限截断
        let all_stock = if all_stock.len() > MAX_QUOTES_COUNT {
            logw!("async_hq", "批量行情查询超过上限 {}/{}，自动截断。请自行分组调用。",
                  all_stock.len(), MAX_QUOTES_COUNT);
            &all_stock[..MAX_QUOTES_COUNT]
        } else {
            all_stock
        };

        // 空响应自动重试 (某些服务器不支持实时行情)
        let max_retry = 3;
        let mut retry_count = 0;

        for attempt in 0..max_retry {
            let stock_len = all_stock.len() as u16;
            let pkgdatalen = (stock_len as u32) * 7 + 12;

            let mut packet = Vec::with_capacity(26 + stock_len as usize * 7);
            packet.extend_from_slice(&0x010Cu16.to_le_bytes());
            packet.extend_from_slice(&0x02006320u32.to_le_bytes());
            packet.extend_from_slice(&(pkgdatalen as u16).to_le_bytes());
            packet.extend_from_slice(&(pkgdatalen as u16).to_le_bytes());
            packet.extend_from_slice(&CMD_SECURITY_QUOTES.to_le_bytes());
            packet.extend_from_slice(&0u32.to_le_bytes());
            packet.extend_from_slice(&0u16.to_le_bytes());
            packet.extend_from_slice(&stock_len.to_le_bytes());
            for &(market, code) in all_stock {
                packet.push(market);
                packet.extend_from_slice(&utils::code_bytes(code));
            }

            let body = self.send_and_recv(&packet).await?;
            let quotes = parse_security_quotes(&body)?;

            if !quotes.is_empty() {
                if retry_count > 0 {
                    logi!("async", "got {} quotes after {} server switch(es)", quotes.len(), retry_count);
                }
                return Ok(quotes);
            }

            // 空响应，尝试切换服务器
            retry_count += 1;
            logd!("async", "attempt {}/{}: empty quotes, switching server",
                  attempt + 1, max_retry);

            if attempt < max_retry - 1 {
                self.reconnect_to_another_server().await;
            }
        }

        logw!("async", "all {} attempts returned empty quotes", max_retry);
        Ok(Vec::new())
    }

    /// 获取证券列表
    pub async fn get_security_list(
        &self,
        market: u8,
        start: u16,
    ) -> Result<Vec<SecurityInfo>> {
        if start == 0 {
            let cache = self.list_cache.lock().await;
            if let Some(entry) = cache.get(&market) {
                if Instant::now() < entry.expires_at {
                    return Ok(entry.data.clone());
                }
            }
            drop(cache);
        }

        let mut packet = Vec::with_capacity(16);
        packet.extend_from_slice(&[
            0x0c, 0x01, 0x18, 0x64, 0x01, 0x01, 0x06, 0x00, 0x06, 0x00, 0x50, 0x04,
        ]);
        packet.extend_from_slice(&(market as u16).to_le_bytes());
        packet.extend_from_slice(&start.to_le_bytes());

        let body = self.send_and_recv(&packet).await?;
        let result = parse_security_list(&body)?;

        if start == 0 {
            let mut cache = self.list_cache.lock().await;
            cache.insert(
                market,
                CacheEntry {
                    data: result.clone(),
                    expires_at: Instant::now() + self.cache_ttl,
                },
            );
        }

        Ok(result)
    }

    /// 获取证券数量
    pub async fn get_security_count(&self, market: u8) -> Result<u16> {
        {
            let cache = self.count_cache.lock().await;
            if let Some(entry) = cache.get(&market) {
                if Instant::now() < entry.expires_at {
                    return Ok(entry.data);
                }
            }
        }

        let mut packet = Vec::with_capacity(18);
        packet.extend_from_slice(&[
            0x0c, 0x0c, 0x18, 0x6c, 0x00, 0x01, 0x08, 0x00, 0x08, 0x00, 0x4e, 0x04,
        ]);
        packet.extend_from_slice(&(market as u16).to_le_bytes());
        packet.extend_from_slice(&[0x75, 0xc7, 0x33, 0x01]);

        let body = self.send_and_recv(&packet).await?;
        let count = parse_security_count(&body)?;

        let mut cache = self.count_cache.lock().await;
        cache.insert(
            market,
            CacheEntry {
                data: count,
                expires_at: Instant::now() + self.cache_ttl,
            },
        );
        Ok(count)
    }

    /// 获取当日分时数据 (委托给历史分时 API，避免实时 API 价格编码异常)
    pub async fn get_minute_time_data(
        &self,
        market: u8,
        code: &str,
    ) -> Result<Vec<MinuteTimePrice>> {
        let today = utils::today_yyyymmdd();
        self.get_history_minute_time_data(market, code, today).await
    }

    /// 获取历史分时数据
    pub async fn get_history_minute_time_data(
        &self,
        market: u8,
        code: &str,
        date: u32,
    ) -> Result<Vec<MinuteTimePrice>> {
        let code_buf = utils::code_bytes(code);
        let mut packet = Vec::with_capacity(23);
        packet.extend_from_slice(&[
            0x0c, 0x01, 0x30, 0x00, 0x01, 0x01, 0x0d, 0x00, 0x0d, 0x00, 0xb4, 0x0f,
        ]);
        packet.extend_from_slice(&date.to_le_bytes());
        packet.push(market);
        packet.extend_from_slice(&code_buf);

        let body = self.send_and_recv(&packet).await?;
        parse_history_minute_time_data(&body, market, code)
    }

    /// 获取逐笔成交
    pub async fn get_transaction_data(
        &self,
        market: u8,
        code: &str,
        start: u16,
        count: u16,
    ) -> Result<Vec<TickData>> {
        let code_buf = utils::code_bytes(code);
        let mut packet = Vec::with_capacity(24);
        packet.extend_from_slice(&[
            0x0c, 0x17, 0x08, 0x01, 0x01, 0x01, 0x0e, 0x00, 0x0e, 0x00, 0xc5, 0x0f,
        ]);
        packet.extend_from_slice(&(market as u16).to_le_bytes());
        packet.extend_from_slice(&code_buf);
        packet.extend_from_slice(&start.to_le_bytes());
        packet.extend_from_slice(&count.to_le_bytes());

        let body = self.send_and_recv(&packet).await?;
        let coefficient = get_security_coefficient(market, code);
        parse_transaction_data_with_coefficient(&body, coefficient)
    }

    /// 获取历史逐笔成交
    pub async fn get_history_transaction_data(
        &self,
        market: u8,
        code: &str,
        start: u16,
        count: u16,
        date: u32,
    ) -> Result<Vec<TickData>> {
        let code_buf = utils::code_bytes(code);
        let mut packet = Vec::with_capacity(28);
        packet.extend_from_slice(&[
            0x0c, 0x01, 0x30, 0x01, 0x00, 0x01, 0x12, 0x00, 0x12, 0x00, 0xb5, 0x0f,
        ]);
        packet.extend_from_slice(&date.to_le_bytes());
        packet.extend_from_slice(&(market as u16).to_le_bytes());
        packet.extend_from_slice(&code_buf);
        packet.extend_from_slice(&start.to_le_bytes());
        packet.extend_from_slice(&count.to_le_bytes());

        let body = self.send_and_recv(&packet).await?;
        let coefficient = get_security_coefficient(market, code);
        parse_history_transaction_data_with_coefficient(&body, coefficient)
    }

    /// 获取财务信息
    pub async fn get_finance_info(&self, market: u8, code: &str) -> Result<FinanceInfo> {
        let code_buf = utils::code_bytes(code);
        let mut packet = Vec::with_capacity(21);
        packet.extend_from_slice(&[
            0x0c, 0x1f, 0x18, 0x76, 0x00, 0x01, 0x0b, 0x00, 0x0b, 0x00, 0x10, 0x00, 0x01,
            0x00,
        ]);
        packet.push(market);
        packet.extend_from_slice(&code_buf);

        let body = self.send_and_recv(&packet).await?;
        parse_finance_info(&body, market, code)
    }

    /// 获取除权除息信息
    pub async fn get_xdxr_info(&self, market: u8, code: &str) -> Result<Vec<XdXrInfo>> {
        let code_buf = utils::code_bytes(code);
        let mut packet = Vec::with_capacity(21);
        packet.extend_from_slice(&[
            0x0c, 0x1f, 0x18, 0x76, 0x00, 0x01, 0x0b, 0x00, 0x0b, 0x00, 0x0f, 0x00, 0x01,
            0x00,
        ]);
        packet.push(market);
        packet.extend_from_slice(&code_buf);

        let body = self.send_and_recv(&packet).await?;
        parse_xdxr_info(&body)
    }
}

fn decompress_zlib(data: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(data);
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| crate::error_codes::ErrorCode::DECOMPRESS_FAILED.err(format!("{}", e)))?;
    Ok(decompressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ================================================================
    // 基础创建 / 状态
    // ================================================================

    #[test]
    fn test_async_client_creation() {
        let client = AsyncTdxHqClient::new();
        assert_eq!(client.pool_size, DEFAULT_POOL_SIZE);
    }

    #[test]
    fn test_async_client_custom_pool_size() {
        let client = AsyncTdxHqClient::with_pool_size(8);
        assert_eq!(client.pool_size, 8);
    }

    #[test]
    fn test_async_client_min_pool_size() {
        let client = AsyncTdxHqClient::with_pool_size(0);
        assert_eq!(client.pool_size, 1);
    }

    #[tokio::test]
    async fn test_async_client_not_connected() {
        let client = AsyncTdxHqClient::new();
        assert_eq!(client.connection_count().await, 0);
    }

    #[tokio::test]
    async fn test_async_client_disconnect_empty() {
        let client = AsyncTdxHqClient::new();
        client.disconnect().await;
        assert_eq!(client.connection_count().await, 0);
    }

    #[tokio::test]
    async fn test_async_client_send_when_disconnected() {
        let client = AsyncTdxHqClient::new();
        let result = client.get_security_count(1).await;
        assert!(result.is_err());
    }

    // ================================================================
    // AsyncRateLimiter
    // ================================================================

    #[tokio::test]
    async fn test_async_rate_limiter_no_delay_first_call() {
        let limiter = AsyncRateLimiter::new(50);
        let start = Instant::now();
        limiter.wait().await;
        assert!(start.elapsed() < Duration::from_millis(20));
    }

    #[tokio::test]
    async fn test_async_rate_limiter_delays_second_call() {
        let limiter = AsyncRateLimiter::new(100);
        limiter.wait().await;
        let start = Instant::now();
        limiter.wait().await;
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(70));
        assert!(elapsed < Duration::from_millis(200));
    }

    #[tokio::test]
    async fn test_async_rate_limiter_set_rps() {
        let mut limiter = AsyncRateLimiter::new(100);
        limiter.set_rps(200);
        limiter.wait().await;
        let start = Instant::now();
        limiter.wait().await;
        assert!(start.elapsed() < Duration::from_millis(30));
    }

    #[tokio::test]
    async fn test_async_rate_limiter_set_rps_zero_disables() {
        let mut limiter = AsyncRateLimiter::new(100);
        limiter.set_rps(0);
        limiter.wait().await;
        let start = Instant::now();
        limiter.wait().await;
        assert!(start.elapsed() < Duration::from_millis(10));
    }

    // ================================================================
    // 通道 roundtrip (直接注入 Response，不经过 TCP)
    // ================================================================

    /// 创建一个 mock 连接 task: 收到请求后直接返回预设响应 (不走 TCP)
    fn spawn_mock_task(response: Vec<u8>) -> mpsc::Sender<Request> {
        let (tx, rx) = mpsc::channel::<Request>(64);
        let resp = Arc::new(response);
        tokio::spawn(async move {
            let mut rx = rx;
            while let Some(req) = rx.recv().await {
                let _ = req.reply.send(Ok((*resp).clone()));
            }
        });
        tx
    }

    #[tokio::test]
    async fn test_channel_roundtrip_mock() {
        let tx = spawn_mock_task(vec![0x01, 0x02, 0x03]);

        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(Request {
            data: vec![0xAA],
            reply: reply_tx,
        })
        .await
        .unwrap();

        let result = reply_rx.await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), vec![0x01, 0x02, 0x03]);
    }

    #[tokio::test]
    async fn test_channel_multiple_sequential_mock() {
        let tx = spawn_mock_task(vec![0x42]);

        for _ in 0..5 {
            let (reply_tx, reply_rx) = oneshot::channel();
            tx.send(Request {
                data: vec![0x00],
                reply: reply_tx,
            })
            .await
            .unwrap();
            assert_eq!(reply_rx.await.unwrap().unwrap(), vec![0x42]);
        }
    }

    #[tokio::test]
    async fn test_channel_concurrent_mock() {
        let tx = spawn_mock_task(vec![0xFF]);

        let mut handles = Vec::new();
        for _ in 0..20 {
            let tx = tx.clone();
            handles.push(tokio::spawn(async move {
                let (reply_tx, reply_rx) = oneshot::channel();
                tx.send(Request {
                    data: vec![0x00],
                    reply: reply_tx,
                })
                .await
                .unwrap();
                reply_rx.await.unwrap()
            }));
        }

        for h in handles {
            assert_eq!(h.await.unwrap().unwrap(), vec![0xFF]);
        }
    }

    // ================================================================
    // 连接池轮转 + 并发
    // ================================================================

    /// 向 client 注入 mock 连接句柄
    async fn inject_mock(client: &AsyncTdxHqClient, tx: mpsc::Sender<Request>) {
        client.connections.lock().await.push(ConnectionHandle { tx });
    }

    #[tokio::test]
    async fn test_pool_round_robin() {
        let client = AsyncTdxHqClient::with_pool_size(2);
        let tx1 = spawn_mock_task(vec![]);
        let tx2 = spawn_mock_task(vec![]);
        inject_mock(&client, tx1).await;
        inject_mock(&client, tx2).await;

        assert_eq!(client.connection_count().await, 2);

        // 6 次请求，轮转 idx 从 0 到 5
        for _ in 0..6 {
            let conns = client.connections.lock().await;
            let idx = client.next_idx.fetch_add(1, Ordering::Relaxed) % conns.len();
            let (tx, rx) = oneshot::channel();
            conns[idx]
                .tx
                .send(Request {
                    data: vec![],
                    reply: tx,
                })
                .await
                .unwrap();
            drop(conns);
            assert!(rx.await.unwrap().is_ok());
        }
        assert_eq!(client.next_idx.load(Ordering::Relaxed), 6);
    }

    #[tokio::test]
    async fn test_pool_concurrent_4_connections() {
        let client = AsyncTdxHqClient::with_pool_size(4);
        for _ in 0..4 {
            inject_mock(&client, spawn_mock_task(vec![0xAA; 100])).await;
        }

        let start = Instant::now();
        let mut handles = Vec::new();
        for _ in 0..20 {
            let conns = client.connections.lock().await;
            let idx = client.next_idx.fetch_add(1, Ordering::Relaxed) % conns.len();
            let tx = conns[idx].tx.clone();
            drop(conns);

            handles.push(tokio::spawn(async move {
                let (reply_tx, reply_rx) = oneshot::channel();
                tx.send(Request {
                    data: vec![],
                    reply: reply_tx,
                })
                .await
                .unwrap();
                reply_rx.await.unwrap()
            }));
        }

        let mut ok = 0;
        for h in handles {
            if let Ok(Ok(_)) = h.await {
                ok += 1;
            }
        }
        let elapsed = start.elapsed();

        assert_eq!(ok, 20);
        println!("20 requests via 4 mock connections: {:?}", elapsed);
    }

    // ================================================================
    // 断开
    // ================================================================

    #[tokio::test]
    async fn test_disconnect_clears_pool() {
        let client = AsyncTdxHqClient::new();
        inject_mock(&client, spawn_mock_task(vec![])).await;

        assert_eq!(client.connection_count().await, 1);
        client.disconnect().await;
        assert_eq!(client.connection_count().await, 0);
    }

    #[tokio::test]
    async fn test_disconnect_then_send_fails() {
        let client = AsyncTdxHqClient::new();
        inject_mock(&client, spawn_mock_task(vec![])).await;
        client.disconnect().await;

        let result = client.get_security_count(1).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_closed_channel_detected() {
        let (tx, rx) = mpsc::channel::<Request>(16);
        drop(rx); // 关闭接收端

        let client = AsyncTdxHqClient::new();
        client.connections.lock().await.push(ConnectionHandle { tx });

        // 发送应失败 (通道已关闭)
        let result = client.get_security_count(1).await;
        assert!(result.is_err());
    }

    // ================================================================
    // TradingPhase
    // ================================================================

    #[test]
    fn test_detect_trading_phase_returns_valid() {
        let phase = utils::detect_trading_phase();
        assert!(matches!(
            phase,
            utils::TradingPhase::Trading
                | utils::TradingPhase::PrePost
                | utils::TradingPhase::Closed
        ));
    }

    #[test]
    fn test_client_set_phase() {
        let mut client = AsyncTdxHqClient::new();
        client.set_phase(TradingPhase::Closed);
        client.set_phase(TradingPhase::Trading);
    }

    #[test]
    fn test_client_auto_detect_phase() {
        let mut client = AsyncTdxHqClient::new();
        let phase = client.auto_detect_phase();
        assert!(matches!(
            phase,
            TradingPhase::Trading | TradingPhase::PrePost | TradingPhase::Closed
        ));
    }

    // ================================================================
    // Heartbeat
    // ================================================================

    #[tokio::test]
    async fn test_heartbeat_start_and_stop() {
        let client = AsyncTdxHqClient::new();
        // 注入一个 mock 连接 (心跳需要至少一个连接)
        inject_mock(&client, spawn_mock_task(vec![0x00; 16])).await;

        // 启动心跳
        client.start_heartbeat().await;
        assert!(client.is_connected());

        // 停止心跳
        client.stop_heartbeat().await;
        // 停止后 connected 状态不变 (仅 disconnect 会清除)
    }

    #[tokio::test]
    async fn test_heartbeat_marks_dead_on_closed_channel() {
        let client = AsyncTdxHqClient::new();

        // 注入一个已关闭的通道
        let (tx, rx) = mpsc::channel::<Request>(16);
        drop(rx);
        client.connections.lock().await.push(ConnectionHandle { tx });

        // 启动心跳 — 第一次心跳就会发现通道已关闭
        client.start_heartbeat().await;

        // 等待一个心跳周期 + 余量
        tokio::time::sleep(Duration::from_secs_f64(
            crate::protocol::constants::DEFAULT_HEARTBEAT_INTERVAL + 1.0,
        ))
        .await;

        assert!(!client.is_connected());
        client.stop_heartbeat().await;
    }

    #[tokio::test]
    async fn test_disconnect_stops_heartbeat() {
        let client = AsyncTdxHqClient::new();
        inject_mock(&client, spawn_mock_task(vec![0x00; 16])).await;

        client.start_heartbeat().await;
        assert!(client.is_connected());

        client.disconnect().await;
        assert!(!client.is_connected());
        assert_eq!(client.connection_count().await, 0);
    }
}
