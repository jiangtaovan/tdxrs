//! 基金客户端 Python 绑定
//!
//! 提供基金 (ETF、LOF、REITs、分级基金等) 数据的 Python 接口。
//! 向后兼容 TdxHqEtfClient。

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::fund::client::TdxHqFundClient;
use crate::fund::constants as fund_const;

/// 基金行情客户端 - Python 绑定
///
/// 封装 TdxHqClient，提供基金专用的 API。
/// 自动处理基金代码验证和系数转换。
///
/// # Example
///
/// ```python
/// from tdxrs._internal import TdxHqFundClient
/// from tdxrs.constants import MARKET_SH, MARKET_SZ, KLINE_DAILY
///
/// client = TdxHqFundClient()
/// client.connect_to_any()
///
/// # 获取基金列表
/// funds = client.get_fund_list(MARKET_SH)
/// print(f"Found {len(funds)} funds")
///
/// # 获取 ETF K线
/// bars = client.get_fund_bars(KLINE_DAILY, MARKET_SH, "510300", 0, 100)
/// for bar in bars:
///     print(f"{bar['datetime']}: {bar['open']:.3f}")
/// ```
#[pyclass(name = "TdxHqFundClient")]
pub struct PyTdxHqFundClient {
    client: TdxHqFundClient,
}

#[pymethods]
impl PyTdxHqFundClient {
    /// 创建新的基金客户端
    #[new]
    fn new() -> Self {
        Self {
            client: TdxHqFundClient::new(),
        }
    }

    /// 连接到 TDX 服务器
    #[pyo3(signature = (ip, port, timeout=None))]
    fn connect(&self, ip: &str, port: u16, timeout: Option<f64>) -> PyResult<bool> {
        self.client
            .connect(ip, port, timeout)
            .map_err(|e| pyo3::exceptions::PyConnectionError::new_err(e.to_string()))
    }

    /// 连接到任意可用服务器
    #[pyo3(signature = (timeout=None))]
    fn connect_to_any(&self, timeout: Option<f64>) -> PyResult<bool> {
        self.client
            .connect_to_any(timeout)
            .map_err(|e| pyo3::exceptions::PyConnectionError::new_err(e.to_string()))
    }

    /// 断开连接
    fn disconnect(&self) {
        self.client.disconnect();
    }

