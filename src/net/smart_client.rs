//! 智能连接客户端 — 分层健康检查 + 本地缓存
//!
//! 与 `TdxHqClient` 相同的 API，但采用不同的连接策略:
//! - **快速初始连接**: 仅验证 TCP + 握手，不做 K 线健康检查
//! - **惰性健康检查**: 首次 K 线请求返回空时触发，自动切换服务器
//! - **本地缓存**: 记录成功/失败服务器，下次连接优先使用缓存
//! - **黑名单机制**: 连续失败的服务器自动加入黑名单 (24h 过期)
//! - **真实 K 线探测** (v0.6.9): `probe_and_cache` 拉取一根日K验证数据，
//!   握手成功但 K 线为空的服务器记为失败 (Issue #12 对齐)
//!
//! ## 使用场景
//!
//! - 网络环境不稳定，部分服务器对当前用户不可用
//! - 需要快速初始化连接，首次 K 线请求可能需要重试
//! - 长期运行，需要自动适应服务器状态变化
//!
//! ## 与 TdxHqClient 对比
//!
//! | 维度 | TdxHqClient | TdxSmartClient |
//! |------|-------------|----------------|
//! | 初始连接 | 无健康检查 | 无健康检查 |
//! | K 线请求 | 直接返回 | 返回空时自动重试 |
//! | 服务器缓存 | 无 | 本地 JSON 缓存 |
//! | 黑名单 | 无 | 自动标记失败服务器 |
//! | 适用场景 | 网络稳定 | 网络不稳定 |
//!
//! ## 注意事项
//!
//! - 首次使用时无缓存，行为与 TdxHqClient 相同
//! - 缓存文件位于 `~/.tdxrs/server_cache.json`
//! - 黑名单有效期 24h，过期后自动重试

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::error::Result;
use crate::error_codes::ErrorCode;
use crate::net::client::TdxHqClient;
use crate::net::utils;
use crate::protocol::constants::*;
use crate::protocol::types::*;
use crate::{logi, logw, loge};

// ================================================================
// 服务器缓存
// ================================================================

/// 服务器缓存条目
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ServerCacheEntry {
    ip: String,
    port: u16,
    name: String,
    timestamp: u64,
    latency_ms: u32,
}

/// 黑名单条目
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct BlacklistEntry {
    ip: String,
    port: u16,
    reason: String,
    timestamp: u64,
}

/// 服务器统计
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ServerStats {
    success: u32,
    fail: u32,
    avg_latency: u32,
}

/// 服务器缓存文件结构
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ServerCache {
    version: u32,
    last_success: Option<ServerCacheEntry>,
    blacklist: Vec<BlacklistEntry>,
    server_stats: HashMap<String, ServerStats>,
}

impl ServerCache {
    fn new() -> Self {
        Self {
            version: 1,
            last_success: None,
            blacklist: Vec::new(),
            server_stats: HashMap::new(),
        }
    }

    /// 获取缓存文件路径
    fn cache_path() -> PathBuf {
        // 优先使用 HOME 环境变量，否则使用当前目录
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        home.join(".tdxrs").join("server_cache.json")
    }

    /// 加载缓存
    fn load() -> Self {
        let path = Self::cache_path();
        match fs::read_to_string(&path) {
            Ok(content) => {
                match serde_json::from_str(&content) {
                    Ok(cache) => cache,
                    Err(e) => {
                        logw!("cache", "failed to parse cache: {}", e);
                        Self::new()
                    }
                }
            }
            Err(_) => Self::new(),
        }
    }

    /// 保存缓存
    fn save(&self) {
        let path = Self::cache_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(self) {
            Ok(content) => {
                if let Err(e) = fs::write(&path, content) {
                    logw!("cache", "failed to save cache: {}", e);
                }
            }
            Err(e) => {
                logw!("cache", "failed to serialize cache: {}", e);
            }
        }
    }

    /// 检查服务器是否在黑名单中
    fn is_blacklisted(&self, ip: &str, port: u16) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.blacklist.iter().any(|entry| {
            entry.ip == ip
                && entry.port == port
                && now - entry.timestamp < 86400 // 24h 过期
        })
    }

    /// 添加服务器到黑名单
    fn add_to_blacklist(&mut self, ip: &str, port: u16, reason: &str) {
        // 避免重复添加
        if self.is_blacklisted(ip, port) {
            return;
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.blacklist.push(BlacklistEntry {
            ip: ip.to_string(),
            port,
            reason: reason.to_string(),
            timestamp: now,
        });

        // 清理过期条目
        self.blacklist.retain(|entry| now - entry.timestamp < 86400);

        self.save();
    }

    /// 记录成功连接
    fn record_success(&mut self, ip: &str, port: u16, name: &str, latency_ms: u32) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.last_success = Some(ServerCacheEntry {
            ip: ip.to_string(),
            port,
            name: name.to_string(),
            timestamp: now,
            latency_ms,
        });

        // 更新统计
        let key = format!("{}:{}", ip, port);
        let stats = self.server_stats.entry(key).or_insert(ServerStats {
            success: 0,
            fail: 0,
            avg_latency: 0,
        });
        stats.success += 1;
        stats.avg_latency = (stats.avg_latency * (stats.success - 1) + latency_ms) / stats.success;

        self.save();
    }

    /// 记录失败连接
    fn record_failure(&mut self, ip: &str, port: u16) {
        let key = format!("{}:{}", ip, port);
        let stats = self.server_stats.entry(key).or_insert(ServerStats {
            success: 0,
            fail: 0,
            avg_latency: 0,
        });
        stats.fail += 1;

        self.save();
    }
}

