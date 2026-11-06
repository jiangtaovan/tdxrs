//! Python 绑定 — TdxSmartClient
//!
//! 提供与 TdxHqClient 相同的 API，但采用分层健康检查和服务器缓存策略。

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use pyo3::IntoPyObjectExt;

use crate::net::smart_client::TdxSmartClient;
use crate::net::utils::HandshakeMode;

/// 智能连接客户端
///
/// 与 TdxHqClient 相同的 API，但采用不同的连接策略:
/// - 快速初始连接: 仅验证 TCP + 握手，不做 K 线健康检查
/// - 惰性健康检查: 首次 K 线请求返回空时触发，自动切换服务器
/// - 本地缓存: 记录成功/失败服务器，下次连接优先使用缓存
/// - 黑名单机制: 连续失败的服务器自动加入黑名单 (24h 过期)
#[pyclass(name = "TdxSmartClient")]
pub struct PyTdxSmartClient {
    client: TdxSmartClient,
}

#[pymethods]
impl PyTdxSmartClient {
    /// 创建新的智能客户端
    #[new]
    fn new() -> Self {
        Self {
            client: TdxSmartClient::new(),
        }
    }

    /// 连接到任意可用服务器 (快速模式)
    ///
    /// 仅验证 TCP + 握手，不做 K 线健康检查。
    /// 优先使用缓存的成功服务器。
    #[pyo3(signature = (timeout=None))]
    fn connect_to_any(&self, timeout: Option<f64>) -> PyResult<bool> {
        self.client
            .connect_to_any(timeout)
            .map_err(|e| pyo3::exceptions::PyConnectionError::new_err(e.to_string()))
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

    /// 获取 K 线数据 (带自动重试)
    ///
    /// 如果返回空数据，自动触发健康检查并尝试切换服务器。
    #[pyo3(signature = (category, market, code, start=0, count=800, fq=1))]
    fn get_security_bars(
        &self,
        py: Python,
        category: u8,
        market: u8,
        code: &str,
        start: u32,
        count: u16,
        fq: u8,
    ) -> PyResult<Py<PyAny>> {
        let bars = self
            .client
            .get_security_bars(category, market, code, start, count, fq)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        let result = PyList::empty(py);
        for bar in bars {
            let dict = PyDict::new(py);
            dict.set_item("open", bar.open)?;
            dict.set_item("high", bar.high)?;
            dict.set_item("low", bar.low)?;
            dict.set_item("close", bar.close)?;
            dict.set_item("vol", bar.vol)?;
            dict.set_item("amount", bar.amount)?;
            dict.set_item("year", bar.year)?;
            dict.set_item("month", bar.month)?;
            dict.set_item("day", bar.day)?;
            dict.set_item("hour", bar.hour)?;
            dict.set_item("minute", bar.minute)?;
            dict.set_item("datetime", &bar.datetime)?;
            result.append(dict)?;
        }

        Ok(result.into_py_any(py)?)
    }

    /// 获取实时行情 (带自动重试)
    fn get_security_quotes(
        &self,
        py: Python,
        all_stock: Vec<(u8, String)>,
    ) -> PyResult<Py<PyAny>> {
        let refs: Vec<(u8, &str)> = all_stock
            .iter()
            .map(|(m, c)| (*m, c.as_str()))
            .collect();

        let quotes = self
            .client
            .get_security_quotes(&refs)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        let result = PyList::empty(py);
        for q in quotes {
            let dict = PyDict::new(py);
            dict.set_item("market", q.market)?;
            dict.set_item("code", &q.code)?;
            dict.set_item("active1", q.active1)?;
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
            dict.set_item("ask1", q.ask1)?;
            dict.set_item("bid_vol1", q.bid_vol1)?;
            dict.set_item("ask_vol1", q.ask_vol1)?;
            dict.set_item("bid2", q.bid2)?;
            dict.set_item("ask2", q.ask2)?;
            dict.set_item("bid_vol2", q.bid_vol2)?;
            dict.set_item("ask_vol2", q.ask_vol2)?;
            dict.set_item("bid3", q.bid3)?;
            dict.set_item("ask3", q.ask3)?;
            dict.set_item("bid_vol3", q.bid_vol3)?;
            dict.set_item("ask_vol3", q.ask_vol3)?;
            dict.set_item("bid4", q.bid4)?;
            dict.set_item("ask4", q.ask4)?;
            dict.set_item("bid_vol4", q.bid_vol4)?;
            dict.set_item("ask_vol4", q.ask_vol4)?;
            dict.set_item("bid5", q.bid5)?;
            dict.set_item("ask5", q.ask5)?;
            dict.set_item("bid_vol5", q.bid_vol5)?;
            dict.set_item("ask_vol5", q.ask_vol5)?;
            dict.set_item("reversed_bytes0", &q.reversed_bytes0)?;
            dict.set_item("reversed_bytes1", &q.reversed_bytes1)?;
            dict.set_item("reversed_bytes2", &q.reversed_bytes2)?;
            dict.set_item("active2", q.active2)?;
            result.append(dict)?;
        }

        Ok(result.into_py_any(py)?)
    }

    /// 获取缓存统计信息
    fn cache_stats(&self) -> String {
        self.client.cache_stats()
    }

    /// 清除缓存
    fn clear_cache(&self) {
        self.client.clear_cache();
    }

    /// 探测所有服务器并更新缓存
    ///
    /// 类似 mootdx 的 bestip 功能。
    fn probe_and_cache(&self, timeout_secs: f64) -> Vec<(String, u16, String, u32)> {
        self.client.probe_and_cache(timeout_secs)
    }

    /// 断开连接
    fn disconnect(&self) {
        self.client.inner().disconnect();
    }

    /// 是否已连接
    fn is_connected(&self) -> bool {
        self.client.inner().is_connected()
    }
}
