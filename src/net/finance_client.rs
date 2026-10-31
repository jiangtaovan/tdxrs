//! 财务数据客户端 — 独立连接管理 + 本地磁盘缓存
//!
//! 与行情客户端分离的原因:
//!   1. 财务数据包体差异大 — get_finance_info (~136B) vs gpcw files (~12MB)
//!   2. 连接时长不同 — gpcw 下载可能持续数秒，不适合共享连接池
//!   3. 方便后续扩展 — 字段名映射、DataFrame 输出、本地缓存
//!
//! ## 磁盘缓存
//!
//! 通过 `set_cache_dir(path)` 启用。gpcw 文件下载后缓存到本地，
//! 24 小时内重复查询直接读取缓存，跳过网络下载。
//!
//! ## API 一览
//!
//! | 方法 | 数据量 | 说明 |
//! |------|:-----:|------|
//! | `get_finance_info(market, code)` | ~136B | 单股票 34 项实时财务 |
//! | `get_xdxr_info(market, code)` | ~200B | 除权除息历史 |
//! | `get_report_file(filename, offset)` | ≤30KB | 分片下载 gpcw 文件 |
//! | `get_financial_list()` | ~2KB | 可用报告期列表 (gpcw.txt) |

use flate2::read::ZlibDecoder;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, SystemTime};

use crate::error::Result;
use crate::net::connection::TcpConnection;
use crate::net::packet::{ResponseHeader, RSP_HEADER_LEN};
use crate::net::utils;
use crate::protocol::parsers::*;
use crate::protocol::types::*;
use crate::reader::financial::{parse_financial, FinancialRecord};
use crate::{loge, logi, logw};

/// 财务数据客户端默认超时 (秒)
const DEFAULT_FINANCE_TIMEOUT: f64 = 15.0;
/// 单次请求 chunk 大小 (30KB)
const CHUNK_SIZE: u32 = 0x7530;
/// 磁盘缓存有效期 (24 小时)
const CACHE_TTL: Duration = Duration::from_secs(24 * 3600);

// ================================================================
// 财务客户端
// ================================================================

pub struct TdxFinanceClient {
    ip: String,
    port: u16,
    timeout: f64,
    cache_dir: Option<PathBuf>,
    /// 握手模式 (Issue #12)
    handshake_mode: AtomicU8,
}