    /// 是否已连接
    fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    /// 设置握手模式 (Issue #12)
    ///
    /// 可选值: "full" / "skip_cmd3" / "cmd1_only" / "none"
    #[pyo3(signature = (mode))]
    fn set_handshake_mode(&self, mode: &str) -> PyResult<()> {
        let m = match mode {
            "full" | "Full" => HandshakeMode::Full,
            "skip_cmd3" | "SkipCmd3" => HandshakeMode::SkipCmd3,
            "cmd1_only" | "Cmd1Only" => HandshakeMode::Cmd1Only,
            "none" | "None" => HandshakeMode::None,
            _ => {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "mode must be one of: 'full', 'skip_cmd3', 'cmd1_only', 'none'"
                ));
            }
        };
        self.client.set_handshake_mode(m);
        Ok(())
    }

    /// 获取当前握手模式
    fn handshake_mode(&self) -> &'static str {
        match self.client.handshake_mode() {
            HandshakeMode::Full => "full",
            HandshakeMode::SkipCmd3 => "skip_cmd3",
            HandshakeMode::Cmd1Only => "cmd1_only",
            HandshakeMode::None => "none",
        }
    }

    /// 获取基金列表
    ///
    /// Args:
    ///     market: 市场代码 (0=深圳, 1=上海)
    ///
    /// Returns:
    ///     list[dict]: 基金信息列表
    #[pyo3(signature = (market,))]
    fn get_fund_list(&self, py: Python<'_>, market: u8) -> PyResult<Py<PyAny>> {
        let funds = self
            .client
            .get_fund_list(market)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for fund in &funds {
            let dict = PyDict::new(py);
            dict.set_item("market", fund.market)?;
            dict.set_item("code", &fund.code)?;
            dict.set_item("name", &fund.name)?;
            dict.set_item("fund_type", fund.fund_type.name())?;
            dict.set_item("fund_type_zh", fund.fund_type.name_zh())?;
            dict.set_item("vol_unit", fund.vol_unit)?;
            dict.set_item("decimal_point", fund.decimal_point)?;
            dict.set_item("pre_close", fund.pre_close)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取基金 K线数据
    #[pyo3(signature = (category, market, code, start=0, count=800))]
    fn get_fund_bars(
        &self,
        py: Python<'_>,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
    ) -> PyResult<Py<PyAny>> {
        let bars = self
            .client
            .get_fund_bars(category, market, code, start, count)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for b in &bars {
            let dict = PyDict::new(py);
            dict.set_item("open", b.open)?;
            dict.set_item("close", b.close)?;
            dict.set_item("high", b.high)?;
            dict.set_item("low", b.low)?;
            dict.set_item("vol", b.vol)?;
            dict.set_item("amount", b.amount)?;
            dict.set_item("year", b.year)?;
            dict.set_item("month", b.month)?;
            dict.set_item("day", b.day)?;
            dict.set_item("hour", b.hour)?;
            dict.set_item("minute", b.minute)?;
            dict.set_item("datetime", &b.datetime)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取基金 K线数据 (自动分页)
    #[pyo3(signature = (category, market, code, count=800))]
    fn get_fund_bars_all(
        &self,
        py: Python<'_>,
        category: u8,
        market: u8,
        code: &str,
        count: u16,
    ) -> PyResult<Py<PyAny>> {
        let bars = self
            .client
            .get_fund_bars_all(category, market, code, count)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for b in &bars {
            let dict = PyDict::new(py);
            dict.set_item("open", b.open)?;
            dict.set_item("close", b.close)?;
            dict.set_item("high", b.high)?;
            dict.set_item("low", b.low)?;
            dict.set_item("vol", b.vol)?;
            dict.set_item("amount", b.amount)?;
            dict.set_item("year", b.year)?;
            dict.set_item("month", b.month)?;
            dict.set_item("day", b.day)?;
            dict.set_item("hour", b.hour)?;
            dict.set_item("minute", b.minute)?;
            dict.set_item("datetime", &b.datetime)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取基金实时行情
    #[pyo3(signature = (stocks,))]
    fn get_fund_quotes(&self, py: Python<'_>, stocks: Vec<(u8, String)>) -> PyResult<Py<PyAny>> {
        let stock_refs: Vec<(u8, &str)> = stocks.iter().map(|(m, c)| (*m, c.as_str())).collect();
        let quotes = self
            .client
            .get_fund_quotes(&stock_refs)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for q in &quotes {
            let dict = PyDict::new(py);
            dict.set_item("market", q.market)?;
            dict.set_item("code", &q.code)?;
            dict.set_item("price", q.price)?;
            dict.set_item("last_close", q.last_close)?;
            dict.set_item("open", q.open)?;
            dict.set_item("high", q.high)?;
            dict.set_item("low", q.low)?;
            dict.set_item("vol", q.vol)?;
            dict.set_item("amount", q.amount)?;
            dict.set_item("bid1", q.bid1)?;
            dict.set_item("bid_vol1", q.bid_vol1)?;
            dict.set_item("bid2", q.bid2)?;
            dict.set_item("bid_vol2", q.bid_vol2)?;
            dict.set_item("bid3", q.bid3)?;
            dict.set_item("bid_vol3", q.bid_vol3)?;
            dict.set_item("bid4", q.bid4)?;
            dict.set_item("bid_vol4", q.bid_vol4)?;
            dict.set_item("bid5", q.bid5)?;
            dict.set_item("bid_vol5", q.bid_vol5)?;
            dict.set_item("ask1", q.ask1)?;
            dict.set_item("ask_vol1", q.ask_vol1)?;
            dict.set_item("ask2", q.ask2)?;
            dict.set_item("ask_vol2", q.ask_vol2)?;
            dict.set_item("ask3", q.ask3)?;
            dict.set_item("ask_vol3", q.ask_vol3)?;
            dict.set_item("ask4", q.ask4)?;
            dict.set_item("ask_vol4", q.ask_vol4)?;
            dict.set_item("ask5", q.ask5)?;
            dict.set_item("ask_vol5", q.ask_vol5)?;
            dict.set_item("servertime", &q.servertime)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取基金分时数据
    #[pyo3(signature = (market, code))]
    fn get_fund_minute_time_data(
        &self,
        py: Python<'_>,
        market: u8,
        code: &str,
    ) -> PyResult<Py<PyAny>> {
        let data = self
            .client
            .get_fund_minute_time_data(market, code)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for item in &data {
            let dict = PyDict::new(py);
            dict.set_item("time", &item.time)?;
            dict.set_item("price", item.price)?;
            dict.set_item("avg_price", item.avg_price)?;
            dict.set_item("vol", item.vol)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取基金历史分时数据
    #[pyo3(signature = (market, code, date))]
    fn get_fund_history_minute_time_data(
        &self,
        py: Python<'_>,
        market: u8,
        code: &str,
        date: u32,
    ) -> PyResult<Py<PyAny>> {
        let data = self
            .client
            .get_fund_history_minute_time_data(market, code, date)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for item in &data {
            let dict = PyDict::new(py);
            dict.set_item("time", &item.time)?;
            dict.set_item("price", item.price)?;
            dict.set_item("avg_price", item.avg_price)?;
            dict.set_item("vol", item.vol)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取基金逐笔成交
    #[pyo3(signature = (market, code, start=0, count=2000))]
    fn get_fund_transaction_data(
        &self,
        py: Python<'_>,
        market: u8,
        code: &str,
        start: u16,
        count: u16,
    ) -> PyResult<Py<PyAny>> {
        let data = self
            .client
            .get_fund_transaction_data(market, code, start, count)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for item in &data {
            let dict = PyDict::new(py);
            dict.set_item("time", &item.time)?;
            dict.set_item("price", item.price)?;
            dict.set_item("vol", item.vol)?;
            dict.set_item("num", item.num)?;
            dict.set_item("buyorsell", item.buyorsell)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取基金历史逐笔成交
    #[pyo3(signature = (market, code, start, count, date))]
    fn get_fund_history_transaction_data(
        &self,
        py: Python<'_>,
        market: u8,
        code: &str,
        start: u16,
        count: u16,
        date: u32,
    ) -> PyResult<Py<PyAny>> {
        let data = self
            .client
            .get_fund_history_transaction_data(market, code, start, count, date)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for item in &data {
            let dict = PyDict::new(py);
            dict.set_item("time", &item.time)?;
            dict.set_item("price", item.price)?;
            dict.set_item("vol", item.vol)?;
            dict.set_item("buyorsell", item.buyorsell)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取基金除权除息信息
    #[pyo3(signature = (market, code))]
    fn get_fund_xdxr_info(
        &self,
        py: Python<'_>,
        market: u8,
        code: &str,
    ) -> PyResult<Py<PyAny>> {
        let data = self
            .client
            .get_fund_xdxr_info(market, code)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for item in &data {
            let dict = PyDict::new(py);
            dict.set_item("year", item.year)?;
            dict.set_item("month", item.month)?;
            dict.set_item("day", item.day)?;
            dict.set_item("category", item.category)?;
            dict.set_item("fenhong", item.fenhong)?;
            dict.set_item("peigujia", item.peigujia)?;
            dict.set_item("songzhuangu", item.songzhuangu)?;
            dict.set_item("peigu", item.peigu)?;
            dict.set_item("suogu", item.suogu)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取基金财务信息
    #[pyo3(signature = (market, code))]
    fn get_fund_finance_info(
        &self,
        py: Python<'_>,
        market: u8,
        code: &str,
    ) -> PyResult<Py<PyAny>> {
        let info = self
            .client
            .get_fund_finance_info(market, code)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let dict = PyDict::new(py);
        dict.set_item("market", info.market)?;
        dict.set_item("code", &info.code)?;
        dict.set_item("zongguben", info.zongguben)?;
        dict.set_item("liutongguben", info.liutongguben)?;
        dict.set_item("meigujingzichan", info.meigujingzichan)?;
        dict.set_item("zongzichan", info.zongzichan)?;
        dict.set_item("jingzichan", info.jingzichan)?;
        Ok(dict.into())
    }

    // ============================================================
    // 静态方法
    // ============================================================

    /// 判断是否为基金代码
    #[staticmethod]
    fn is_fund(market: u8, code: &str) -> bool {
        fund_const::is_fund(market, code)
    }

    /// 分类基金类型
    ///
    /// Returns:
    ///     str: 基金类型名称 (ETF/LOF/REITs/Structured/OpenEnd/Bond/Money/Other)
    #[staticmethod]
    fn classify_fund(market: u8, code: &str) -> &str {
        fund_const::classify_fund(market, code).name()
    }

    /// 自动判断市场代码
    #[staticmethod]
    fn auto_market_code(code: &str) -> u8 {
        fund_const::auto_market_code(code)
    }
}

impl Default for PyTdxHqFundClient {
    fn default() -> Self {
        Self::new()
    }
}

// ================================================================
// TdxBlockClient Python 绑定
// ================================================================

use crate::block::client::TdxBlockClient;
use crate::net::utils::HandshakeMode;

/// 板块行情客户端 - Python 绑定
///
/// 封装 TdxDirectClient，提供板块专用 API。
/// 内置板块查询限制 (1min 禁用，分钟级默认 50 条)。
///
/// # Example
///
/// ```python
/// from tdxrs._internal import TdxBlockClient
///
/// client = TdxBlockClient("58.63.254.191", 7709, 5.0)
/// bars = client.get_block_bars(4, "880001", 0, 100)  # 日K
/// quotes = client.get_block_quotes(["880001", "880002"])
/// ```
#[pyclass(name = "TdxBlockClient")]
pub struct PyTdxBlockClient {
    client: TdxBlockClient,
}

#[pymethods]
impl PyTdxBlockClient {
    /// 创建板块客户端
    #[new]
    #[pyo3(signature = (ip, port, timeout))]
    fn new(ip: &str, port: u16, timeout: f64) -> Self {
        Self {
            client: TdxBlockClient::new(ip, port, timeout),
        }
    }

    /// 更新服务器
    fn set_server(&self, ip: &str, port: u16) {
        self.client.set_server(ip, port);
    }

    /// 更新超时
    fn set_timeout(&self, timeout: f64) {
        self.client.set_timeout(timeout);
    }

    /// 设置握手模式 (Issue #12)
    ///
    /// 可选值: "full" / "skip_cmd3" / "cmd1_only" / "none"
    #[pyo3(signature = (mode))]
    fn set_handshake_mode(&self, mode: &str) -> PyResult<()> {
        let m = match mode {
            "full" | "Full" => HandshakeMode::Full,
            "skip_cmd3" | "SkipCmd3" => HandshakeMode::SkipCmd3,
            "cmd1_only" | "Cmd1Only" => HandshakeMode::Cmd1Only,
            "none" | "None" => HandshakeMode::None,
            _ => {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "mode must be one of: 'full', 'skip_cmd3', 'cmd1_only', 'none'"
                ));
            }
        };
        self.client.set_handshake_mode(m);
        Ok(())
    }

    /// 获取当前握手模式
    fn handshake_mode(&self) -> &'static str {
        match self.client.handshake_mode() {
            HandshakeMode::Full => "full",
            HandshakeMode::SkipCmd3 => "skip_cmd3",
            HandshakeMode::Cmd1Only => "cmd1_only",
            HandshakeMode::None => "none",
        }
    }

    /// 获取板块 K 线数据
    ///
    /// Args:
    ///     category: K线种类 (0=5min, 1=15min, 3=60min, 4=day, 5=week, 6=month)
    ///     code: 板块代码 (88xxxx)
    ///     start: 起始位置
    ///     count: 请求条数 (0=使用默认值)
    ///
    /// Returns:
    ///     list[dict]: K线数据列表
    #[pyo3(signature = (category, code, start=0, count=0))]
    fn get_block_bars(
        &self,
        py: Python<'_>,
        category: u8,
        code: &str,
        start: u32,
        count: u16,
    ) -> PyResult<Py<PyAny>> {
        let bars = self
            .client
            .get_block_bars(category, code, start, count)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for bar in &bars {
            let dict = PyDict::new(py);
            dict.set_item("datetime", &bar.datetime)?;
            dict.set_item("open", bar.open)?;
            dict.set_item("high", bar.high)?;
            dict.set_item("low", bar.low)?;
            dict.set_item("close", bar.close)?;
            dict.set_item("amount", bar.amount)?;
            dict.set_item("vol", bar.vol)?;
            dict.set_item("year", bar.year)?;
            dict.set_item("month", bar.month)?;
            dict.set_item("day", bar.day)?;
            dict.set_item("hour", bar.hour)?;
            dict.set_item("minute", bar.minute)?;
            dict.set_item("up_count", bar.up_count)?;
            dict.set_item("down_count", bar.down_count)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取板块 K 线 (默认条数)
    #[pyo3(signature = (category, code))]
    fn get_block_bars_default(
        &self,
        py: Python<'_>,
        category: u8,
        code: &str,
    ) -> PyResult<Py<PyAny>> {
        self.get_block_bars(py, category, code, 0, 0)
    }

    /// 获取板块实时行情
    ///
    /// Args:
    ///     codes: 板块代码列表 (88xxxx)
    ///
    /// Returns:
    ///     list[dict]: 行情数据列表
    fn get_block_quotes(&self, py: Python<'_>, codes: Vec<String>) -> PyResult<Py<PyAny>> {
        let code_refs: Vec<&str> = codes.iter().map(|s| s.as_str()).collect();
        let quotes = self
            .client
            .get_block_quotes(&code_refs)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for q in &quotes {
            let dict = PyDict::new(py);
            dict.set_item("market", q.market)?;
            dict.set_item("code", &q.code)?;
            dict.set_item("price", q.price)?;
            dict.set_item("last_close", q.last_close)?;
            dict.set_item("open", q.open)?;
            dict.set_item("high", q.high)?;
            dict.set_item("low", q.low)?;
            dict.set_item("vol", q.vol)?;
            dict.set_item("amount", q.amount)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    // ================================================================
    // 板块列表 (从服务器下载 .dat 文件)
    // ================================================================

    /// 从服务器下载并解析板块文件
    ///
    /// Args:
    ///     block_file: 文件名，如 "block_fg.dat", "block_gn.dat", "block_zs.dat"
    ///
    /// Returns:
    ///     list[dict]: 板块成分股级别的记录，每条含 blockname, block_type, code_index, code
    fn get_block_list(&self, py: Python<'_>, block_file: &str) -> PyResult<Py<PyAny>> {
        let data = self
            .client
            .get_block_list(block_file)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        let list = PyList::empty(py);
        for d in &data {
            let dict = PyDict::new(py);
            dict.set_item("blockname", &d.blockname)?;
            dict.set_item("block_type", d.block_type)?;
            dict.set_item("code_index", d.code_index)?;
            dict.set_item("code", &d.code)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取行业板块列表 (block_fg.dat)
    ///
    /// Returns:
    ///     list[dict]: 筛选类标签板块，如融资融券、破净资产、高股息等
    fn get_industry_blocks(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.get_block_list(py, crate::protocol::constants::BLOCK_FG)
    }

    /// 获取概念板块列表 (block_gn.dat)
    ///
    /// Returns:
    ///     list[dict]: 概念板块，如5G概念、一带一路、碳中和等
    fn get_concept_blocks(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.get_block_list(py, crate::protocol::constants::BLOCK_GN)
    }

    /// 获取指数成分列表 (block_zs.dat)
    ///
    /// Returns:
    ///     list[dict]: 指数成分板块，如沪深300、上证50、创业板指等
    fn get_index_blocks(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.get_block_list(py, crate::protocol::constants::BLOCK_SZ)
    }
}
