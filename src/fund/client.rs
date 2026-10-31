//! 基金客户端封装
//!
//! 封装现有的 `TdxHqClient`，提供基金专用的 API。
//! 不修改现有客户端代码，仅通过组合方式扩展功能。
//!
//! 支持的基金类型: ETF、LOF、REITs、分级基金、债券基金等。

use crate::error::Result;
use crate::net::client::TdxHqClient;
use crate::protocol::types::{MinuteTimePrice, TickData};

use super::constants::{classify_fund, is_fund, MARKET_SH, MARKET_SZ};
use super::types::*;
use super::utils::{validate_fund_code, validate_fund_stocks};

/// 基金行情客户端
///
/// 封装 `TdxHqClient`，提供基金专用的数据获取方法。
/// 自动处理基金代码验证和系数转换。
///
/// # Example
/// ```no_run
/// use tdxrs::fund::client::TdxHqFundClient;
/// use tdxrs::fund::constants::{MARKET_SH, MARKET_SZ};
///
/// let client = TdxHqFundClient::new();
/// client.connect_to_any(None).unwrap();
///
/// // 获取基金列表
/// let funds = client.get_fund_list(MARKET_SH).unwrap();
/// println!("Found {} funds in SH market", funds.len());
///
/// // 获取 ETF K线
/// let bars = client.get_fund_bars(KLINE_DAILY, MARKET_SH, "510300", 0, 100).unwrap();
/// println!("Got {} bars", bars.len());
/// ```
pub struct TdxHqFundClient {
    inner: TdxHqClient,
}

impl TdxHqFundClient {
    /// 创建新的基金客户端
    pub fn new() -> Self {
        Self {
            inner: TdxHqClient::new(),
        }
    }

    /// 连接到指定服务器
    pub fn connect(&self, ip: &str, port: u16, timeout: Option<f64>) -> Result<bool> {
        self.inner.connect(ip, port, timeout)
    }

    /// 连接到任意可用服务器
    pub fn connect_to_any(&self, timeout: Option<f64>) -> Result<bool> {
        self.inner.connect_to_any(timeout)
    }

    /// 断开连接
    pub fn disconnect(&self) {
        self.inner.disconnect()
    }

    /// 检查是否已连接
    pub fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    /// 设置握手模式 (委托到内部 TdxHqClient)
    ///
    /// Issue #12: 若服务器拒绝行情数据，切换为 `HandshakeMode::SkipCmd3`
    pub fn set_handshake_mode(&self, mode: crate::net::utils::HandshakeMode) {
        self.inner.set_handshake_mode(mode);
    }

    /// 获取当前握手模式
    pub fn handshake_mode(&self) -> crate::net::utils::HandshakeMode {
        self.inner.handshake_mode()
    }

    /// 获取基金列表
    ///
    /// 从证券列表中筛选出基金，返回基金基本信息列表。
    ///
    /// # Arguments
    ///
    /// * `market` - 市场代码 (0=深圳, 1=上海)
    ///
    /// # Returns
    ///
    /// 返回基金信息列表，包含代码、名称、基金类型、昨收价等。
    pub fn get_fund_list(&self, market: u8) -> Result<Vec<FundInfo>> {
        let total = self.inner.get_security_count(market)?;
        let mut funds = Vec::new();
        let page_size = 1000u16;

        for start in (0..total).step_by(page_size as usize) {
            let sec_list = self.inner.get_security_list(market, start)?;
            if sec_list.is_empty() {
                break;
            }

            for item in &sec_list {
                if is_fund(market, &item.code) {
                    let fund_type = classify_fund(market, &item.code);
                    funds.push(FundInfo {
                        market,
                        code: item.code.clone(),
                        name: item.name.clone(),
                        fund_type,
                        vol_unit: item.volunit as u32,
                        decimal_point: item.decimal_point as u32,
                        pre_close: item.pre_close,
                    });
                }
            }

            // 如果这一页最后一条已经超出基金范围，提前结束
            if let Some(last) = sec_list.last() {
                let last_is_beyond = match market {
                    MARKET_SH => {
                        !last.code.starts_with("50") && !last.code.starts_with("51")
                            && last.code.as_str() > "519999"
                    }
                    MARKET_SZ => {
                        !last.code.starts_with("15") && !last.code.starts_with("16")
                            && last.code.as_str() > "169999"
                    }
                    _ => false,
                };
                if last_is_beyond && !funds.is_empty() {
                    break;
                }
            }
        }

        Ok(funds)
    }

    /// 获取基金 K线数据
    ///
    /// 支持所有 K线周期 (5分钟、15分钟、日线、周线等)。
    /// 价格已应用基金系数 (0.001)。
    ///
    /// # Arguments
    ///
    /// * `category` - K线种类 (0=5分钟, 1=15分钟, 4=日线, 5=周线 等)
    /// * `market` - 市场代码
    /// * `code` - 基金代码
    /// * `start` - 起始偏移 (0=最新)
    /// * `count` - 数量 (最大 800)
    pub fn get_fund_bars(
        &self,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
    ) -> Result<Vec<FundBar>> {
        validate_fund_code(market, code).map_err(|e| {
            crate::error_codes::ErrorCode::FUND_CODE_NOT_SUPPORTED.err(e.to_string())
        })?;

        let bars = self.inner.get_security_bars(category, market, code, start, count, 0)?;
        Ok(bars.iter().map(FundBar::from_security_bar).collect())
    }

