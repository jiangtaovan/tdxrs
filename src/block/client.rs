//! 板块专用客户端
//!
//! 封装 TdxDirectClient，内置板块查询限制，避免误用通用客户端。
//!
//! ## 限制策略
//!
//! | 数据类型 | 限制 | 理由 |
//! |---------|------|------|
//! | K线 (日/周/月) | 无限制 | 数据量小，常用 |
//! | K线 (60min) | 默认 200，上限 800 | 高频需求少 |
//! | K线 (30min/15min/5min) | 默认 50，上限 200 | 高频数据量大 |
//! | K线 (1min) | **禁用** | 板块无此需求 |
//! | 分时数据 | **禁用** | 板块聚合数据无意义 |
//! | 逐笔成交 | **禁用** | 板块无买卖方向 |
//! | 实时行情 | 允许 | 单次查询，量小 |

use std::sync::Mutex;

use crate::error::{Result, TdxError};
use crate::net::direct_client::TdxDirectClient;
use crate::protocol::constants::{DEFAULT_PORT, PRIMARY_SERVERS};
use crate::protocol::types::{IndexBar, SecurityQuote};
use crate::reader::block::BlockRecord;
use crate::{logd, logi, logw};

/// K线级别限制配置
struct KlineLimit {
    /// 默认返回条数
    default_count: u16,
    /// 最大允许条数
    max_count: u16,
    /// 是否允许查询
    allowed: bool,
}

/// 板块专用客户端
///
/// 内置板块查询限制，不暴露分时/逐笔等高频接口。
/// 支持空响应自动重试：当K线请求返回空数据时，自动切换服务器重试。
pub struct TdxBlockClient {
    client: Mutex<TdxDirectClient>,
    /// 用户指定的初始服务器 (ip, port)
    user_server: (String, u16),
    /// 当前使用的服务器在候选列表中的索引 (None = 尚未确定)
    current_index: Mutex<Option<usize>>,
    /// 候选服务器列表: (name, ip, port)
    servers: Vec<(&'static str, &'static str, u16)>,
}

impl TdxBlockClient {
    /// 创建板块客户端
    ///
    /// - `ip`: 服务器地址
    /// - `port`: 端口 (默认 7709)
    /// - `timeout`: 超时秒数 (默认 5.0)
    ///
    /// 当K线请求返回空数据时，自动尝试 PRIMARY_SERVERS 中的其他服务器。
    pub fn new(ip: &str, port: u16, timeout: f64) -> Self {
        // 构建候选服务器列表: 用户指定的服务器 + PRIMARY_SERVERS (去重)
        let mut servers: Vec<(&str, &str, u16)> = Vec::with_capacity(PRIMARY_SERVERS.len() + 1);
        let mut user_in_primary = false;
        for &(name, pip, pport) in PRIMARY_SERVERS {
            if pip == ip && pport == port {
                user_in_primary = true;
            }
            servers.push((name, pip, pport));
        }
        // 如果用户指定的服务器不在 PRIMARY 中，插入到最前面
        if !user_in_primary {
            // 泄漏一个 String 使其成为 'static — 这里是合理的，因为服务器地址是全局常量级别的生命周期
            let ip_static: &'static str = Box::leak(ip.to_string().into_boxed_str());
            let name_static: &'static str = Box::leak(format!("user:{}", ip).into_boxed_str());
            servers.insert(0, (name_static, ip_static, port));
        }

        Self {
            client: Mutex::new(TdxDirectClient::new(ip, port, timeout)),
            user_server: (ip.to_string(), port),
            current_index: Mutex::new(None),
            servers,
        }
    }

    /// 使用默认端口和超时创建
    pub fn with_default(ip: &str) -> Self {
        Self::new(ip, DEFAULT_PORT, 5.0)
    }

    /// 更新服务器地址
    pub fn set_server(&self, ip: &str, port: u16) {
        self.client.lock().unwrap().set_server(ip, port);
    }

    /// 更新超时
    pub fn set_timeout(&self, timeout: f64) {
        self.client.lock().unwrap().set_timeout(timeout);
    }

    /// 设置握手模式 (委托到内部 TdxDirectClient)
    ///
    /// Issue #12: 若服务器拒绝行情数据，切换为 `HandshakeMode::SkipCmd3`
    pub fn set_handshake_mode(&self, mode: crate::net::utils::HandshakeMode) {
        self.client.lock().unwrap().set_handshake_mode(mode);
    }

    /// 获取当前握手模式
    pub fn handshake_mode(&self) -> crate::net::utils::HandshakeMode {
        self.client.lock().unwrap().handshake_mode()
    }

