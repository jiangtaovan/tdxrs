//! F10 公司资料客户端 — 独立连接管理
//!
//! 与行情客户端分离的原因:
//!   1. F10 数据包体大 — 单分类 5KB-65KB，全量 10w-20w 字符
//!   2. 连接时长不同 — 全量获取可能持续数秒，不适合共享连接池
//!   3. 避免影响行情数据的实时性

use flate2::read::ZlibDecoder;
use std::io::Read;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::error::Result;
use crate::net::connection::TcpConnection;
use crate::net::packet::{ResponseHeader, RSP_HEADER_LEN};
use crate::net::utils;
use crate::profile::constants::*;
use crate::profile::parser::{parse_company_info_category, parse_company_info_content};
use crate::profile::types::*;
use crate::protocol::constants::{MARKET_SH, MARKET_SZ};
use crate::{loge};

/// F10 客户端默认超时 (秒)
const DEFAULT_F10_TIMEOUT: f64 = 10.0;

/// F10 公司资料客户端 (独立连接)
///
/// 每次请求建立独立 TCP 连接，不占用共享连接池。
/// 适用于 F10 数据获取等大包体、长耗时操作。
///
/// # 示例
///
/// ```rust
/// use tdxrs::net::f10_client::TdxF10Client;
///
/// let client = TdxF10Client::new("180.153.18.170", 7709, None);
/// let categories = client.get_category(1, "600519")?;
/// let content = client.get_content(1, "600519", &categories[0])?;
/// ```
pub struct TdxF10Client {
    ip: String,
    port: u16,
    timeout: f64,
    /// 握手模式 (Issue #12)
    handshake_mode: AtomicU8,
}

