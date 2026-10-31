//! TdxDirectClient Python 绑定
//!
//! 裸连接客户端: 无连接池、无重试、无心跳, 每请求新建 TCP 连接

use std::sync::Mutex;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::error::TdxError;
use crate::net::direct_client::TdxDirectClient;
use crate::net::utils::HandshakeMode;
use crate::protocol::constants::DEFAULT_PORT;

/// 将 TdxError 转换为 Python 异常 (带错误码)
fn to_py_err(e: TdxError) -> PyErr {
    match &e {
        TdxError::Coded(coded) => {
            // 带错误码的错误，使用 ValueError 并包含错误码
            pyo3::exceptions::PyValueError::new_err(coded.format())
        }
        TdxError::Connection(_) | TdxError::ConnectionTimeout | TdxError::Disconnected => {
            pyo3::exceptions::PyConnectionError::new_err(e.to_string())
        }
        _ => pyo3::exceptions::PyValueError::new_err(e.to_string()),
    }
}

/// TDX 裸连接客户端 — Python 绑定
#[pyclass(name = "TdxDirectClient")]
pub struct PyTdxDirectClient {
    client: Mutex<TdxDirectClient>,
}

#[pymethods]
impl PyTdxDirectClient {
    /// 创建裸连接客户端
    ///
    /// ip: 服务器地址, port: 端口(默认7709), timeout: 超时秒数
    #[new]
    #[pyo3(signature = (ip, port=DEFAULT_PORT, timeout=5.0))]
    fn new(ip: &str, port: u16, timeout: f64) -> Self {
        Self {
            client: Mutex::new(TdxDirectClient::new(ip, port, timeout)),
        }
    }

    /// 更新服务器地址
    fn set_server(&self, ip: &str, port: u16) {
        self.client.lock().unwrap().set_server(ip, port);
    }

    /// 更新超时
    fn set_timeout(&self, timeout: f64) {
        self.client.lock().unwrap().set_timeout(timeout);
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
        self.client.lock().unwrap().set_handshake_mode(m);
        Ok(())
    }

    /// 获取当前握手模式
    fn handshake_mode(&self) -> &'static str {
        match self.client.lock().unwrap().handshake_mode() {
            HandshakeMode::Full => "full",
            HandshakeMode::SkipCmd3 => "skip_cmd3",
            HandshakeMode::Cmd1Only => "cmd1_only",
            HandshakeMode::None => "none",
        }
    }

    // ============================================================
    // K线
    // ============================================================

    #[pyo3(signature = (category, market, code, start=0, count=800, fq=1))]
    fn get_security_bars(
        &self, py: Python<'_>, category: u8, market: u8, code: &str,
        start: u32, count: u16, fq: u8,
    ) -> PyResult<Py<PyAny>> {
        let bars = self.client.lock().unwrap()
            .get_security_bars(category, market, code, start, count, fq)
            .map_err(to_py_err)?;
        bars_to_list(py, &bars, |b, d| {
            d.set_item("open", b.open)?;
            d.set_item("close", b.close)?;
            d.set_item("high", b.high)?;
            d.set_item("low", b.low)?;
            d.set_item("vol", b.vol)?;
            d.set_item("amount", b.amount)?;
            d.set_item("year", b.year)?;
            d.set_item("month", b.month)?;
            d.set_item("day", b.day)?;
            d.set_item("hour", b.hour)?;
            d.set_item("minute", b.minute)?;
            d.set_item("datetime", &b.datetime)?;
            Ok(())
        })
    }

    #[pyo3(signature = (category, market, code, start=0, count=800, fq=1))]
    fn get_index_bars(
        &self, py: Python<'_>, category: u8, market: u8, code: &str,
        start: u32, count: u16, fq: u8,
    ) -> PyResult<Py<PyAny>> {
        let bars = self.client.lock().unwrap()
            .get_index_bars(category, market, code, start, count, fq)
            .map_err(to_py_err)?;
        bars_to_list(py, &bars, |b, d| {
            d.set_item("open", b.open)?;
            d.set_item("close", b.close)?;
            d.set_item("high", b.high)?;
            d.set_item("low", b.low)?;
            d.set_item("vol", b.vol)?;
            d.set_item("amount", b.amount)?;
            d.set_item("year", b.year)?;
            d.set_item("month", b.month)?;
            d.set_item("day", b.day)?;
            d.set_item("hour", b.hour)?;
            d.set_item("minute", b.minute)?;
            d.set_item("datetime", &b.datetime)?;
            d.set_item("up_count", b.up_count)?;
            d.set_item("down_count", b.down_count)?;
            Ok(())
        })
    }

    // ============================================================
    // 实时行情
    // ============================================================

    fn get_security_quotes(
        &self, py: Python<'_>, all_stock: Vec<(u8, String)>,
    ) -> PyResult<Py<PyAny>> {
        let refs: Vec<(u8, &str)> = all_stock.iter().map(|(m, c)| (*m, c.as_str())).collect();
        let quotes = self.client.lock().unwrap().get_security_quotes(&refs)
            .map_err(to_py_err)?;
        quotes_to_list(py, &quotes)
    }

    // ============================================================
    // 证券信息
    // ============================================================

    #[pyo3(signature = (market, start=0))]
    fn get_security_list(&self, py: Python<'_>, market: u8, start: u16) -> PyResult<Py<PyAny>> {
        let list = self.client.lock().unwrap().get_security_list(market, start)
            .map_err(to_py_err)?;
        sec_list_to_list(py, &list)
    }

    fn get_security_count(&self, market: u8) -> PyResult<u16> {
        self.client.lock().unwrap().get_security_count(market)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    // ============================================================
    // 分时 + 逐笔 + 财务 + 除权 + 板块
    // ============================================================

    fn get_minute_time_data(&self, py: Python<'_>, market: u8, code: &str) -> PyResult<Py<PyAny>> {
        let data = self.client.lock().unwrap().get_minute_time_data(market, code)
            .map_err(to_py_err)?;
        minute_to_list(py, &data)
    }

    fn get_history_minute_time_data(
        &self, py: Python<'_>, market: u8, code: &str, date: u32,
    ) -> PyResult<Py<PyAny>> {
        let data = self.client.lock().unwrap().get_history_minute_time_data(market, code, date)
            .map_err(to_py_err)?;
        minute_to_list(py, &data)
    }

    #[pyo3(signature = (market, code, start=0, count=2000))]
    fn get_transaction_data(
        &self, py: Python<'_>, market: u8, code: &str, start: u16, count: u16,
    ) -> PyResult<Py<PyAny>> {
        let data = self.client.lock().unwrap().get_transaction_data(market, code, start, count)
            .map_err(to_py_err)?;
        tick_to_list(py, &data)
    }

    #[pyo3(signature = (market, code, start=0, count=2000, date=0))]
    fn get_history_transaction_data(
        &self, py: Python<'_>, market: u8, code: &str, start: u16, count: u16, date: u32,
    ) -> PyResult<Py<PyAny>> {
        let data = self.client.lock().unwrap()
            .get_history_transaction_data(market, code, start, count, date)
            .map_err(to_py_err)?;
        tick_to_list(py, &data)
    }

    fn get_finance_info(&self, py: Python<'_>, market: u8, code: &str) -> PyResult<Py<PyAny>> {
        let info = self.client.lock().unwrap().get_finance_info(market, code)
            .map_err(to_py_err)?;
        finance_to_dict(py, &info)
    }

    fn get_xdxr_info(&self, py: Python<'_>, market: u8, code: &str) -> PyResult<Py<PyAny>> {
        let data = self.client.lock().unwrap().get_xdxr_info(market, code)
            .map_err(to_py_err)?;
        xdxr_to_list(py, &data)
    }

    fn get_and_parse_block_info(&self, py: Python<'_>, block_file: &str) -> PyResult<Py<PyAny>> {
        let data = self.client.lock().unwrap().get_and_parse_block_info(block_file)
            .map_err(to_py_err)?;
        block_to_list(py, &data)
    }
}