    /// 获取用户指定的初始服务器
    pub fn user_server(&self) -> &(String, u16) {
        &self.user_server
    }

    /// 切换到下一台候选服务器
    ///
    /// 返回 true 表示成功切换，false 表示已无更多候选。
    fn switch_to_next_server(&self) -> bool {
        let mut idx_guard = self.current_index.lock().unwrap();
        let current = idx_guard.unwrap_or(0);
        let next = current + 1;
        if next >= self.servers.len() {
            return false;
        }
        let (name, ip, port) = self.servers[next];
        logd!("block", "switching to server {} ({}:{})", name, ip, port);
        self.client.lock().unwrap().set_server(ip, port);
        *idx_guard = Some(next);
        true
    }

    /// 获取 K 级别限制配置
    fn kline_limit(category: u8) -> KlineLimit {
        match category {
            // 日/周/月/季/年 — 无限制
            4 | 5 | 6 | 10 | 11 => KlineLimit {
                default_count: 100,
                max_count: 800,
                allowed: true,
            },
            // 60min — 默认 200
            3 => KlineLimit {
                default_count: 200,
                max_count: 800,
                allowed: true,
            },
            // 30min — 默认 50
            2 => KlineLimit {
                default_count: 50,
                max_count: 200,
                allowed: true,
            },
            // 15min — 默认 50
            1 => KlineLimit {
                default_count: 50,
                max_count: 200,
                allowed: true,
            },
            // 5min — 默认 50
            0 => KlineLimit {
                default_count: 50,
                max_count: 200,
                allowed: true,
            },
            // 1min (8) / 扩展1min (7) — 禁用
            7 | 8 => KlineLimit {
                default_count: 0,
                max_count: 0,
                allowed: false,
            },
            // 其他 — 按日线处理
            _ => KlineLimit {
                default_count: 100,
                max_count: 800,
                allowed: true,
            },
        }
    }

    // ================================================================
    // 板块 K 线 (核心功能)
    // ================================================================

    /// 获取板块 K 线数据
    ///
    /// 自动应用板块限制:
    /// - 日/周/月: 无限制 (max 800)
    /// - 分钟级: 默认 50，max 200
    /// - 1min: 禁用，返回错误
    ///
    /// 空响应自动重试: 当服务器返回空K线数据时，自动切换到下一台候选服务器重试。
    ///
    /// `code`: 板块代码 (88xxxx)
    /// `category`: K线种类 (0=5min, 1=15min, 3=60min, 4=day, 5=week, 6=month)
    /// `count`: 请求条数 (0=使用默认值)
    pub fn get_block_bars(
        &self,
        category: u8,
        code: &str,
        start: u32,
        count: u16,
    ) -> Result<Vec<IndexBar>> {
        let limit = Self::kline_limit(category);

        if !limit.allowed {
            return Err(TdxError::InvalidData(format!(
                "category={} (1min/扩展1min) is not allowed for block queries",
                category
            )));
        }

        let actual_count = if count == 0 {
            limit.default_count
        } else {
            count.min(limit.max_count)
        };

        // 日K线及以上: 空响应自动重试 (与 TdxHqClient 一致)
        let should_retry = category >= 4;
        let max_retry = if should_retry { self.servers.len().min(5) } else { 1 };
        let mut retry_count = 0u32;

        for attempt in 0..max_retry {
            let bars = self.client.lock().unwrap()
                .get_index_bars_inner(category, 1, code, start, actual_count, 0)?;

            if !bars.is_empty() {
                if retry_count > 0 {
                    logi!("block", "got {} bars for {} after {} server switch(es)",
                          bars.len(), code, retry_count);
                }
                return Ok(bars);
            }

            if !should_retry || attempt >= max_retry - 1 {
                break;
            }

            retry_count += 1;
            logd!("block", "attempt {}/{}: empty K-line for {}, switching server",
                  attempt + 1, max_retry, code);

            if !self.switch_to_next_server() {
                logd!("block", "no more candidate servers");
                break;
            }
        }

        if should_retry && retry_count > 0 {
            logw!("block", "all {} attempts returned empty K-line for {}", max_retry, code);
        }

        Ok(Vec::new())
    }

    /// 获取板块 K 线 (使用默认条数)
    pub fn get_block_bars_default(
        &self,
        category: u8,
        code: &str,
    ) -> Result<Vec<IndexBar>> {
        self.get_block_bars(category, code, 0, 0)
    }

    // ================================================================
    // 板块实时行情
    // ================================================================