// ================================================================
// TdxSmartClient
// ================================================================

/// 智能连接客户端
///
/// 包装 `TdxHqClient`，增加惰性健康检查和服务器缓存功能。
pub struct TdxSmartClient {
    /// 内部客户端
    inner: TdxHqClient,
    /// 服务器缓存
    cache: Mutex<ServerCache>,
    /// 当前连接的服务器信息
    current_server: Mutex<Option<(String, u16, String)>>,
    /// 是否已通过健康检查
    health_checked: AtomicBool,
    /// 重试次数上限
    max_retry: usize,
}

impl TdxSmartClient {
    /// 创建新的智能客户端
    pub fn new() -> Self {
        Self {
            inner: TdxHqClient::new(),
            cache: Mutex::new(ServerCache::load()),
            current_server: Mutex::new(None),
            health_checked: AtomicBool::new(false),
            max_retry: 3,
        }
    }

    /// 连接到任意可用服务器 (快速模式)
    ///
    /// 仅验证 TCP + 握手，不做 K 线健康检查。
    /// 优先使用缓存的成功服务器。
    pub fn connect_to_any(&self, timeout: Option<f64>) -> Result<bool> {
        let cache = self.cache.lock().unwrap();

        // 1. 尝试缓存的成功服务器
        if let Some(ref last) = cache.last_success {
            logi!("smart", "trying cached server: {} ({}:{})", last.name, last.ip, last.port);
            match self.inner.connect(&last.ip, last.port, timeout) {
                Ok(true) => {
                    *self.current_server.lock().unwrap() = Some((last.ip.clone(), last.port, last.name.clone()));
                    self.health_checked.store(false, Ordering::SeqCst);
                    return Ok(true);
                }
                _ => {
                    logw!("smart", "cached server {} unavailable, trying next", last.ip);
                }
            }
        }

        drop(cache);

        // 2. 遍历 PRIMARY_SERVERS (跳过黑名单)
        for &(name, ip, port) in PRIMARY_SERVERS {
            if self.cache.lock().unwrap().is_blacklisted(ip, port) {
                logi!("smart", "skipping blacklisted server: {}:{}", ip, port);
                continue;
            }

            match self.inner.connect(ip, port, timeout) {
                Ok(true) => {
                    *self.current_server.lock().unwrap() = Some((ip.to_string(), port, name.to_string()));
                    self.health_checked.store(false, Ordering::SeqCst);
                    return Ok(true);
                }
                _ => continue,
            }
        }

        // 3. 兜底: 遍历 ALL_KNOWN_SERVERS
        for &(name, ip, port) in ALL_KNOWN_SERVERS {
            if self.cache.lock().unwrap().is_blacklisted(ip, port) {
                continue;
            }

            match self.inner.connect(ip, port, timeout) {
                Ok(true) => {
                    *self.current_server.lock().unwrap() = Some((ip.to_string(), port, name.to_string()));
                    self.health_checked.store(false, Ordering::SeqCst);
                    return Ok(true);
                }
                _ => continue,
            }
        }

        loge!("smart", "all servers unreachable");
        Err(ErrorCode::CONNECTION_FAILED.err("all servers unreachable"))
    }