// ============================================================
// 序列化辅助 (与 py_client.rs 共享模式)
// ============================================================

use crate::protocol::types::*;
use crate::reader::block::BlockRecord;

fn bars_to_list<B, F>(py: Python<'_>, bars: &[B], fill: F) -> PyResult<Py<PyAny>>
where F: Fn(&B, &Bound<'_, PyDict>) -> PyResult<()>
{
    let list = PyList::empty(py);
    for b in bars {
        let dict = PyDict::new(py);
        fill(b, &dict)?;
        list.append(dict)?;
    }
    Ok(list.into())
}

fn quotes_to_list(py: Python<'_>, quotes: &[SecurityQuote]) -> PyResult<Py<PyAny>> {
    let list = PyList::empty(py);
    for q in quotes {
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

fn sec_list_to_list(py: Python<'_>, data: &[SecurityInfo]) -> PyResult<Py<PyAny>> {
    let list = PyList::empty(py);
    for s in data {
        let dict = PyDict::new(py);
        dict.set_item("code", &s.code)?;
        dict.set_item("volunit", s.volunit)?;
        dict.set_item("decimal_point", s.decimal_point)?;
        dict.set_item("name", &s.name)?;
        dict.set_item("pre_close", s.pre_close)?;
        list.append(dict)?;
    }
    Ok(list.into())
}

fn minute_to_list(py: Python<'_>, data: &[MinuteTimePrice]) -> PyResult<Py<PyAny>> {
    let list = PyList::empty(py);
    for d in data {
        let dict = PyDict::new(py);
        dict.set_item("time", &d.time)?;
        dict.set_item("price", d.price)?;
        dict.set_item("avg_price", d.avg_price)?;
        dict.set_item("vol", d.vol)?;
        list.append(dict)?;
    }
    Ok(list.into())
}

fn tick_to_list(py: Python<'_>, data: &[TickData]) -> PyResult<Py<PyAny>> {
    let list = PyList::empty(py);
    for d in data {
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

fn finance_to_dict(py: Python<'_>, info: &FinanceInfo) -> PyResult<Py<PyAny>> {
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

fn xdxr_to_list(py: Python<'_>, data: &[XdXrInfo]) -> PyResult<Py<PyAny>> {
    let list = PyList::empty(py);
    for d in data {
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

fn block_to_list(py: Python<'_>, data: &[BlockRecord]) -> PyResult<Py<PyAny>> {
    let list = PyList::empty(py);
    for d in data {
        let dict = PyDict::new(py);
        dict.set_item("blockname", &d.blockname)?;
        dict.set_item("block_type", d.block_type)?;
        dict.set_item("code_index", d.code_index)?;
        dict.set_item("code", &d.code)?;
        list.append(dict)?;
    }
    Ok(list.into())
}
