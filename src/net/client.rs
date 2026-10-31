use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flate2::read::ZlibDecoder;
use std::io::Read;

use crate::error::{Result, TdxError};
use crate::error_codes::ErrorCode;
use crate::net::connection::TcpConnection;
use crate::net::packet::{ResponseHeader, RSP_HEADER_LEN};
use crate::net::pool::{ConnectionPool, PoolConfig, PoolStats};
use crate::net::utils::{self, RateLimiter};
use crate::protocol::constants::*;
use crate::protocol::parsers::*;
use crate::protocol::types::*;
use crate::{logd, logi, logw, loge};

/// 缓存条目
struct CacheEntry<T> {
    data: T,
    expires_at: Instant,
}

/// TDX 行情客户端
pub struct TdxHqClient {
    pool: Mutex<Arc<ConnectionPool>>,
    connected: Arc<AtomicBool>,
    auto_retry: AtomicBool,
    heartbeat_stop: Mutex<Option<Arc<AtomicBool>>>,
    heartbeat_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    last_server: Arc<Mutex<Option<(String, u16)>>>,
    count_cache: Mutex<HashMap<u8, CacheEntry<u16>>>,
    list_cache: Mutex<HashMap<u8, CacheEntry<Vec<SecurityInfo>>>>,
    cache_ttl: Mutex<Duration>,
    connect_timeout: Mutex<f64>,
    /// 用户自定义优先服务器列表 (为空时使用 PRIMARY_SERVERS)
    server_list: Mutex<Vec<(String, String, u16)>>,
    /// 黑名单服务器列表 (IP, 端口)
    blocked_servers: Mutex<Vec<(String, u16)>>,
    /// 速率限制器: 默认 (50 req/s)
    rate_limiter: RateLimiter,
    /// 速率限制器: 日K 级别 (15 req/s)
    rate_limiter_daily: RateLimiter,
    /// 速率限制器: 分时级别 (10 req/s, 不允许解禁)
    rate_limiter_minute: RateLimiter,
    /// 复权上下文数据量档位 (默认 Mid ≈ 20 年), 以 u8 存储
    fq_context_tier: AtomicU8,
    /// 握手模式 (Issue #12: Full=三步, SkipCmd3=跳过 cmd3), 以 u8 存储
    handshake_mode: Arc<AtomicU8>,
}

impl TdxHqClient {
    pub fn new() -> Self {
        let handshake_mode = Arc::new(AtomicU8::new(utils::HandshakeMode::Full as u8));
        let mode_for_closure = handshake_mode.clone();
        let mut config = PoolConfig::default();
        // 设置握手回调，让连接池自动为新连接执行握手
        config.handshake_fn = Some(Box::new(move |conn: &mut TcpConnection| -> Result<()> {
            let mode = utils::HandshakeMode::from_u8(mode_for_closure.load(Ordering::Relaxed));
            utils::perform_handshake_with_mode(conn, mode)
        }));
        let default_server = (
            PRIMARY_SERVERS[0].1.to_string(),
            PRIMARY_SERVERS[0].2,
        );
        Self {
            pool: Mutex::new(Arc::new(ConnectionPool::new_single(default_server, config))),
            connected: Arc::new(AtomicBool::new(false)),
            auto_retry: AtomicBool::new(true),
            heartbeat_stop: Mutex::new(None),
            heartbeat_handle: Mutex::new(None),
            last_server: Arc::new(Mutex::new(None)),
            count_cache: Mutex::new(HashMap::new()),
            list_cache: Mutex::new(HashMap::new()),
            cache_ttl: Mutex::new(Duration::from_secs(30)),
            connect_timeout: Mutex::new(CONNECT_TIMEOUT),
            server_list: Mutex::new(Vec::new()),
            blocked_servers: Mutex::new(Vec::new()),
            rate_limiter: RateLimiter::new(20),      // 默认 50 req/s
            rate_limiter_daily: RateLimiter::new(67), // 日K 15 req/s
            rate_limiter_minute: RateLimiter::new(100), // 分时 10 req/s (不允许解禁)
            fq_context_tier: AtomicU8::new(utils::FqContextTier::default() as u8),
            handshake_mode,
        }
    }

    /// 检查代码是否为板块代码 (88xxxx)，如果是则返回错误
    fn check_not_block_code(&self, code: &str) -> Result<()> {
        if crate::error_codes::is_block_code(code) {
            return Err(TdxError::coded(
                ErrorCode::BLOCK_CODE_IN_GENERAL_CLIENT,
                format!("code={}", code),
            ));
        }
        Ok(())
    }

    /// 获取当前握手模式
    pub fn handshake_mode(&self) -> utils::HandshakeMode {
        utils::HandshakeMode::from_u8(self.handshake_mode.load(Ordering::Relaxed))
    }

    /// 设置握手模式
    ///
    /// 默认 `HandshakeMode::Full`。若服务器拒绝行情数据 (返回空/K线异常)，
    /// 可切换为 `HandshakeMode::SkipCmd3` 跳过 SETUP_CMD3。
    pub fn set_handshake_mode(&self, mode: utils::HandshakeMode) {
        self.handshake_mode.store(mode as u8, Ordering::Relaxed);
    }

    /// 执行握手 (使用当前配置的模式)
    fn do_handshake(&self, conn: &mut TcpConnection) -> Result<()> {
        utils::perform_handshake_with_mode(conn, self.handshake_mode())
    }

    /// 连接到 TDX 服务器
    pub fn connect(&self, ip: &str, port: u16, timeout: Option<f64>) -> Result<bool> {
        self.connect_internal(ip, port, timeout, true)
    }