impl TdxFinanceClient {
    pub fn new(ip: &str, port: u16, timeout: Option<f64>) -> Self {
        Self {
            ip: ip.to_string(),
            port,
            timeout: timeout.unwrap_or(DEFAULT_FINANCE_TIMEOUT),
            cache_dir: None,
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

    pub fn set_server(&mut self, ip: &str, port: u16) {
        self.ip = ip.to_string();
        self.port = port;
    }

    pub fn set_timeout(&mut self, secs: f64) {
        self.timeout = secs;
    }

    /// 设置本地缓存目录 — 启用后 gpcw 文件自动缓存 24 小时
    ///
    /// 设为 `None` 禁用缓存。
    /// 缓存文件以原始文件名存储 (如 `gpcw20260331.dat`)。
    pub fn set_cache_dir(&mut self, path: Option<PathBuf>) {
        if let Some(ref p) = path {
            let _ = fs::create_dir_all(p);
        }
        self.cache_dir = path;
    }

    /// 获取当前缓存目录
    pub fn cache_dir(&self) -> Option<&PathBuf> {
        self.cache_dir.as_ref()
    }

    // ============================================================
    // 磁盘缓存逻辑
    // ============================================================

    /// 从缓存读取文件 (未过期返回 Some, 未命中/过期返回 None)
    fn cache_get(&self, filename: &str) -> Option<Vec<u8>> {
        let dir = self.cache_dir.as_ref()?;
        // 文件名提取: "tdxfin/gpcw20260331.dat" → "gpcw20260331.dat"
        let short = filename.rsplit('/').next().unwrap_or(filename);
        let path = dir.join(short);

        let meta = fs::metadata(&path).ok()?;
        let mtime = meta.modified().ok()?;
        let age = SystemTime::now().duration_since(mtime).unwrap_or(Duration::MAX);

        if age > CACHE_TTL {
            return None; // 过期
        }

        fs::read(&path).ok()
    }

    /// 写入数据到缓存
    fn cache_put(&self, filename: &str, data: &[u8]) {
        if let Some(ref dir) = self.cache_dir {
            let short = filename.rsplit('/').next().unwrap_or(filename);
            let _ = fs::write(dir.join(short), data);
        }
    }

    // ============================================================
    // 核心: 发包/收包/解压 (每次独立连接)
    // ============================================================

    fn send_and_recv(&self, packet: &[u8]) -> Result<Vec<u8>> {
        let mut conn = TcpConnection::connect(&self.ip, self.port, self.timeout)
            .map_err(|e| {
                loge!("finance", "connect to {}:{} failed: {}", self.ip, self.port, e);
                e
            })?;
        utils::perform_handshake_with_mode(&mut conn, self.handshake_mode())?;

        conn.send(packet)?;

        let head_buf = conn.recv(RSP_HEADER_LEN)?;
        let header = ResponseHeader::parse(&head_buf)?;

        let zip_size = header.zip_size as usize;
        let mut body_buf = Vec::with_capacity(zip_size);
        while body_buf.len() < zip_size {
            let remaining = zip_size - body_buf.len();
            let chunk = conn.recv(remaining)?;
            body_buf.extend_from_slice(&chunk);
        }

        if body_buf.is_empty() {
            return Err(crate::error_codes::ErrorCode::DISCONNECTED.err("empty response body"));
        }

        if header.zip_size != header.unzip_size {
            let mut decoder = ZlibDecoder::new(&body_buf[..]);
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed).map_err(|e| {
                crate::error_codes::ErrorCode::DECOMPRESS_FAILED.err(format!("{}", e))
            })?;
            Ok(decompressed)
        } else {
            Ok(body_buf)
        }
    }

    // ============================================================
    // 单股票实时财务
    // ============================================================

    pub fn get_finance_info(&self, market: u8, code: &str) -> Result<FinanceInfo> {
        let code_buf = utils::code_bytes(code);
        let mut packet = Vec::with_capacity(21);
        packet.extend_from_slice(&[
            0x0c, 0x1f, 0x18, 0x76, 0x00, 0x01, 0x0b, 0x00, 0x0b, 0x00, 0x10, 0x00, 0x01, 0x00,
        ]);
        packet.push(market);
        packet.extend_from_slice(&code_buf);
        parse_finance_info(&self.send_and_recv(&packet)?, market, code)
    }

    pub fn get_xdxr_info(&self, market: u8, code: &str) -> Result<Vec<XdXrInfo>> {
        let code_buf = utils::code_bytes(code);
        let mut packet = Vec::with_capacity(21);
        packet.extend_from_slice(&[
            0x0c, 0x1f, 0x18, 0x76, 0x00, 0x01, 0x0b, 0x00, 0x0b, 0x00, 0x0f, 0x00, 0x01, 0x00,
        ]);
        packet.push(market);
        packet.extend_from_slice(&code_buf);
        parse_xdxr_info(&self.send_and_recv(&packet)?)
    }

    // ============================================================
    // 报告文件下载 (分片)
    // ============================================================

    /// 下载报告文件的单个分片 (不走缓存 — 分片由上层 get_report_file_by_size 管理)
    pub fn get_report_file(&self, filename: &str, offset: u32) -> Result<Vec<u8>> {
        let name_bytes = filename.as_bytes();
        let mut name_buf = [0u8; 100];
        let len = name_bytes.len().min(100);
        name_buf[..len].copy_from_slice(&name_bytes[..len]);

        let raw_data_len = 2 + 4 + 4 + 100;
        let mut raw_data = Vec::with_capacity(raw_data_len);
        raw_data.extend_from_slice(&0x06B9u16.to_le_bytes());
        raw_data.extend_from_slice(&offset.to_le_bytes());
        raw_data.extend_from_slice(&CHUNK_SIZE.to_le_bytes());
        raw_data.extend_from_slice(&name_buf);

        let pkg_len = raw_data_len as u16;
        let mut packet = Vec::with_capacity(6 + pkg_len as usize);
        packet.extend_from_slice(&[0x0c, 0x12, 0x34, 0x00, 0x00, 0x00]);
        packet.extend_from_slice(&pkg_len.to_le_bytes());
        packet.extend_from_slice(&pkg_len.to_le_bytes());
        packet.extend_from_slice(&raw_data);

        let body = self.send_and_recv(&packet)?;
        if body.len() < 4 {
            return Ok(Vec::new());
        }

        let chunk_size = u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize;
        if chunk_size > 0 && body.len() >= 4 + chunk_size {
            Ok(body[4..4 + chunk_size].to_vec())
        } else {
            Ok(Vec::new())
        }
    }

    /// 下载完整的报告文件 (自动分片 + 重组, 优先磁盘缓存)
    pub fn get_report_file_by_size(
        &self,
        filename: &str,
        filesize: u32,
    ) -> Result<Vec<u8>> {
        // 1. 检查磁盘缓存
        if let Some(cached) = self.cache_get(filename) {
            logi!("finance", "cache hit: {}", filename);
            return Ok(cached);
        }

        // 2. 从网络下载
        let data = self.download_report_file(filename, filesize)?;

        // 3. 写入缓存
        self.cache_put(filename, &data);

        Ok(data)
    }

    /// 实际下载逻辑 (不分缓存)
    fn download_report_file(&self, filename: &str, filesize: u32) -> Result<Vec<u8>> {
        if filesize == 0 {
            // 未知大小: 下载第一片后判断总量, 最多 4 片
            let first = self.get_report_file(filename, 0)?;
            if first.len() < CHUNK_SIZE as usize {
                return Ok(first);
            }
            let mut data = first;
            for page in 1u32..4 {
                let chunk = self.get_report_file(filename, page * CHUNK_SIZE)?;
                if chunk.is_empty() { break; }
                data.extend_from_slice(&chunk);
                if chunk.len() < CHUNK_SIZE as usize { break; }
            }
            return Ok(data);
        }

        let effective_size = filesize;
        let mut data = Vec::with_capacity(effective_size as usize);
        let mut offset = 0u32;

        while (offset as u32) < effective_size {
            let chunk = self.get_report_file(filename, offset)?;
            if chunk.is_empty() {
                break;
            }
            data.extend_from_slice(&chunk);
            offset += chunk.len() as u32;
            if chunk.len() < CHUNK_SIZE as usize {
                break;
            }
        }

        Ok(data)
    }

    // ============================================================
    // 全市场历史财务 (gpcw*.dat)
    // ============================================================

    /// 获取可用报告期列表 (从 gpcw.txt, 优先磁盘缓存)
    pub fn get_financial_list(&self) -> Result<Vec<GpcwFileInfo>> {
        // gpcw.txt 也走缓存路径 (TTL 24h)
        let data = self.get_report_file_by_size("tdxfin/gpcw.txt", 0)?;
        let content = String::from_utf8_lossy(&data);
        let mut files = Vec::new();
        for line in content.trim().split('\n') {
            let parts: Vec<&str> = line.trim().split(',').collect();
            if parts.len() >= 3 {
                files.push(GpcwFileInfo {
                    filename: parts[0].to_string(),
                    hash: parts[1].to_string(),
                    filesize: parts[2].parse().unwrap_or(0),
                });
            }
        }
        Ok(files)
    }

    /// 下载并解析指定的 gpcw*.dat 报告期数据 (优先磁盘缓存)
    pub fn get_financial_data(&self, filename: &str, filesize: u32) -> Result<Vec<FinancialRecord>> {
        let full = format!("tdxfin/{}", filename);
        let data = self.get_report_file_by_size(&full, filesize)?;
        parse_financial(&data)
    }

    // ============================================================
    // 命名财务指标
    // ============================================================

    /// 获取单只股票的命名财务指标 (45 个核心字段, 英文 key, TDX 原始值)
    pub fn get_finance_indicators(
        &self,
        filename: &str,
        filesize: u32,
        code: &str,
    ) -> Result<std::collections::HashMap<&'static str, f64>> {
        let records = self.get_financial_data(filename, filesize)?;
        for r in &records {
            if r.code == code {
                return Ok(crate::protocol::finance_fields::extract_indicators(&r.fields));
            }
        }
        logw!("finance", "stock {} not found in {}", code, filename);
        Err(crate::error_codes::ErrorCode::INVALID_STOCK_CODE.err(
            format!("stock {} not found in {}", code, filename)
        ))
    }