    /// 惰性健康检查
    ///
    /// 在首次 K 线请求返回空时调用。
    /// 如果健康检查失败，断开连接并尝试下一个服务器。
    fn lazy_health_check(&self) -> bool {
        if self.health_checked.load(Ordering::SeqCst) {
            return true;
        }

        let server = self.current_server.lock().unwrap().clone();
        if let Some((ip, port, name)) = server {
            logi!("smart", "performing lazy health check on {}:{}...", ip, port);

            // 使用 K 线请求验证
            let packet = utils::build_security_bars_packet(4, 1, "600519", 0, 1, 0);
            match self.inner.send_raw_and_recv(&packet) {
                Ok(body) => {
                    use crate::protocol::parsers::parse_security_bars;
                    match parse_security_bars(&body, 4) {
                        Ok(bars) if !bars.is_empty() => {
                            logi!("smart", "health check passed: got {} bars", bars.len());
                            self.health_checked.store(true, Ordering::SeqCst);
                            self.cache.lock().unwrap().record_success(&ip, port, &name, 0);
                            return true;
                        }
                        Ok(_) => {
                            logw!("smart", "health check failed: K-line empty, server may have protocol anomaly");
                            self.cache.lock().unwrap().add_to_blacklist(&ip, port, "kline_empty");
                            self.cache.lock().unwrap().record_failure(&ip, port);
                            self.inner.disconnect();
                            return false;
                        }
                        Err(e) => {
                            logw!("smart", "health check failed: parse error: {}", e);
                            self.cache.lock().unwrap().add_to_blacklist(&ip, port, "parse_error");
                            self.cache.lock().unwrap().record_failure(&ip, port);
                            self.inner.disconnect();
                            return false;
                        }
                    }
                }
                Err(e) => {
                    logw!("smart", "health check failed: {}", e);
                    self.cache.lock().unwrap().record_failure(&ip, port);
                    self.inner.disconnect();
                    return false;
                }
            }
        }

        false
    }

    /// 尝试切换到下一个服务器
    fn try_next_server(&self) -> Result<bool> {
        let current = self.current_server.lock().unwrap().clone();

        // 遍历 PRIMARY_SERVERS，跳过当前和黑名单
        for &(name, ip, port) in PRIMARY_SERVERS {
            if let Some((ref cur_ip, cur_port, _)) = current {
                if ip == cur_ip && port == cur_port {
                    continue;
                }
            }

            if self.cache.lock().unwrap().is_blacklisted(ip, port) {
                continue;
            }

            match self.inner.connect(ip, port, Some(5.0)) {
                Ok(true) => {
                    *self.current_server.lock().unwrap() = Some((ip.to_string(), port, name.to_string()));
                    self.health_checked.store(false, Ordering::SeqCst);
                    logi!("smart", "switched to server: {}:{}", ip, port);
                    return Ok(true);
                }
                _ => continue,
            }
        }

        loge!("smart", "no alternative server available");
        Err(ErrorCode::CONNECTION_FAILED.err("no alternative server available"))
    }

