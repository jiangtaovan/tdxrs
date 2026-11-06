//! AsyncTdxHqClient Python 绑定
//!
//! 基于 tokio 异步客户端的同步 Python 包装，内部持有独立 Runtime。
//! API 与 `TdxHqClient` 完全一致，底层使用通道化连接池实现并发。

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};
use pyo3::IntoPyObjectExt;

use crate::error::TdxError;
use crate::net::async_client::AsyncTdxHqClient;
use crate::net::utils::{HandshakeMode, TradingPhase};

/// 将 TdxError 转换为 Python 异常
fn to_py_err(e: TdxError) -> PyErr {
    match &e {
        TdxError::Coded(coded) => pyo3::exceptions::PyValueError::new_err(coded.format()),
        TdxError::Connection(_) | TdxError::ConnectionTimeout | TdxError::Disconnected => {
            pyo3::exceptions::PyConnectionError::new_err(e.to_string())
        }
        _ => pyo3::exceptions::PyValueError::new_err(e.to_string()),
    }
}

/// 异步 TDX 行情客户端 — Python 绑定
///
/// 内部持有独立 tokio Runtime，底层使用通道化连接池实现并发。
/// API 与 `TdxHqClient` 完全一致。
///
/// ```python
/// from tdxrs._internal import AsyncTdxHqClient
///
/// client = AsyncTdxHqClient()
/// client.connect("180.153.18.170", 7709)
/// bars = client.get_security_bars(4, 1, "600519", 0, 100)
/// client.disconnect()
/// ```
#[pyclass(name = "AsyncTdxHqClient")]
pub struct PyAsyncTdxHqClient {
    rt: tokio::runtime::Runtime,
    client: AsyncTdxHqClient,
}

#[pymethods]
impl PyAsyncTdxHqClient {
    #[new]
    fn new() -> Self {
        Self {
            rt: tokio::runtime::Runtime::new().expect("failed to create tokio runtime"),
            client: AsyncTdxHqClient::new(),
        }
    }

    /// 指定连接池大小创建
    #[staticmethod]
    #[pyo3(signature = (pool_size=4))]
    fn with_pool_size(pool_size: usize) -> Self {
        Self {
            rt: tokio::runtime::Runtime::new().expect("failed to create tokio runtime"),
            client: AsyncTdxHqClient::with_pool_size(pool_size),
        }
    }

    // ============================================================
    // 连接管理
    // ============================================================

    /// 连接到 TDX 服务器 (建立连接池)
    #[pyo3(signature = (ip, port, timeout=None))]
    fn connect(&self, ip: &str, port: u16, timeout: Option<f64>) -> PyResult<bool> {
        self.rt
            .block_on(self.client.connect(ip, port, timeout))
            .map_err(to_py_err)
    }

    /// 连接到任意可用服务器
    #[pyo3(signature = (timeout=None))]
    fn connect_to_any(&self, timeout: Option<f64>) -> PyResult<bool> {
        self.rt
            .block_on(self.client.connect_to_any(timeout))
            .map_err(to_py_err)
    }

    /// 断开所有连接
    fn disconnect(&self) {
        self.rt.block_on(self.client.disconnect());
    }

    /// 当前连接数
    fn connection_count(&self) -> usize {
        self.rt.block_on(self.client.connection_count())
    }

    /// 连接是否存活
    fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    // ============================================================
    // 配置
    // ============================================================

    /// 设置限流 RPS (每秒请求数, 0=禁用, 上限 200)
    fn set_rate_limit(&mut self, rps: u32) {
        self.client.set_rate_limit(rps);
    }

    /// 设置交易阶段限流 (15/30/60 req/s)
    fn set_phase(&mut self, phase: &str) {
        let p = match phase {
            "trading" => TradingPhase::Trading,
            "prepost" => TradingPhase::PrePost,
            "closed" => TradingPhase::Closed,
            _ => return,
        };
        self.client.set_phase(p);
    }