    /// 连接到任意可用服务器
    ///
    /// 遍历顺序: 上次成功服务器 → 用户自定义列表 → PRIMARY_SERVERS → ALL_KNOWN_SERVERS
    /// 黑名单中的服务器会被自动跳过 (v0.6.7)。
    ///
    /// ## 优化 (v0.6.7)
    ///
    /// - 优先尝试上次成功连接的服务器，避免每次都从头遍历
    /// - 已连接时直接返回，不重复连接
    pub fn connect_to_any(&self, timeout: Option<f64>) -> Result<bool> {
        // 0) 已连接时直接返回
        if self.connected.load(Ordering::SeqCst) {
            return Ok(true);
        }

        // 0.5) 优先尝试上次成功的服务器
        {
            let last = self.last_server.lock().unwrap();
            if let Some((ref ip, port)) = *last {
                if !self.is_server_blocked(ip, port) {
                    match self.connect_internal(ip, port, timeout, false) {
                        Ok(true) => return Ok(true),
                        _ => {}
                    }
                }
            }
        }

        // 1) 用户自定义列表
        {
            let list = self.server_list.lock().unwrap();
            for (_, ip, port) in list.iter() {
                if self.is_server_blocked(ip, *port) {
                    continue;
                }
                match self.connect_internal(ip, *port, timeout, false) {
                    Ok(true) => return Ok(true),
                    _ => continue,
                }
            }
        }

        // 2) PRIMARY_SERVERS (10台)
        for &(_, ip, port) in PRIMARY_SERVERS {
            if self.is_server_blocked(ip, port) {
                continue;
            }
            match self.connect_internal(ip, port, timeout, false) {
                Ok(true) => return Ok(true),
                _ => continue,
            }
        }

        // 3) ALL_KNOWN_SERVERS (101台, 兜底)
        for &(_, ip, port) in ALL_KNOWN_SERVERS {
            if self.is_server_blocked(ip, port) {
                continue;
            }
            match self.connect_internal(ip, port, timeout, false) {
                Ok(true) => return Ok(true),
                _ => continue,
            }
        }

        loge!("hq", "all servers unreachable (tried user/primary/all_known lists)");
        Err(crate::error_codes::ErrorCode::CONNECTION_FAILED.err(
            "all servers unreachable"
        ))
    }

    // ================================================================
    // 服务器列表管理
    // ================================================================

    /// 设置自定义优先服务器列表 (替换默认 PRIMARY_SERVERS)
    pub fn set_servers(&self, servers: &[(&str, &str, u16)]) {
        let mut list = self.server_list.lock().unwrap();
        list.clear();
        for &(name, ip, port) in servers {
            list.push((name.to_string(), ip.to_string(), port));
        }
    }

    /// 在优先列表头部添加一台服务器
    pub fn add_server(&self, name: &str, ip: &str, port: u16) {
        let mut list = self.server_list.lock().unwrap();
        list.insert(0, (name.to_string(), ip.to_string(), port));
    }

    /// 按响应时间重排优先服务器顺序 (最快在前)
    ///
    /// `sorted`: 由 probe_servers() 返回的排序结果, 取前N台
    pub fn reorder_servers(&self, sorted: &[(&str, &str, u16)]) {
        let mut list = self.server_list.lock().unwrap();
        list.clear();
        for &(name, ip, port) in sorted {
            list.push((name.to_string(), ip.to_string(), port));
        }
    }

    // ================================================================
    // 黑名单管理 (v0.6.7)
    // ================================================================

    /// 将服务器加入黑名单
    ///
    /// 黑名单中的服务器在 `connect_to_any` 和 `reconnect_to_another_server` 中被跳过。
    /// 适用于用户已知某台服务器在当前网络环境下不可用的情况。
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let client = TdxHqClient::new();
    /// client.block_server("183.60.224.177", 7709);  // 屏蔽广发13
    /// client.connect_to_any(None)?;  // 自动跳过黑名单服务器
    /// ```
    pub fn block_server(&self, ip: &str, port: u16) {
        let mut blocked = self.blocked_servers.lock().unwrap();
        if !blocked.iter().any(|(i, p)| i == ip && *p == port) {
            blocked.push((ip.to_string(), port));
            logi!("hq", "server {}:{} added to blacklist", ip, port);
        }
    }

    /// 从黑名单移除服务器
    pub fn unblock_server(&self, ip: &str, port: u16) {
        let mut blocked = self.blocked_servers.lock().unwrap();
        blocked.retain(|(i, p)| i != ip || *p != port);
        logi!("hq", "server {}:{} removed from blacklist", ip, port);
    }

    /// 获取黑名单列表
    pub fn blocked_servers(&self) -> Vec<(String, u16)> {
        self.blocked_servers.lock().unwrap().clone()
    }

    /// 清空黑名单
    pub fn clear_blocked_servers(&self) {
        self.blocked_servers.lock().unwrap().clear();
        logi!("hq", "server blacklist cleared");
    }

    /// 检查服务器是否在黑名单中
    fn is_server_blocked(&self, ip: &str, port: u16) -> bool {
        let blocked = self.blocked_servers.lock().unwrap();
        blocked.iter().any(|(i, p)| i == ip && *p == port)
    }