    /// 获取基金 K线数据 (自动分页)
    ///
    /// 自动翻页获取指定数量的 K线数据。
    pub fn get_fund_bars_all(
        &self,
        category: u8,
        market: u8,
        code: &str,
        count: u16,
    ) -> Result<Vec<FundBar>> {
        validate_fund_code(market, code).map_err(|e| {
            crate::error_codes::ErrorCode::FUND_CODE_NOT_SUPPORTED.err(e.to_string())
        })?;

        let bars = self.inner.get_security_bars_all(category, market, code, count, 0)?;
        Ok(bars.iter().map(FundBar::from_security_bar).collect())
    }

    /// 获取基金实时行情
    ///
    /// 批量获取多只基金的实时报价。
    /// 单次查询上限 60 只 (TDX 服务端硬限制)，超出自动截断并打印警告。
    /// 如需查询更多，请自行分组调用后合并结果。
    ///
    /// # Arguments
    ///
    /// * `stocks` - 基金列表 [(market, code), ...]
    pub fn get_fund_quotes(&self, stocks: &[(u8, &str)]) -> Result<Vec<FundQuote>> {
        validate_fund_stocks(stocks).map_err(|e| {
            crate::error_codes::ErrorCode::INVALID_FORMAT.err(e.to_string())
        })?;

        let quotes = self.inner.get_security_quotes(stocks)?;
        Ok(quotes.iter().map(FundQuote::from_security_quote).collect())
    }

    /// 获取基金分时数据
    ///
    /// 获取当日分时数据 (仅交易时段有数据)。
    pub fn get_fund_minute_time_data(&self, market: u8, code: &str) -> Result<Vec<MinuteTimePrice>> {
        validate_fund_code(market, code).map_err(|e| {
            crate::error_codes::ErrorCode::FUND_CODE_NOT_SUPPORTED.err(e.to_string())
        })?;

        self.inner.get_minute_time_data(market, code)
    }

    /// 获取基金历史分时数据
    pub fn get_fund_history_minute_time_data(
        &self,
        market: u8,
        code: &str,
        date: u32,
    ) -> Result<Vec<MinuteTimePrice>> {
        validate_fund_code(market, code).map_err(|e| {
            crate::error_codes::ErrorCode::FUND_CODE_NOT_SUPPORTED.err(e.to_string())
        })?;

        self.inner.get_history_minute_time_data(market, code, date)
    }

    /// 获取基金逐笔成交
    pub fn get_fund_transaction_data(
        &self,
        market: u8,
        code: &str,
        start: u16,
        count: u16,
    ) -> Result<Vec<TickData>> {
        validate_fund_code(market, code).map_err(|e| {
            crate::error_codes::ErrorCode::FUND_CODE_NOT_SUPPORTED.err(e.to_string())
        })?;

        self.inner.get_transaction_data(market, code, start, count)
    }

    /// 获取基金历史逐笔成交
    pub fn get_fund_history_transaction_data(
        &self,
        market: u8,
        code: &str,
        start: u16,
        count: u16,
        date: u32,
    ) -> Result<Vec<TickData>> {
        validate_fund_code(market, code).map_err(|e| {
            crate::error_codes::ErrorCode::FUND_CODE_NOT_SUPPORTED.err(e.to_string())
        })?;

        self.inner
            .get_history_transaction_data(market, code, start, count, date)
    }

    /// 获取基金除权除息信息
    pub fn get_fund_xdxr_info(&self, market: u8, code: &str) -> Result<Vec<FundXdXrInfo>> {
        validate_fund_code(market, code).map_err(|e| {
            crate::error_codes::ErrorCode::FUND_CODE_NOT_SUPPORTED.err(e.to_string())
        })?;

        let xdxr = self.inner.get_xdxr_info(market, code)?;
        Ok(xdxr.iter().map(FundXdXrInfo::from_xdxr_info).collect())
    }

    /// 获取基金财务信息
    pub fn get_fund_finance_info(&self, market: u8, code: &str) -> Result<FundFinanceInfo> {
        validate_fund_code(market, code).map_err(|e| {
            crate::error_codes::ErrorCode::FUND_CODE_NOT_SUPPORTED.err(e.to_string())
        })?;

        let info = self.inner.get_finance_info(market, code)?;
        Ok(FundFinanceInfo::from_finance_info(&info))
    }

}

impl Default for TdxHqFundClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::constants::KLINE_DAILY;

    #[test]
    fn test_fund_client_creation() {
        let client = TdxHqFundClient::new();
        assert!(!client.is_connected());
    }

    #[test]
    fn test_fund_client_default() {
        let client = TdxHqFundClient::default();
        assert!(!client.is_connected());
    }

    #[test]
    fn test_get_fund_bars_invalid_code() {
        let client = TdxHqFundClient::new();
        let result = client.get_fund_bars(KLINE_DAILY, 1, "600519", 0, 10);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_fund_quotes_invalid_code() {
        let client = TdxHqFundClient::new();
        let result = client.get_fund_quotes(&[(1, "600519")]);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_fund_xdxr_info_invalid_code() {
        let client = TdxHqFundClient::new();
        let result = client.get_fund_xdxr_info(1, "600519");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_fund_finance_info_invalid_code() {
        let client = TdxHqFundClient::new();
        let result = client.get_fund_finance_info(1, "600519");
        assert!(result.is_err());
    }
}