impl TdxF10Client {
    /// 创建新的 F10 客户端
    ///
    /// # 参数
    /// * `ip` - 服务器 IP
    /// * `port` - 服务器端口
    /// * `timeout` - 超时时间 (秒)，默认 10 秒
    pub fn new(ip: &str, port: u16, timeout: Option<f64>) -> Self {
        Self {
            ip: ip.to_string(),
            port,
            timeout: timeout.unwrap_or(DEFAULT_F10_TIMEOUT),
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

    /// 设置服务器地址
    pub fn set_server(&mut self, ip: &str, port: u16) {
        self.ip = ip.to_string();
        self.port = port;
    }

    /// 设置超时时间
    pub fn set_timeout(&mut self, secs: f64) {
        self.timeout = secs;
    }

    // ============================================================
    // 核心: 发包/收包/解压 (每次独立连接)
    // ============================================================

    fn send_and_recv(&self, packet: &[u8]) -> Result<Vec<u8>> {
        let mut conn = TcpConnection::connect(&self.ip, self.port, self.timeout)
            .map_err(|e| {
                loge!("f10", "connect to {}:{} failed: {}", self.ip, self.port, e);
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
    // F10 API
    // ============================================================

    /// 获取 F10 分类列表
    ///
    /// # 参数
    /// * `market` - 市场代码 (0=SZ, 1=SH)
    /// * `code` - 股票代码
    pub fn get_category(&self, market: u8, code: &str) -> Result<Vec<F10Category>> {
        // 验证市场代码
        if market != MARKET_SZ && market != MARKET_SH {
            return Err(crate::error_codes::ErrorCode::ARGUMENT_OUT_OF_RANGE.err(
                format!("无效的市场代码: {} (仅支持 0=SZ, 1=SH)", market)
            ));
        }

        // 验证股票代码
        if code.len() != 6 {
            return Err(crate::error_codes::ErrorCode::INVALID_STOCK_CODE.err(
                format!("{} (必须为 6 位数字)", code)
            ));
        }

        // 构建请求包
        let mut pkg = Vec::with_capacity(24);
        pkg.extend_from_slice(&CATEGORY_REQUEST_HEADER);
        pkg.extend_from_slice(&(market as u16).to_le_bytes());

        // 股票代码 (GBK, 6 字节)
        let code_gbk = utils::encode_gbk(code)?;
        pkg.extend_from_slice(&code_gbk);

        // 保留字段 (u32, 0)
        pkg.extend_from_slice(&0u32.to_le_bytes());

        // 发送请求并接收响应
        let response = self.send_and_recv(&pkg)?;

        // 解析响应
        parse_company_info_category(&response)
    }

    /// 获取 F10 分类列表 (自动识别市场)
    pub fn get_category_auto(&self, code: &str) -> Result<Vec<F10Category>> {
        let market = utils::auto_market(code)?;
        self.get_category(market, code)
    }

    /// 获取 F10 内容
    ///
    /// # 参数
    /// * `market` - 市场代码
    /// * `code` - 股票代码
    /// * `category` - 分类信息 (从 get_category 获取)
    pub fn get_content(
        &self,
        market: u8,
        code: &str,
        category: &F10Category,
    ) -> Result<F10Content> {
        // 验证市场代码
        if market != MARKET_SZ && market != MARKET_SH {
            return Err(crate::error_codes::ErrorCode::ARGUMENT_OUT_OF_RANGE.err(
                format!("无效的市场代码: {} (仅支持 0=SZ, 1=SH)", market)
            ));
        }

        // 验证股票代码
        if code.len() != 6 {
            return Err(crate::error_codes::ErrorCode::INVALID_STOCK_CODE.err(
                format!("{} (必须为 6 位数字)", code)
            ));
        }

        // 构建请求包
        let mut pkg = Vec::with_capacity(104);
        pkg.extend_from_slice(&CONTENT_REQUEST_HEADER);
        pkg.extend_from_slice(&(market as u16).to_le_bytes());

        // 股票代码 (GBK, 6 字节)
        let code_gbk = utils::encode_gbk(code)?;
        pkg.extend_from_slice(&code_gbk);

        // 保留字段 (u16, 0)
        pkg.extend_from_slice(&0u16.to_le_bytes());

        // 文件名 (GBK, 80 字节, null 填充) — 优先使用原始字节避免编码损耗
        let filename_gbk = if category.filename_raw.is_empty() {
            utils::encode_gbk_padded(&category.filename, CATEGORY_FILENAME_SIZE)?
        } else {
            let mut raw = category.filename_raw.clone();
            if raw.len() < CATEGORY_FILENAME_SIZE {
                raw.resize(CATEGORY_FILENAME_SIZE, 0);
            } else if raw.len() > CATEGORY_FILENAME_SIZE {
                raw.truncate(CATEGORY_FILENAME_SIZE);
            }
            raw
        };
        pkg.extend_from_slice(&filename_gbk);

        // 起始位置 (u32)
        pkg.extend_from_slice(&category.start.to_le_bytes());

        // 数据长度 (u32)
        pkg.extend_from_slice(&category.length.to_le_bytes());

        // 保留字段 (u32, 0)
        pkg.extend_from_slice(&0u32.to_le_bytes());

        // 发送请求并接收响应
        let response = self.send_and_recv(&pkg)?;

        // 解析响应
        let content = parse_company_info_content(&response)?;

        Ok(F10Content::new(
            category.name.clone(),
            content,
        ))
    }

    /// 获取指定分类的内容
    ///
    /// # 参数
    /// * `market` - 市场代码
    /// * `code` - 股票代码
    /// * `name` - 分类名称 (如 "公司概况")
    pub fn get_content_by_name(
        &self,
        market: u8,
        code: &str,
        name: &str,
    ) -> Result<F10Content> {
        // 先获取分类列表
        let categories = self.get_category(market, code)?;

        // 查找指定分类
        let category = categories
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| {
                crate::error_codes::ErrorCode::MISSING_FIELD.err(format!(
                    "未找到分类 '{}'，可用分类: {}",
                    name,
                    categories
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;

        self.get_content(market, code, category)
    }

    /// 获取所有分类的内容
    ///
    /// # 参数
    /// * `market` - 市场代码
    /// * `code` - 股票代码
    pub fn get_all_contents(
        &self,
        market: u8,
        code: &str,
    ) -> Result<Vec<F10Content>> {
        let categories = self.get_category(market, code)?;
        let mut contents = Vec::with_capacity(categories.len());

        for category in &categories {
            match self.get_content(market, code, category) {
                Ok(content) => contents.push(content),
                Err(e) => {
                    loge!("f10", "获取分类 '{}' 失败: {}", category.name, e);
                }
            }
        }

        Ok(contents)
    }

    /// 获取所有分类的内容 (返回 F10Data)
    ///
    /// # 参数
    /// * `market` - 市场代码
    /// * `code` - 股票代码
    pub fn get_all_data(&self, market: u8, code: &str) -> Result<F10Data> {
        let contents = self.get_all_contents(market, code)?;
        let mut data = F10Data::new(code.to_string(), market);
        for content in contents {
            data.add_content(content);
        }
        Ok(data)
    }
}