    /// 探测所有已知服务器, 返回按 API 响应时间升序排列的结果
    ///
    /// 返回: Vec<(名称, IP, 端口, TCP延迟ms, 握手延迟ms, API延迟ms)>
    /// 不修改当前优先列表。用户根据结果自行调用 reorder_servers()
    pub fn probe_servers(
        &self,
        timeout_secs: f64,
    ) -> Vec<(&'static str, &'static str, u16, f64, f64, f64)> {
        let mut results = Vec::new();
        let _timeout = Some(timeout_secs);

        for &(name, ip, port) in ALL_KNOWN_SERVERS {
            let tcp_start = Instant::now();
            let result = (|| -> std::result::Result<(f64, f64), TdxError> {
                let mut tcp = TcpConnection::connect(ip, port, timeout_secs)?;
                let tcp_ms = tcp_start.elapsed().as_secs_f64() * 1000.0;

                let hs_start = Instant::now();
                utils::perform_handshake_with_mode(&mut tcp, self.handshake_mode())?;
                let hs_ms = hs_start.elapsed().as_secs_f64() * 1000.0;

                // 简单 API (get_security_count)
                // 验证服务器可用 (发送 API 请求)
                let mut pkt = Vec::with_capacity(18);
                pkt.extend_from_slice(&[
                    0x0c, 0x0c, 0x18, 0x6c, 0x00, 0x01, 0x08, 0x00, 0x08, 0x00, 0x4e, 0x04,
                ]);
                pkt.extend_from_slice(&(1u16.to_le_bytes())); // market=SH
                pkt.extend_from_slice(&[0x75, 0xc7, 0x33, 0x01]);
                tcp.send(&pkt)?;
                let head = tcp.recv(RSP_HEADER_LEN)?;
                let h = ResponseHeader::parse(&head)?;
                let zs = h.zip_size as usize;
                let mut body = Vec::with_capacity(zs);
                while body.len() < zs {
                    body.extend_from_slice(&tcp.recv(zs - body.len())?);
                }
                Ok((tcp_ms, hs_ms))
            })();

            match result {
                Ok((tcp_ms, hs_ms)) => {
                    let api_ms = {
                        let mut conn = TcpConnection::connect(ip, port, timeout_secs).unwrap();
                        utils::perform_handshake_with_mode(&mut conn, self.handshake_mode()).unwrap();
                        let start = Instant::now();
                        let mut pkt = Vec::with_capacity(18);
                        pkt.extend_from_slice(&[
                            0x0c, 0x0c, 0x18, 0x6c, 0x00, 0x01, 0x08, 0x00, 0x08, 0x00, 0x4e, 0x04,
                        ]);
                        pkt.extend_from_slice(&(1u16.to_le_bytes()));
                        pkt.extend_from_slice(&[0x75, 0xc7, 0x33, 0x01]);
                        conn.send(&pkt).unwrap();
                        let head = conn.recv(RSP_HEADER_LEN).unwrap();
                        let h = ResponseHeader::parse(&head).unwrap();
                        let zs = h.zip_size as usize;
                        let mut body = Vec::with_capacity(zs);
                        while body.len() < zs {
                            body.extend_from_slice(&conn.recv(zs - body.len()).unwrap());
                        }
                        start.elapsed().as_secs_f64() * 1000.0
                    };
                    results.push((name, ip, port, tcp_ms, hs_ms, api_ms));
                }
                Err(_) => continue,
            }
        }

        results.sort_by(|a, b| a.5.partial_cmp(&b.5).unwrap());
        results
    }

    fn connect_internal(
        &self,
        ip: &str,
        port: u16,
        timeout: Option<f64>,
        start_heartbeat: bool,
    ) -> Result<bool> {
        let timeout_secs = timeout.unwrap_or(*self.connect_timeout.lock().unwrap());

        // 建立连接并执行握手
        let mut tcp = TcpConnection::connect(ip, port, timeout_secs)?;
        self.do_handshake(&mut tcp)?;

        // 握手成功，复用现有连接池 (保持心跳线程引用有效)
        let server = (ip.to_string(), port);
        {
            let pool_guard = self.pool.lock().unwrap();
            pool_guard.close_all();  // 关闭旧连接
            pool_guard.push(tcp, server.clone());  // 放入新连接
        }

        self.connected.store(true, Ordering::SeqCst);
        *self.last_server.lock().unwrap() = Some(server);

        // 清除缓存
        self.count_cache.lock().unwrap().clear();
        self.list_cache.lock().unwrap().clear();

        if start_heartbeat {
            self.start_heartbeat();
        }
        logi!("hq", "connected to {}:{}", ip, port);
        Ok(true)
    }

    /// 断开连接
    pub fn disconnect(&self) {
        self.stop_heartbeat();
        self.pool.lock().unwrap().close_all();
        self.connected.store(false, Ordering::SeqCst);
        logi!("hq", "disconnected");
    }

    /// 是否已连接
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// 设置是否自动重试
    pub fn set_auto_retry(&self, enabled: bool) {
        self.auto_retry.store(enabled, Ordering::SeqCst);
    }

    /// 设置缓存 TTL (秒)
    pub fn set_cache_ttl(&self, ttl_secs: u64) {
        *self.cache_ttl.lock().unwrap() = Duration::from_secs(ttl_secs);
    }

    /// 设置连接超时 (秒)
    pub fn set_connect_timeout(&self, timeout: f64) {
        *self.connect_timeout.lock().unwrap() = timeout;
    }

    /// 获取连接池状态
    pub fn pool_stats(&self) -> PoolStats {
        self.pool.lock().unwrap().stats()
    }

    /// 设置默认请求速率限制 (每秒请求数, 0=禁用)
    ///
    /// 默认 50 req/s。
    pub fn set_rate_limit(&self, rps: u32) {
        self.rate_limiter.set_rps(rps);
    }

    /// 设置日K 级别速率限制 (每秒请求数, 0=禁用)
    ///
    /// 默认 15 req/s。影响: get_security_bars / get_index_bars (category >= 4)
    pub fn set_rate_limit_daily(&self, rps: u32) {
        self.rate_limiter_daily.set_rps(rps);
    }

    /// 获取分时级别速率限制 (固定 10 req/s, 不可修改)
    pub fn rate_limit_minute(&self) -> u32 {
        10
    }

    // ================================================================
    // Heartbeat
    // ================================================================