    /// 获取 K 线数据 (带自动重试)
    ///
    /// 如果返回空数据，自动触发健康检查并尝试切换服务器。
    pub fn get_security_bars(
        &self,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
        fq: u8,
    ) -> Result<Vec<SecurityBar>> {
        let mut last_err = None;

        for attempt in 0..self.max_retry {
            match self.inner.get_security_bars(category, market, code, start, count, fq) {
                Ok(bars) if !bars.is_empty() => {
                    // 成功获取数据
                    if attempt > 0 {
                        logi!("smart", "got {} bars after {} retries", bars.len(), attempt);
                    }
                    return Ok(bars);
                }
                Ok(_) => {
                    // 返回空数据，触发健康检查
                    logw!("smart", "attempt {}/{}: empty response, triggering health check", attempt + 1, self.max_retry);

                    if !self.lazy_health_check() {
                        // 健康检查失败，尝试切换服务器
                        logw!("smart", "health check failed, trying next server...");
                        match self.try_next_server() {
                            Ok(true) => continue,
                            Ok(false) => {
                                last_err = Some(ErrorCode::CONNECTION_FAILED.err("no more servers"));
                                break;
                            }
                            Err(e) => {
                                last_err = Some(e);
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    logw!("smart", "attempt {}/{}: error: {}, trying next server", attempt + 1, self.max_retry, e);
                    last_err = Some(e);
                    // 连接错误，尝试切换服务器
                    match self.try_next_server() {
                        Ok(true) => continue,
                        _ => break,
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| ErrorCode::RETRY_EXHAUSTED.err("max retry reached")))
    }

    /// 获取实时行情 (带自动重试)
    pub fn get_security_quotes(
        &self,
        all_stock: &[(u8, &str)],
    ) -> Result<Vec<SecurityQuote>> {
        let mut last_err = None;

        for attempt in 0..self.max_retry {
            match self.inner.get_security_quotes(all_stock) {
                Ok(quotes) if !quotes.is_empty() => {
                    if attempt > 0 {
                        logi!("smart", "got {} quotes after {} retries", quotes.len(), attempt);
                    }
                    return Ok(quotes);
                }
                Ok(_) => {
                    // 返回空数据，触发健康检查
                    logw!("smart", "attempt {}/{}: empty quotes, triggering health check", attempt + 1, self.max_retry);

                    if !self.lazy_health_check() {
                        logw!("smart", "health check failed, trying next server...");
                        match self.try_next_server() {
                            Ok(true) => continue,
                            _ => break,
                        }
                    }
                }
                Err(e) => {
                    last_err = Some(e);
                    logw!("smart", "attempt {}/{}: error, trying next server", attempt + 1, self.max_retry);
                    match self.try_next_server() {
                        Ok(true) => continue,
                        _ => break,
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| ErrorCode::RETRY_EXHAUSTED.err("max retry reached")))
    }

    /// 委托其他方法到内部客户端
    pub fn inner(&self) -> &TdxHqClient {
        &self.inner
    }

    /// 设置握手模式 (委托到内部 TdxHqClient)
    ///
    /// Issue #12: 若服务器拒绝行情数据，切换为 `HandshakeMode::SkipCmd3`
    pub fn set_handshake_mode(&self, mode: utils::HandshakeMode) {
        self.inner.set_handshake_mode(mode);
    }

    /// 获取当前握手模式
    pub fn handshake_mode(&self) -> utils::HandshakeMode {
        self.inner.handshake_mode()
    }

    /// 获取缓存统计
    pub fn cache_stats(&self) -> String {
        let cache = self.cache.lock().unwrap();
        format!(
            "last_success: {:?}, blacklist: {}, stats: {}",
            cache.last_success.as_ref().map(|s| format!("{}:{}", s.ip, s.port)),
            cache.blacklist.len(),
            cache.server_stats.len()
        )
    }

    /// 清除缓存
    pub fn clear_cache(&self) {
        let mut cache = self.cache.lock().unwrap();
        *cache = ServerCache::new();
        cache.save();
        logi!("smart", "cache cleared");
    }

    /// 探测所有服务器并更新缓存
    ///
    /// 类似 mootdx 的 bestip 功能。
    ///
    /// **真实 K 线探测** (v0.6.9, Issue #12 对齐):
    /// 不只验证 TCP + 握手 — 连接成功后再拉取一根 600519 日K，
    /// 数据非空才算服务器可用。握手成功 ≠ 行情可用 (部分服务器
    /// 收到 SETUP_CMD3 后会掐掉行情权限，返回 2 字节 0x2003)。
    ///
    /// 注意: 探测使用客户端当前握手模式。若目标服务器拒绝行情数据，
    /// 请先 `set_handshake_mode(HandshakeMode::SkipCmd3)` 再探测。
    pub fn probe_and_cache(&self, timeout_secs: f64) -> Vec<(String, u16, String, u32)> {
        let mut results = Vec::new();

        for &(name, ip, port) in PRIMARY_SERVERS {
            let start = Instant::now();
            match self.inner.connect(ip, port, Some(timeout_secs)) {
                Ok(true) => {
                    // 真实 K 线验证: 拉取一根日K，数据非空才算可用
                    let bars =
                        self.inner.get_security_bars(4, 1, "600519", 0, 1, 1);
                    let latency = start.elapsed().as_millis() as u32;
                    match bars {
                        Ok(bars) if !bars.is_empty() => {
                            self.cache.lock().unwrap().record_success(ip, port, name, latency);
                            results.push((ip.to_string(), port, name.to_string(), latency));
                            logi!("probe", "{}:{} ({}) - {}ms (kline ok)", ip, port, name, latency);
                        }
                        Ok(_) => {
                            // 握手成功但 K 线为空 — Issue #12 典型症状
                            self.cache.lock().unwrap().record_failure(ip, port);
                            logw!("probe", "{}:{} ({}) - handshake ok but empty kline", ip, port, name);
                        }
                        Err(e) => {
                            self.cache.lock().unwrap().record_failure(ip, port);
                            logw!("probe", "{}:{} ({}) - kline err: {}", ip, port, name, e);
                        }
                    }
                    self.inner.disconnect();
                }
                _ => {
                    self.cache.lock().unwrap().record_failure(ip, port);
                    logw!("probe", "{}:{} ({}) - connect failed", ip, port, name);
                }
            }
        }

        results.sort_by_key(|r| r.3);
        results
    }
}

impl Default for TdxSmartClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_cache_blacklist() {
        let mut cache = ServerCache::new();
        assert!(!cache.is_blacklisted("1.2.3.4", 7709));

        cache.add_to_blacklist("1.2.3.4", 7709, "test");
        assert!(cache.is_blacklisted("1.2.3.4", 7709));
        assert!(!cache.is_blacklisted("5.6.7.8", 7709));
    }

    #[test]
    fn test_server_cache_stats() {
        let mut cache = ServerCache::new();
        cache.record_success("1.2.3.4", 7709, "test", 100);
        cache.record_success("1.2.3.4", 7709, "test", 200);
        cache.record_failure("1.2.3.4", 7709);

        let key = "1.2.3.4:7709".to_string();
        let stats = cache.server_stats.get(&key).unwrap();
        assert_eq!(stats.success, 2);
        assert_eq!(stats.fail, 1);
        assert_eq!(stats.avg_latency, 150); // (100 + 200) / 2
    }
}