    /// 获取板块实时行情
    ///
    /// `codes`: 板块代码列表 (88xxxx)
    ///
    /// 空响应自动重试: 当服务器返回空行情数据时，自动切换到下一台候选服务器重试。
    pub fn get_block_quotes(&self, codes: &[&str]) -> Result<Vec<SecurityQuote>> {
        let pairs: Vec<(u8, &str)> = codes.iter().map(|&c| (1u8, c)).collect();
        let max_retry = self.servers.len().min(3);
        let mut retry_count = 0u32;

        for attempt in 0..max_retry {
            let quotes = self.client.lock().unwrap().get_security_quotes_inner(&pairs)?;

            if !quotes.is_empty() {
                if retry_count > 0 {
                    logi!("block", "got {} quotes after {} server switch(es)",
                          quotes.len(), retry_count);
                }
                return Ok(quotes);
            }

            if attempt >= max_retry - 1 {
                break;
            }

            retry_count += 1;
            logd!("block", "attempt {}/{}: empty quotes, switching server",
                  attempt + 1, max_retry);

            if !self.switch_to_next_server() {
                break;
            }
        }

        if retry_count > 0 {
            logw!("block", "all {} attempts returned empty quotes", max_retry);
        }

        Ok(Vec::new())
    }

    // ================================================================
    // 板块列表 (从服务器下载 .dat 文件)
    // ================================================================

    /// 从服务器下载并解析板块文件
    ///
    /// `block_file`: 文件名，如 `"block_fg.dat"`, `"block_gn.dat"`, `"block_zs.dat"`
    ///
    /// 返回板块成分股级别的记录。同一板块名称会出现多次（每个成分股一条）。
    /// 使用 `BlockQuery::list_blocks()` 可按板块名称去重聚合。
    pub fn get_block_list(&self, block_file: &str) -> Result<Vec<BlockRecord>> {
        self.client.lock().unwrap().get_and_parse_block_info(block_file)
    }

    /// 获取行业板块列表 (block_fg.dat)
    ///
    /// 返回筛选类标签板块，如融资融券、破净资产、高股息等。
    pub fn get_industry_blocks(&self) -> Result<Vec<BlockRecord>> {
        self.get_block_list(crate::protocol::constants::BLOCK_FG)
    }

    /// 获取概念板块列表 (block_gn.dat)
    ///
    /// 返回概念板块，如5G概念、一带一路、碳中和等。
    pub fn get_concept_blocks(&self) -> Result<Vec<BlockRecord>> {
        self.get_block_list(crate::protocol::constants::BLOCK_GN)
    }

    /// 获取指数成分列表 (block_zs.dat)
    ///
    /// 返回指数成分板块，如沪深300、上证50、创业板指等。
    pub fn get_index_blocks(&self) -> Result<Vec<BlockRecord>> {
        self.get_block_list(crate::protocol::constants::BLOCK_SZ)
    }

    // ================================================================
    // 不暴露的接口 (明确禁用)
    // ================================================================

    // 以下接口不暴露给板块客户端:
    // - get_minute_time_data: 板块分时数据无意义
    // - get_transaction_data: 板块逐笔成交无买卖方向
    // - get_security_bars: 应使用 get_block_bars (带限制)
    // - get_history_minute_time_data: 同上
    // - get_history_transaction_data: 同上
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kline_limit_daily() {
        let limit = TdxBlockClient::kline_limit(4); // day
        assert!(limit.allowed);
        assert_eq!(limit.default_count, 100);
        assert_eq!(limit.max_count, 800);
    }

    #[test]
    fn test_kline_limit_5min() {
        let limit = TdxBlockClient::kline_limit(0); // 5min
        assert!(limit.allowed);
        assert_eq!(limit.default_count, 50);
        assert_eq!(limit.max_count, 200);
    }

    #[test]
    fn test_kline_limit_60min() {
        let limit = TdxBlockClient::kline_limit(3); // 60min
        assert!(limit.allowed);
        assert_eq!(limit.default_count, 200);
        assert_eq!(limit.max_count, 800);
    }

    #[test]
    fn test_kline_limit_1min_disabled() {
        let limit = TdxBlockClient::kline_limit(8); // 1min
        assert!(!limit.allowed);
    }

    #[test]
    fn test_new_client() {
        let client = TdxBlockClient::new("127.0.0.1", 7709, 5.0);
        // Just verify it compiles and creates
        let _ = client;
    }

    #[test]
    fn test_with_default() {
        let client = TdxBlockClient::with_default("127.0.0.1");
        let _ = client;
    }
}