    fn start_heartbeat(&self) {
        self.stop_heartbeat();
        let stop = Arc::new(AtomicBool::new(false));
        let pool = Arc::clone(&self.pool.lock().unwrap());
        let connected = Arc::clone(&self.connected);
        let stop_clone = stop.clone();
        let last_server = Arc::clone(&self.last_server);
        let interval = Duration::from_secs_f64(DEFAULT_HEARTBEAT_INTERVAL);
        let connect_timeout = *self.connect_timeout.lock().unwrap();
        let handshake_mode = Arc::clone(&self.handshake_mode);

        let handle = std::thread::spawn(move || {
            while !stop_clone.load(Ordering::Relaxed) {
                std::thread::sleep(interval);
                if stop_clone.load(Ordering::Relaxed) {
                    break;
                }

                // 尝试从池中借出连接做心跳
                let current_server = last_server.lock().unwrap().clone()
                    .unwrap_or_else(|| (PRIMARY_SERVERS[0].1.to_string(), PRIMARY_SERVERS[0].2));
                if let Ok(Some(mut guard)) = pool.try_borrow(&current_server) {
                    let alive = (|| -> bool {
                        let conn = guard.conn();
                        let mut packet = Vec::with_capacity(18);
                        packet.extend_from_slice(&[
                            0x0c, 0x0c, 0x18, 0x6c, 0x00, 0x01, 0x08, 0x00, 0x08, 0x00, 0x4e,
                            0x04,
                        ]);
                        packet.extend_from_slice(&0u16.to_le_bytes());
                        packet.extend_from_slice(&[0x75, 0xc7, 0x33, 0x01]);

                        if conn.send(&packet).is_err() {
                            return false;
                        }
                        let head = match conn.recv(RSP_HEADER_LEN) {
                            Ok(h) => h,
                            Err(_) => return false,
                        };
                        let header = match ResponseHeader::parse(&head) {
                            Ok(h) => h,
                            Err(_) => return false,
                        };
                        let zip_size = header.zip_size as usize;
                        let mut body = Vec::with_capacity(zip_size);
                        while body.len() < zip_size {
                            match conn.recv(zip_size - body.len()) {
                                Ok(chunk) => body.extend_from_slice(&chunk),
                                Err(_) => return false,
                            }
                        }
                        true
                    })();

                    if !alive {
                        // 心跳失败: 标记断线 + 关闭池中空闲连接
                        connected.store(false, Ordering::SeqCst);
                        pool.close_all();
                        logw!("hq", "heartbeat failed, pool cleared, attempting reconnect...");

                        // 尝试重连到替代服务器 (跳过当前失败的服务器)
                        let mut reconnected = false;
                        for &(name, ip, port) in PRIMARY_SERVERS {
                            if ip == current_server.0 && port == current_server.1 {
                                continue; // 跳过当前失败的服务器
                            }
                            match TcpConnection::connect(ip, port, connect_timeout) {
                                Ok(mut tcp) => {
                                    let mode = utils::HandshakeMode::from_u8(
                                        handshake_mode.load(Ordering::Relaxed),
                                    );
                                    if utils::perform_handshake_with_mode(&mut tcp, mode).is_ok() {
                                        let new_server = (ip.to_string(), port);
                                        pool.push(tcp, new_server.clone());
                                        *last_server.lock().unwrap() = Some(new_server);
                                        connected.store(true, Ordering::SeqCst);
                                        logi!("hq", "heartbeat reconnect to {} ({})", name, ip);
                                        reconnected = true;
                                        break;
                                    }
                                }
                                Err(_) => continue,
                            }
                        }
                        if !reconnected {
                            loge!("hq", "heartbeat reconnect failed, all PRIMARY servers unreachable");
                        }
                    }
                }
            }
        });

        *self.heartbeat_stop.lock().unwrap() = Some(stop);
        *self.heartbeat_handle.lock().unwrap() = Some(handle);
        self.connected.store(true, Ordering::SeqCst);
    }

    fn stop_heartbeat(&self) {
        if let Some(stop) = self.heartbeat_stop.lock().unwrap().take() {
            stop.store(true, Ordering::SeqCst);
        }
        if let Some(h) = self.heartbeat_handle.lock().unwrap().take() {
            let _ = h.join();
        }
    }

    // ================================================================
    // Internal send/recv with retry
    // ================================================================

    /// 发送请求并接收响应 (默认限流)
    fn send_and_recv(&self, packet: &[u8]) -> Result<Vec<u8>> {
        self.send_and_recv_limited(packet, &self.rate_limiter)
    }

    /// 发送请求并接收响应 (指定限流器)
    fn send_and_recv_limited(&self, packet: &[u8], limiter: &RateLimiter) -> Result<Vec<u8>> {
        // 限流
        limiter.wait();

        // 第一次尝试
        match self.try_send_and_recv(packet) {
            Ok(body) => return Ok(body),
            Err(e) if !self.auto_retry.load(Ordering::SeqCst) => return Err(e),
            Err(_) => {}
        }

        // 重试
        for (i, &interval) in RETRY_INTERVALS.iter().enumerate() {
            logw!("hq", "request failed, retry {}/{} in {:.1}s", i + 1, RETRY_INTERVALS.len(), interval);
            std::thread::sleep(Duration::from_secs_f64(interval));

            // 尝试重连
            self.reconnect_if_needed();

            // 重试请求
            match self.try_send_and_recv(packet) {
                Ok(body) => return Ok(body),
                Err(_) => continue,
            }
        }

        loge!("hq", "retry exhausted after {} attempts", RETRY_INTERVALS.len() + 1);
        Err(crate::error_codes::ErrorCode::RETRY_EXHAUSTED.err(
            format!("{} attempts", RETRY_INTERVALS.len() + 1)
        ))
    }