    /// 自动检测交易阶段并设置限流，返回阶段名称
    fn auto_detect_phase(&mut self) -> String {
        let phase = self.client.auto_detect_phase();
        match phase {
            TradingPhase::Trading => "trading".to_string(),
            TradingPhase::PrePost => "prepost".to_string(),
            TradingPhase::Closed => "closed".to_string(),
        }
    }

    /// 设置握手模式 (Issue #15)
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

    // ============================================================
    // K线 — dict 输出
    // ============================================================

    /// 获取K线数据
    ///
    /// fq: 复权类型, 0=未复权 1=前复权(默认) 2=后复权
    #[pyo3(signature = (category, market, code, start=0, count=800, fq=1))]
    fn get_security_bars(
        &self,
        py: Python<'_>,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
        fq: u8,
    ) -> PyResult<Py<PyAny>> {
        let bars = self
            .rt
            .block_on(self.client.get_security_bars(category, market, code, start, count, fq))
            .map_err(to_py_err)?;

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

    /// 获取K线数据 (自动分页)
    #[pyo3(signature = (category, market, code, count=800, fq=1))]
    fn get_security_bars_all(
        &self,
        py: Python<'_>,
        category: u8,
        market: u8,
        code: &str,
        count: u16,
        fq: u8,
    ) -> PyResult<Py<PyAny>> {
        let bars = self
            .rt
            .block_on(self.client.get_security_bars_all(category, market, code, count, fq))
            .map_err(to_py_err)?;

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

    /// 获取指数K线
    #[pyo3(signature = (category, market, code, start=0, count=800, fq=1))]
    fn get_index_bars(
        &self,
        py: Python<'_>,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
        fq: u8,
    ) -> PyResult<Py<PyAny>> {
        let bars = self
            .rt
            .block_on(self.client.get_index_bars(category, market, code, start, count, fq))
            .map_err(to_py_err)?;

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
            dict.set_item("up_count", b.up_count)?;
            dict.set_item("down_count", b.down_count)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取指数K线 (自动分页)
    #[pyo3(signature = (category, market, code, count=800, fq=1))]
    fn get_index_bars_all(
        &self,
        py: Python<'_>,
        category: u8,
        market: u8,
        code: &str,
        count: u16,
        fq: u8,
    ) -> PyResult<Py<PyAny>> {
        let bars = self
            .rt
            .block_on(self.client.get_index_bars(category, market, code, 0, count, fq))
            .map_err(to_py_err)?;

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
            dict.set_item("up_count", b.up_count)?;
            dict.set_item("down_count", b.down_count)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    // ============================================================
    // 实时行情
    // ============================================================

    /// 获取实时行情 (批量)
    fn get_security_quotes(
        &self,
        py: Python<'_>,
        all_stock: Vec<(u8, String)>,
    ) -> PyResult<Py<PyAny>> {
        let refs: Vec<(u8, &str)> = all_stock.iter().map(|(m, c)| (*m, c.as_str())).collect();
        let quotes = self
            .rt
            .block_on(self.client.get_security_quotes(&refs))
            .map_err(to_py_err)?;

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
            dict.set_item("cur_vol", q.cur_vol)?;
            dict.set_item("amount", q.amount)?;
            dict.set_item("s_vol", q.s_vol)?;
            dict.set_item("b_vol", q.b_vol)?;
            dict.set_item("bid1", q.bid1)?;
            dict.set_item("bid_vol1", q.bid_vol1)?;
            dict.set_item("ask1", q.ask1)?;
            dict.set_item("ask_vol1", q.ask_vol1)?;
            dict.set_item("bid2", q.bid2)?;
            dict.set_item("bid_vol2", q.bid_vol2)?;
            dict.set_item("ask2", q.ask2)?;
            dict.set_item("ask_vol2", q.ask_vol2)?;
            dict.set_item("bid3", q.bid3)?;
            dict.set_item("bid_vol3", q.bid_vol3)?;
            dict.set_item("ask3", q.ask3)?;
            dict.set_item("ask_vol3", q.ask_vol3)?;
            dict.set_item("bid4", q.bid4)?;
            dict.set_item("bid_vol4", q.bid_vol4)?;
            dict.set_item("ask4", q.ask4)?;
            dict.set_item("ask_vol4", q.ask_vol4)?;
            dict.set_item("bid5", q.bid5)?;
            dict.set_item("bid_vol5", q.bid_vol5)?;
            dict.set_item("ask5", q.ask5)?;
            dict.set_item("ask_vol5", q.ask_vol5)?;
            dict.set_item("servertime", &q.servertime)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    // ============================================================
    // 证券列表 / 数量
    // ============================================================

    /// 获取证券数量
    fn get_security_count(&self, market: u8) -> PyResult<u16> {
        self.rt
            .block_on(self.client.get_security_count(market))
            .map_err(to_py_err)
    }

    /// 获取证券列表
    #[pyo3(signature = (market, start=0))]
    fn get_security_list(
        &self,
        py: Python<'_>,
        market: u8,
        start: u16,
    ) -> PyResult<Py<PyAny>> {
        let data = self
            .rt
            .block_on(self.client.get_security_list(market, start))
            .map_err(to_py_err)?;

        let list = PyList::empty(py);
        for d in &data {
            let dict = PyDict::new(py);
            dict.set_item("code", &d.code)?;
            dict.set_item("volunit", d.volunit)?;
            dict.set_item("decimal_point", d.decimal_point)?;
            dict.set_item("name", &d.name)?;
            dict.set_item("pre_close", d.pre_close)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    // ============================================================
    // 分时 / 逐笔
    // ============================================================

    /// 获取分时数据
    fn get_minute_time_data(
        &self,
        py: Python<'_>,
        market: u8,
        code: &str,
    ) -> PyResult<Py<PyAny>> {
        let data = self
            .rt
            .block_on(self.client.get_minute_time_data(market, code))
            .map_err(to_py_err)?;

        let list = PyList::empty(py);
        for d in &data {
            let dict = PyDict::new(py);
            dict.set_item("time", &d.time)?;
            dict.set_item("price", d.price)?;
            dict.set_item("avg_price", d.avg_price)?;
            dict.set_item("vol", d.vol)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取历史分时数据
    fn get_history_minute_time_data(
        &self,
        py: Python<'_>,
        market: u8,
        code: &str,
        date: u32,
    ) -> PyResult<Py<PyAny>> {
        let data = self
            .rt
            .block_on(self.client.get_history_minute_time_data(market, code, date))
            .map_err(to_py_err)?;

        let list = PyList::empty(py);
        for d in &data {
            let dict = PyDict::new(py);
            dict.set_item("time", &d.time)?;
            dict.set_item("price", d.price)?;
            dict.set_item("avg_price", d.avg_price)?;
            dict.set_item("vol", d.vol)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取逐笔成交
    #[pyo3(signature = (market, code, start=0, count=2000))]
    fn get_transaction_data(
        &self,
        py: Python<'_>,
        market: u8,
        code: &str,
        start: u16,
        count: u16,
    ) -> PyResult<Py<PyAny>> {
        let data = self
            .rt
            .block_on(self.client.get_transaction_data(market, code, start, count))
            .map_err(to_py_err)?;

        let list = PyList::empty(py);
        for d in &data {
            let dict = PyDict::new(py);
            dict.set_item("time", &d.time)?;
            dict.set_item("price", d.price)?;
            dict.set_item("vol", d.vol)?;
            dict.set_item("num", d.num)?;
            dict.set_item("buyorsell", d.buyorsell)?;
            dict.set_item("reserved", d.reserved)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    /// 获取历史逐笔成交
    #[pyo3(signature = (market, code, start=0, count=2000, date=0))]
    fn get_history_transaction_data(
        &self,
        py: Python<'_>,
        market: u8,
        code: &str,
        start: u16,
        count: u16,
        date: u32,
    ) -> PyResult<Py<PyAny>> {
        let data = self
            .rt
            .block_on(
                self.client
                    .get_history_transaction_data(market, code, start, count, date),
            )
            .map_err(to_py_err)?;

        let list = PyList::empty(py);
        for d in &data {
            let dict = PyDict::new(py);
            dict.set_item("time", &d.time)?;
            dict.set_item("price", d.price)?;
            dict.set_item("vol", d.vol)?;
            dict.set_item("num", d.num)?;
            dict.set_item("buyorsell", d.buyorsell)?;
            dict.set_item("reserved", d.reserved)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    // ============================================================
    // 财务 / 除权除息
    // ============================================================

    /// 获取财务信息
    fn get_finance_info(
        &self,
        py: Python<'_>,
        market: u8,
        code: &str,
    ) -> PyResult<Py<PyAny>> {
        let info = self
            .rt
            .block_on(self.client.get_finance_info(market, code))
            .map_err(to_py_err)?;

        let dict = PyDict::new(py);
        dict.set_item("market", info.market)?;
        dict.set_item("code", &info.code)?;
        dict.set_item("liutongguben", info.liutongguben)?;
        dict.set_item("province", info.province)?;
        dict.set_item("industry", info.industry)?;
        dict.set_item("updated_date", info.updated_date)?;
        dict.set_item("ipo_date", info.ipo_date)?;
        dict.set_item("zongguben", info.zongguben)?;
        dict.set_item("guojiagu", info.guojiagu)?;
        dict.set_item("faqirenfarengu", info.faqirenfarengu)?;
        dict.set_item("farengu", info.farengu)?;
        dict.set_item("bgu", info.bgu)?;
        dict.set_item("hgu", info.hgu)?;
        dict.set_item("zhigonggu", info.zhigonggu)?;
        dict.set_item("zongzichan", info.zongzichan)?;
        dict.set_item("liudongzichan", info.liudongzichan)?;
        dict.set_item("gudingzichan", info.gudingzichan)?;
        dict.set_item("wuxingzichan", info.wuxingzichan)?;
        dict.set_item("gudongrenshu", info.gudongrenshu)?;
        dict.set_item("liudongfuzhai", info.liudongfuzhai)?;
        dict.set_item("changqifuzhai", info.changqifuzhai)?;
        dict.set_item("zibengongjijin", info.zibengongjijin)?;
        dict.set_item("jingzichan", info.jingzichan)?;
        dict.set_item("zhuyingshouru", info.zhuyingshouru)?;
        dict.set_item("zhuyinglirun", info.zhuyinglirun)?;
        dict.set_item("yingshouzhangkuan", info.yingshouzhangkuan)?;
        dict.set_item("yingyelirun", info.yingyelirun)?;
        dict.set_item("touzishouyu", info.touzishouyu)?;
        dict.set_item("jingyingxianjinliu", info.jingyingxianjinliu)?;
        dict.set_item("zongxianjinliu", info.zongxianjinliu)?;
        dict.set_item("cunhuo", info.cunhuo)?;
        dict.set_item("lirunzonghe", info.lirunzonghe)?;
        dict.set_item("shuihoulirun", info.shuihoulirun)?;
        dict.set_item("jinglirun", info.jinglirun)?;
        dict.set_item("weifenpeilirun", info.weifenpeilirun)?;
        dict.set_item("meigujingzichan", info.meigujingzichan)?;
        Ok(dict.into())
    }

    /// 获取除权除息
    fn get_xdxr_info(
        &self,
        py: Python<'_>,
        market: u8,
        code: &str,
    ) -> PyResult<Py<PyAny>> {
        let data = self
            .rt
            .block_on(self.client.get_xdxr_info(market, code))
            .map_err(to_py_err)?;

        let list = PyList::empty(py);
        for d in &data {
            let dict = PyDict::new(py);
            dict.set_item("year", d.year)?;
            dict.set_item("month", d.month)?;
            dict.set_item("day", d.day)?;
            dict.set_item("category", d.category)?;
            dict.set_item("name", &d.name)?;
            dict.set_item("fenhong", d.fenhong)?;
            dict.set_item("peigujia", d.peigujia)?;
            dict.set_item("songzhuangu", d.songzhuangu)?;
            dict.set_item("peigu", d.peigu)?;
            dict.set_item("suogu", d.suogu)?;
            dict.set_item("panqianliutong", d.panqianliutong)?;
            dict.set_item("panhouliutong", d.panhouliutong)?;
            dict.set_item("qianzongguben", d.qianzongguben)?;
            dict.set_item("houzongguben", d.houzongguben)?;
            dict.set_item("fenshu", d.fenshu)?;
            dict.set_item("xingquanjia", d.xingquanjia)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    // ============================================================
    // Tuple 高性能输出
    // ============================================================

    /// 获取K线数据, 返回 list of tuple (高性能模式)
    /// tuple: (open, close, high, low, vol, amount, year, month, day, hour, minute, datetime)
    #[pyo3(signature = (category, market, code, start=0, count=800, fq=1))]
    fn get_security_bars_tuples(
        &self,
        py: Python<'_>,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
        fq: u8,
    ) -> PyResult<Py<PyAny>> {
        let bars = self
            .rt
            .block_on(self.client.get_security_bars(category, market, code, start, count, fq))
            .map_err(to_py_err)?;

        let list = PyList::empty(py);
        for b in &bars {
            let items: Vec<Py<PyAny>> = vec![
                b.open.into_py_any(py)?,
                b.close.into_py_any(py)?,
                b.high.into_py_any(py)?,
                b.low.into_py_any(py)?,
                b.vol.into_py_any(py)?,
                b.amount.into_py_any(py)?,
                b.year.into_py_any(py)?,
                b.month.into_py_any(py)?,
                b.day.into_py_any(py)?,
                b.hour.into_py_any(py)?,
                b.minute.into_py_any(py)?,
                b.datetime.as_str().into_py_any(py)?,
            ];
            let tuple = PyTuple::new(py, &items)?;
            list.append(tuple)?;
        }
        Ok(list.into())
    }

    /// 获取指数K线, 返回 list of tuple (高性能模式)
    #[pyo3(signature = (category, market, code, start=0, count=800, fq=1))]
    fn get_index_bars_tuples(
        &self,
        py: Python<'_>,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
        fq: u8,
    ) -> PyResult<Py<PyAny>> {
        let bars = self
            .rt
            .block_on(self.client.get_index_bars(category, market, code, start, count, fq))
            .map_err(to_py_err)?;

        let list = PyList::empty(py);
        for b in &bars {
            let items: Vec<Py<PyAny>> = vec![
                b.open.into_py_any(py)?,
                b.close.into_py_any(py)?,
                b.high.into_py_any(py)?,
                b.low.into_py_any(py)?,
                b.vol.into_py_any(py)?,
                b.amount.into_py_any(py)?,
                b.year.into_py_any(py)?,
                b.month.into_py_any(py)?,
                b.day.into_py_any(py)?,
                b.hour.into_py_any(py)?,
                b.minute.into_py_any(py)?,
                b.datetime.as_str().into_py_any(py)?,
                b.up_count.into_py_any(py)?,
                b.down_count.into_py_any(py)?,
            ];
            let tuple = PyTuple::new(py, &items)?;
            list.append(tuple)?;
        }
        Ok(list.into())
    }

    /// 获取实时行情, 返回 list of tuple (高性能模式)
    fn get_security_quotes_tuples(
        &self,
        py: Python<'_>,
        all_stock: Vec<(u8, String)>,
    ) -> PyResult<Py<PyAny>> {
        let refs: Vec<(u8, &str)> = all_stock.iter().map(|(m, c)| (*m, c.as_str())).collect();
        let quotes = self
            .rt
            .block_on(self.client.get_security_quotes(&refs))
            .map_err(to_py_err)?;

        let list = PyList::empty(py);
        for q in &quotes {
            let items: Vec<Py<PyAny>> = vec![
                q.market.into_py_any(py)?,
                q.code.as_str().into_py_any(py)?,
                q.price.into_py_any(py)?,
                q.last_close.into_py_any(py)?,
                q.open.into_py_any(py)?,
                q.high.into_py_any(py)?,
                q.low.into_py_any(py)?,
                q.vol.into_py_any(py)?,
                q.cur_vol.into_py_any(py)?,
                q.amount.into_py_any(py)?,
                q.s_vol.into_py_any(py)?,
                q.b_vol.into_py_any(py)?,
                q.bid1.into_py_any(py)?,
                q.bid_vol1.into_py_any(py)?,
                q.ask1.into_py_any(py)?,
                q.ask_vol1.into_py_any(py)?,
                q.bid2.into_py_any(py)?,
                q.bid_vol2.into_py_any(py)?,
                q.ask2.into_py_any(py)?,
                q.ask_vol2.into_py_any(py)?,
                q.bid3.into_py_any(py)?,
                q.bid_vol3.into_py_any(py)?,
                q.ask3.into_py_any(py)?,
                q.ask_vol3.into_py_any(py)?,
                q.bid4.into_py_any(py)?,
                q.bid_vol4.into_py_any(py)?,
                q.ask4.into_py_any(py)?,
                q.ask_vol4.into_py_any(py)?,
                q.bid5.into_py_any(py)?,
                q.bid_vol5.into_py_any(py)?,
                q.ask5.into_py_any(py)?,
                q.ask_vol5.into_py_any(py)?,
            ];
            let tuple = PyTuple::new(py, &items)?;
            list.append(tuple)?;
        }
        Ok(list.into())
    }

    // ============================================================
    // DataFrame 输出
    // ============================================================

    /// 获取K线数据, 返回 pandas DataFrame
    #[pyo3(signature = (category, market, code, start=0, count=800, fq=1))]
    fn get_security_bars_dataframe(
        &self,
        py: Python<'_>,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
        fq: u8,
    ) -> PyResult<Py<PyAny>> {
        let bars = self
            .rt
            .block_on(self.client.get_security_bars(category, market, code, start, count, fq))
            .map_err(to_py_err)?;
        crate::python::py_dataframe::security_bars_to_df(py, &bars)
    }

    /// 获取指数K线, 返回 pandas DataFrame
    #[pyo3(signature = (category, market, code, start=0, count=800, fq=1))]
    fn get_index_bars_dataframe(
        &self,
        py: Python<'_>,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
        fq: u8,
    ) -> PyResult<Py<PyAny>> {
        let bars = self
            .rt
            .block_on(self.client.get_index_bars(category, market, code, start, count, fq))
            .map_err(to_py_err)?;
        crate::python::py_dataframe::index_bars_to_df(py, &bars)
    }

    /// 获取实时行情, 返回 pandas DataFrame
    fn get_security_quotes_dataframe(
        &self,
        py: Python<'_>,
        all_stock: Vec<(u8, String)>,
    ) -> PyResult<Py<PyAny>> {
        let refs: Vec<(u8, &str)> = all_stock.iter().map(|(m, c)| (*m, c.as_str())).collect();
        let quotes = self
            .rt
            .block_on(self.client.get_security_quotes(&refs))
            .map_err(to_py_err)?;
        crate::python::py_dataframe::quotes_to_df(py, &quotes)
    }

    /// 获取多只股票的财务信息, 返回 pandas DataFrame
    fn get_finance_info_dataframe(
        &self,
        py: Python<'_>,
        stocks: Vec<(u8, String)>,
    ) -> PyResult<Py<PyAny>> {
        let mut infos = Vec::new();
        for (market, code) in &stocks {
            let info = self
                .rt
                .block_on(self.client.get_finance_info(*market, code))
                .map_err(to_py_err)?;
            infos.push((info,));
        }
        crate::python::py_dataframe::finance_to_df(py, &infos)
    }
}