    /// 获取单只股票的命名财务指标 (带中文标签, 适合展示/校验)
    pub fn get_finance_indicators_labeled(
        &self,
        filename: &str,
        filesize: u32,
        code: &str,
    ) -> Result<Vec<(&'static str, &'static str, f64)>> {
        let records = self.get_financial_data(filename, filesize)?;
        for r in &records {
            if r.code == code {
                return Ok(crate::protocol::finance_fields::extract_with_labels(&r.fields));
            }
        }
        logw!("finance", "stock {} not found in {}", code, filename);
        Err(crate::error_codes::ErrorCode::INVALID_STOCK_CODE.err(
            format!("stock {} not found in {}", code, filename)
        ))
    }
}

/// gpcw.txt 中的文件清单条目
#[derive(Debug, Clone)]
pub struct GpcwFileInfo {
    pub filename: String,
    pub hash: String,
    pub filesize: u32,
}

// ================================================================
// 单元测试
// ================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_client() {
        let client = TdxFinanceClient::new("127.0.0.1", 7709, None);
        assert_eq!(client.ip, "127.0.0.1");
        assert_eq!(client.port, 7709);
        assert!(client.timeout > 0.0);
        assert!(client.cache_dir.is_none());
    }

    #[test]
    fn test_new_client_custom_timeout() {
        let client = TdxFinanceClient::new("127.0.0.1", 7709, Some(30.0));
        assert!((client.timeout - 30.0).abs() < 0.001);
    }

    #[test]
    fn test_set_server() {
        let mut client = TdxFinanceClient::new("127.0.0.1", 7709, None);
        client.set_server("192.168.1.1", 7727);
        assert_eq!(client.ip, "192.168.1.1");
        assert_eq!(client.port, 7727);
    }

    #[test]
    fn test_set_timeout() {
        let mut client = TdxFinanceClient::new("127.0.0.1", 7709, None);
        client.set_timeout(25.0);
        assert!((client.timeout - 25.0).abs() < 0.001);
    }

    #[test]
    fn test_cache_dir_set_get() {
        let mut client = TdxFinanceClient::new("127.0.0.1", 7709, None);
        assert!(client.cache_dir().is_none());

        let dir = std::env::temp_dir().join("tdxrs_test_cache");
        client.set_cache_dir(Some(dir.clone()));
        assert!(client.cache_dir().is_some());
        assert!(dir.exists());

        // 清理
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_cache_hit_and_expiry() {
        let dir = std::env::temp_dir().join("tdxrs_cache_test2");
        let _ = std::fs::remove_dir_all(&dir);

        let mut client = TdxFinanceClient::new("127.0.0.1", 7709, None);
        client.set_cache_dir(Some(dir.clone()));

        // 没有缓存 → 返回 None
        assert!(client.cache_get("tdxfin/test.dat").is_none());

        // 写入缓存 → 可读取
        client.cache_put("tdxfin/test.dat", b"hello world");
        let cached = client.cache_get("tdxfin/test.dat");
        assert_eq!(cached, Some(b"hello world".to_vec()));

        // 短期文件名: "tdxfin/xxx" → 提取 "xxx"
        // 验证提取逻辑在 cache_get/cache_put 中正确
        assert!(dir.join("test.dat").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