    /// 从连接池借出连接并执行请求
    fn try_send_and_recv(&self, packet: &[u8]) -> Result<Vec<u8>> {
        let server = self.last_server.lock().unwrap().clone()
            .unwrap_or_else(|| (PRIMARY_SERVERS[0].1.to_string(), PRIMARY_SERVERS[0].2));
        let pool = self.pool.lock().unwrap();
        let mut guard = pool.borrow(&server)?;

        let conn = guard.conn();

        // Send
        conn.send(packet)?;

        // Read response header
        let head_buf = conn.recv(RSP_HEADER_LEN)?;
        let header = ResponseHeader::parse(&head_buf)?;

        // Read body
        let zip_size = header.zip_size as usize;
        let mut body_buf = Vec::with_capacity(zip_size);
        while body_buf.len() < zip_size {
            let remaining = zip_size - body_buf.len();
            let chunk = conn.recv(remaining)?;
            body_buf.extend_from_slice(&chunk);
        }

        if body_buf.is_empty() {
            return Err(ErrorCode::DISCONNECTED.err("empty response body"));
        }

        // Decompress if needed
        if header.zip_size != header.unzip_size {
            let mut decoder = ZlibDecoder::new(&body_buf[..]);
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed).map_err(|e| {
                ErrorCode::DECOMPRESS_FAILED.err(format!("{}", e))
            })?;
            Ok(decompressed)
        } else {
            Ok(body_buf)
        }
    }

    /// 发送原始包并接收响应 (公共方法，供扩展模块使用)
    ///
    /// # 参数
    /// * `packet` - 原始请求包
    ///
    /// # 返回
    /// 响应体数据
    ///
    /// # 示例
    ///
    /// ```rust
    /// let mut client = TdxHqClient::new();
    /// client.connect()?;
    ///
    /// // 发送自定义请求
    /// let response = client.send_raw_and_recv(&custom_packet)?;
    /// ```
    pub fn send_raw_and_recv(&self, packet: &[u8]) -> Result<Vec<u8>> {
        self.send_and_recv(packet)
    }

    /// 尝试重连
    ///
    /// 策略: 先试上次服务器 (可能临时故障已恢复)，失败则跳过它尝试替代服务器。
    /// 心跳线程可能已经重连成功 (更新了 pool 和 connected)，此时直接返回。
    fn reconnect_if_needed(&self) {
        if self.connected.load(Ordering::SeqCst) {
            return;
        }

        logw!("hq", "connection lost, attempting reconnect...");

        let last = self.last_server.lock().unwrap().clone();

        // 1) 先试上次服务器 (可能临时故障已恢复)
        if let Some((ref ip, port)) = last {
            if self.connect_internal(ip, port, Some(CONNECT_TIMEOUT), false).is_ok() {
                return;
            }
        }

        // 2) 跳过失败的服务器，尝试替代服务器
        let skip = last.as_ref().map(|(ip, port)| (ip.as_str(), *port));

        // 用户自定义列表
        {
            let list = self.server_list.lock().unwrap();
            for (_, ip, port) in list.iter() {
                if Some((ip.as_str(), *port)) == skip { continue; }
                if self.connect_internal(ip, *port, Some(CONNECT_TIMEOUT), false).is_ok() {
                    return;
                }
            }
        }
        // PRIMARY (跳过失败的)
        for &(_, ip, port) in PRIMARY_SERVERS {
            if Some((ip, port)) == skip { continue; }
            if self.connect_internal(ip, port, Some(CONNECT_TIMEOUT), false).is_ok() {
                return;
            }
        }
        // ALL_KNOWN (跳过失败的)
        for &(_, ip, port) in ALL_KNOWN_SERVERS {
            if Some((ip, port)) == skip { continue; }
            if self.connect_internal(ip, port, Some(CONNECT_TIMEOUT), false).is_ok() {
                return;
            }
        }

        loge!("hq", "reconnect failed, all servers unreachable");
    }

    // ================================================================
    // API Methods
    // ================================================================

    /// 获取K线数据
    ///
    /// `fq`: 复权类型, 0=未复权 1=前复权(默认) 2=后复权
    /// (v0.4.1: fq>0 时自动获取除权信息并客户端侧计算复权)
    ///
    /// ## 空响应自动重试 (v0.6.7)
    ///
    /// 当请求日K线 (category >= 4) 返回空数据时，自动尝试切换服务器重试。
    /// 这解决了部分服务器 (如海通、广发) 握手正常但 K 线返回空的问题。
    /// 最多重试 2 次 (共 3 次请求)，确保单台服务器故障不影响数据获取。
    pub fn get_security_bars(
        &self,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
        fq: u8,
    ) -> Result<Vec<SecurityBar>> {
        self.check_not_block_code(code)?;

        // 日K线空响应自动重试 (仅对日K及以上周期，分钟线不重试)
        let should_retry_empty = category >= 4;
        let max_retry = if should_retry_empty { 3 } else { 1 };

        let mut bars = Vec::new();
        let mut retry_count = 0;

        for attempt in 0..max_retry {
            let packet = utils::build_security_bars_packet(category, market, code, start, count, fq);
            let body = self.send_and_recv_limited(&packet, &self.rate_limiter_daily)?;
            let parsed = parse_security_bars(&body, category)?;

            if !parsed.is_empty() || !should_retry_empty {
                bars = parsed;
                break;
            }

            // 空响应，尝试切换服务器 (debug 级别，减少噪音)
            retry_count += 1;
            logd!("hq", "attempt {}/{}: empty K-line for {}, switching server",
                  attempt + 1, max_retry, code);

            if attempt < max_retry - 1 {
                self.reconnect_to_another_server();
            }
        }

        // 仅在所有重试都失败时输出警告
        if bars.is_empty() && should_retry_empty && retry_count > 0 {
            logw!("hq", "all {} attempts returned empty K-line for {}", max_retry, code);
        } else if retry_count > 0 && !bars.is_empty() {
            // 重试成功，输出信息级别
            logi!("hq", "got {} bars for {} after {} server switch(es)", bars.len(), code, retry_count);
        }

        // 客户端侧复权计算 (v0.4.2)
        if fq != 0 && !bars.is_empty() {
            if let Ok(xdxr) = self.get_xdxr_info(market, code) {
                use crate::protocol::adjuster::{adjust_security_bars, FqType};
                let fq_enum = match fq {
                    2 => FqType::Hfq,
                    _ => FqType::Qfq,
                };
                let context = self.fetch_context_bars_for_adjust(category, market, code, &bars, &xdxr);
                adjust_security_bars(&mut bars, &context, &xdxr, fq_enum);
            }
        }

        Ok(bars)
    }

    /// 尝试切换到另一台服务器
    ///
    /// 遍历 PRIMARY_SERVERS，跳过当前服务器和黑名单，连接到第一台可用的。
    fn reconnect_to_another_server(&self) {
        let current = self.last_server.lock().unwrap().clone();

        for &(_, ip, port) in PRIMARY_SERVERS {
            // 跳过当前服务器
            if let Some((ref cur_ip, cur_port)) = current {
                if ip == cur_ip && port == cur_port {
                    continue;
                }
            }

            // 跳过黑名单服务器
            if self.is_server_blocked(ip, port) {
                continue;
            }

            if self.connect_internal(ip, port, Some(CONNECT_TIMEOUT), false).is_ok() {
                logi!("hq", "switched to server {}:{}", ip, port);
                return;
            }
        }

        logw!("hq", "no alternative server available");
    }

    /// 为复权计算获取额外的历史 K 线上下文
    ///
    /// 当除权除息事件早于请求的 K 线数据范围时,
    /// 需要额外的历史数据来计算这些早期事件的复权因子。
    /// 为复权计算获取历史 K 线上下文
    fn fetch_context_bars_for_adjust(
        &self,
        category: u8,
        market: u8,
        code: &str,
        bars: &[SecurityBar],
        xdxr: &[XdXrInfo],
    ) -> Vec<SecurityBar> {
        utils::fetch_context_bars_for_adjust_with_tier(
            |pkt| self.send_and_recv(pkt),
            category, market, code, bars, xdxr,
            self.fq_context_tier(),
        )
    }

    /// 设置复权上下文数据量档位
    ///
    /// 控制复权计算时拉取的历史 K 线数量:
    /// - `Low`: 约 10 年 (2400 根)
    /// - `Mid`: 约 20 年 (4800 根, 默认)
    /// - `High`: 约 30 年 (7200 根)
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

    /// 获取复权因子计算所需的上下文数据 (追溯到上市)
    ///
    /// 与 `fetch_context_bars_for_adjust` 类似，但会持续拉取直到:
    /// 1. 覆盖所有 XDXR 事件，或
    /// 2. 达到最大页数限制 (30 页 = 24000 根 ≈ 96 年)
    ///
    /// 用于 `calc_fq_factors` 接口，确保因子计算的完整性。
    pub fn fetch_context_for_factors(
        &self,
        category: u8,
        market: u8,
        code: &str,
        bars: &[SecurityBar],
        xdxr: &[XdXrInfo],
    ) -> Result<Vec<SecurityBar>> {
        if bars.is_empty() || xdxr.is_empty() {
            return Ok(Vec::new());
        }

        // 找到最早的除权事件
        let earliest_event = xdxr
            .iter()
            .filter(|x| x.category == 1)
            .map(|x| x.year as u32 * 10000 + x.month as u32 * 100 + x.day as u32)
            .min();

        let Some(ee_date) = earliest_event else { return Ok(Vec::new()) };

        // 检查是否需要上下文
        let first_bar_date =
            bars[0].year as u32 * 10000 + bars[0].month as u32 * 100 + bars[0].day as u32;

        if first_bar_date <= ee_date {
            return Ok(Vec::new());
        }

        // 持续拉取直到覆盖最早事件或达到上限 (30 页)
        let max_per_page = MAX_KLINE_COUNT as u32;
        let max_pages = 30u32; // 约 96 年，足够覆盖任何 A 股上市时间
        let mut context = Vec::new();
        let mut offset = max_per_page;

        for _page in 0..max_pages {
            let pkt = utils::build_security_bars_packet(
                category, market, code, offset, MAX_KLINE_COUNT, 0,
            );
            let body = match self.send_and_recv(&pkt) {
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

            // 找到覆盖最早事件的数据就停止
            if batch_first_date <= ee_date {
                break;
            }

            offset += max_per_page;
            if context.len() == len_before {
                break;
            }
        }

        Ok(context)
    }

    /// 获取K线数据 (自动分页)
    ///
    /// 当 count > MAX_KLINE_COUNT 时自动分页请求并合并结果
    pub fn get_security_bars_all(
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
            let bars = self.get_security_bars(category, market, code, offset, batch, fq)?;
            if bars.is_empty() {
                break;
            }
            let fetched = bars.len() as u16;
            // TDX 返回升序(oldest→newest)，翻页是更早数据，prepend 保持整体升序
            all_bars.splice(0..0, bars);
            remaining = remaining.saturating_sub(fetched);
            offset += fetched as u32;

            if fetched < batch {
                break; // 没有更多数据
            }
        }

        Ok(all_bars)
    }

    /// 获取指数K线
    ///
    /// `fq`: 复权类型, 0=未复权 1=前复权(默认) 2=后复权
    /// 获取指数K线 — 指数不存在复权概念，fq 参数保留接口一致性但不生效
    pub fn get_index_bars(
        &self,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
        fq: u8,
    ) -> Result<Vec<IndexBar>> {
        self.check_not_block_code(code)?;
        let _ = fq; // 指数不复权，强制 fq=0 发送

        // 日K线空响应自动重试 (与 get_security_bars 一致)
        let should_retry_empty = category >= 4;
        let max_retry = if should_retry_empty { 3 } else { 1 };

        let mut bars = Vec::new();
        let mut retry_count = 0;

        for attempt in 0..max_retry {
            let packet = utils::build_index_bars_packet(category, market, code, start, count, 0);
            let body = self.send_and_recv_limited(&packet, &self.rate_limiter_daily)?;
            let parsed = parse_index_bars(&body, category)?;

            if !parsed.is_empty() || !should_retry_empty {
                bars = parsed;
                break;
            }

            retry_count += 1;
            logd!("hq", "attempt {}/{}: empty index K-line for {}, switching server",
                  attempt + 1, max_retry, code);

            if attempt < max_retry - 1 {
                self.reconnect_to_another_server();
            }
        }

        if bars.is_empty() && should_retry_empty && retry_count > 0 {
            logw!("hq", "all {} attempts returned empty index K-line for {}", max_retry, code);
        } else if retry_count > 0 && !bars.is_empty() {
            logi!("hq", "got {} index bars for {} after {} server switch(es)", bars.len(), code, retry_count);
        }

        Ok(bars)
    }

    /// 获取指数K线 (自动分页)
    pub fn get_index_bars_all(
        &self,
        category: u8,
        market: u8,
        code: &str,
        count: u16,
        fq: u8,
    ) -> Result<Vec<IndexBar>> {
        let mut all_bars = Vec::new();
        let mut offset = 0u32;
        let mut remaining = count;

        while remaining > 0 {
            let batch = remaining.min(MAX_KLINE_COUNT);
            let bars = self.get_index_bars(category, market, code, offset, batch, fq)?;
            if bars.is_empty() {
                break;
            }
            let fetched = bars.len() as u16;
            // TDX 返回升序(oldest→newest)，翻页是更早数据，prepend 保持整体升序
            all_bars.splice(0..0, bars);
            remaining = remaining.saturating_sub(fetched);
            offset += fetched as u32;

            if fetched < batch {
                break;
            }
        }

        Ok(all_bars)
    }

    /// 获取实时行情
    ///
    /// 单次查询上限 60 只 (TDX 服务端硬限制)，超出自动截断并打印警告。
    /// 如需查询更多，请自行分组调用后合并结果。
    pub fn get_security_quotes(
        &self,
        all_stock: &[(u8, &str)],
    ) -> Result<Vec<SecurityQuote>> {
        // 服务端上限截断
        let all_stock = if all_stock.len() > MAX_QUOTES_COUNT {
            logw!("hq", "批量行情查询超过上限 {}/{}，自动截断。请自行分组调用。",
                  all_stock.len(), MAX_QUOTES_COUNT);
            &all_stock[..MAX_QUOTES_COUNT]
        } else {
            all_stock
        };
        // 检查是否有板块代码
        for &(_, code) in all_stock {
            self.check_not_block_code(code)?;
        }

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

            let body = self.send_and_recv(&packet)?;
            let quotes = parse_security_quotes(&body)?;

            if !quotes.is_empty() {
                if retry_count > 0 {
                    logi!("hq", "got {} quotes after {} server switch(es)", quotes.len(), retry_count);
                }
                return Ok(quotes);
            }

            // 空响应，尝试切换服务器
            retry_count += 1;
            logd!("hq", "attempt {}/{}: empty quotes, switching server",
                  attempt + 1, max_retry);

            if attempt < max_retry - 1 {
                self.reconnect_to_another_server();
            }
        }

        logw!("hq", "all {} attempts returned empty quotes", max_retry);
        Ok(Vec::new())
    }

    /// 获取证券列表 (带缓存)
    pub fn get_security_list(&self, market: u8, start: u16) -> Result<Vec<SecurityInfo>> {
        if start == 0 {
            let cache = self.list_cache.lock().unwrap();
            if let Some(entry) = cache.get(&market) {
                if Instant::now() < entry.expires_at {
                    return Ok(entry.data.clone());
                }
            }
            drop(cache);

            let mut packet = Vec::with_capacity(16);
            packet.extend_from_slice(&[
                0x0c, 0x01, 0x18, 0x64, 0x01, 0x01, 0x06, 0x00, 0x06, 0x00, 0x50, 0x04,
            ]);
            packet.extend_from_slice(&(market as u16).to_le_bytes());
            packet.extend_from_slice(&start.to_le_bytes());

            let body = self.send_and_recv(&packet)?;
            let result = parse_security_list(&body)?;

            let mut cache = self.list_cache.lock().unwrap();
            let ttl = *self.cache_ttl.lock().unwrap();
            cache.insert(
                market,
                CacheEntry {
                    data: result.clone(),
                    expires_at: Instant::now() + ttl,
                },
            );
            return Ok(result);
        }

        let mut packet = Vec::with_capacity(16);
        packet.extend_from_slice(&[
            0x0c, 0x01, 0x18, 0x64, 0x01, 0x01, 0x06, 0x00, 0x06, 0x00, 0x50, 0x04,
        ]);
        packet.extend_from_slice(&(market as u16).to_le_bytes());
        packet.extend_from_slice(&start.to_le_bytes());

        let body = self.send_and_recv(&packet)?;
        parse_security_list(&body)
    }

    /// 获取证券数量 (带缓存)
    pub fn get_security_count(&self, market: u8) -> Result<u16> {
        let cache = self.count_cache.lock().unwrap();
        if let Some(entry) = cache.get(&market) {
            if Instant::now() < entry.expires_at {
                return Ok(entry.data);
            }
        }
        drop(cache);

        let mut packet = Vec::with_capacity(18);
        packet.extend_from_slice(&[
            0x0c, 0x0c, 0x18, 0x6c, 0x00, 0x01, 0x08, 0x00, 0x08, 0x00, 0x4e, 0x04,
        ]);
        packet.extend_from_slice(&(market as u16).to_le_bytes());
        packet.extend_from_slice(&[0x75, 0xc7, 0x33, 0x01]);

        let body = self.send_and_recv(&packet)?;
        let count = parse_security_count(&body)?;

        let mut cache = self.count_cache.lock().unwrap();
        let ttl = *self.cache_ttl.lock().unwrap();
        cache.insert(
            market,
            CacheEntry {
                data: count,
                expires_at: Instant::now() + ttl,
            },
        );
        Ok(count)
    }

    /// 获取当日分时数据
    ///
    /// 内部委托给历史分时 API (传入今日日期)，避免实时分时 API (0x051d)
    /// 的价格编码异常（基金类价格 1000x 偏高）。
    pub fn get_minute_time_data(
        &self,
        market: u8,
        code: &str,
    ) -> Result<Vec<MinuteTimePrice>> {
        let today = utils::today_yyyymmdd();
        self.get_history_minute_time_data(market, code, today)
    }

    /// 获取历史分时数据
    pub fn get_history_minute_time_data(
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

        let body = self.send_and_recv_limited(&packet, &self.rate_limiter_minute)?;
        parse_history_minute_time_data(&body, market, code)
    }

    /// 获取逐笔成交
    pub fn get_transaction_data(
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

        let body = self.send_and_recv_limited(&packet, &self.rate_limiter_minute)?;
        let coefficient = get_security_coefficient(market, code);
        parse_transaction_data_with_coefficient(&body, coefficient)
    }

    /// 获取历史逐笔成交
    pub fn get_history_transaction_data(
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

        let body = self.send_and_recv_limited(&packet, &self.rate_limiter_minute)?;
        let coefficient = get_security_coefficient(market, code);
        parse_transaction_data_with_coefficient(&body, coefficient)
    }

    /// 获取财务信息
    pub fn get_finance_info(&self, market: u8, code: &str) -> Result<FinanceInfo> {
        let code_buf = utils::code_bytes(code);
        let mut packet = Vec::with_capacity(21);
        packet.extend_from_slice(&[
            0x0c, 0x1f, 0x18, 0x76, 0x00, 0x01, 0x0b, 0x00, 0x0b, 0x00, 0x10, 0x00, 0x01, 0x00,
        ]);
        packet.push(market);
        packet.extend_from_slice(&code_buf);

        let body = self.send_and_recv(&packet)?;
        parse_finance_info(&body, market, code)
    }

    /// 获取除权除息
    pub fn get_xdxr_info(&self, market: u8, code: &str) -> Result<Vec<XdXrInfo>> {
        let code_buf = utils::code_bytes(code);
        let mut packet = Vec::with_capacity(21);
        packet.extend_from_slice(&[
            0x0c, 0x1f, 0x18, 0x76, 0x00, 0x01, 0x0b, 0x00, 0x0b, 0x00, 0x0f, 0x00, 0x01, 0x00,
        ]);
        packet.push(market);
        packet.extend_from_slice(&code_buf);

        let body = self.send_and_recv(&packet)?;
        parse_xdxr_info(&body)
    }

    /// 获取板块元数据
    pub fn get_block_info_meta(&self, block_file: &str) -> Result<BlockInfoMeta> {
        let mut name_buf = [0u8; 40];
        let bytes = block_file.as_bytes();
        let len = bytes.len().min(40);
        name_buf[..len].copy_from_slice(&bytes[..len]);

        let mut packet = Vec::with_capacity(52);
        packet.extend_from_slice(&[
            0x0C, 0x39, 0x18, 0x69, 0x00, 0x01, 0x2A, 0x00, 0x2A, 0x00, 0xC5, 0x02,
        ]);
        packet.extend_from_slice(&name_buf);

        let body = self.send_and_recv(&packet)?;
        parse_block_info_meta(&body)
    }

    /// 获取板块数据
    pub fn get_block_info(
        &self,
        block_file: &str,
        start: u32,
        size: u32,
    ) -> Result<Vec<u8>> {
        let mut name_buf = [0u8; 100];
        let bytes = block_file.as_bytes();
        let len = bytes.len().min(100);
        name_buf[..len].copy_from_slice(&bytes[..len]);

        let mut packet = Vec::with_capacity(120);
        packet.extend_from_slice(&[
            0x0c, 0x37, 0x18, 0x6a, 0x00, 0x01, 0x6e, 0x00, 0x6e, 0x00, 0xb9, 0x06,
        ]);
        packet.extend_from_slice(&start.to_le_bytes());
        packet.extend_from_slice(&size.to_le_bytes());
        packet.extend_from_slice(&name_buf);

        let body = self.send_and_recv(&packet)?;
        parse_block_info(&body)
    }

    /// 获取并解析板块信息
    pub fn get_and_parse_block_info(
        &self,
        block_file: &str,
    ) -> Result<Vec<crate::reader::block::BlockRecord>> {
        let meta = self.get_block_info_meta(block_file)?;
        let chunk_size: u32 = 0x7530;
        let mut all_data = Vec::new();
        let mut offset = 0u32;

        while offset < meta.size {
            let read_size = chunk_size.min(meta.size - offset);
            let chunk = self.get_block_info(block_file, offset, read_size)?;
            all_data.extend_from_slice(&chunk);
            offset += read_size;
        }

        crate::reader::block::parse_block(&all_data)
    }
}
