use serde::Serialize;

// ============================================================
// K线数据
// ============================================================

#[derive(Debug, Clone, Serialize)]
pub struct SecurityBar {
    pub open: f64,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    pub vol: f64,
    pub amount: f64,
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub datetime: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexBar {
    pub open: f64,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    pub vol: f64,
    pub amount: f64,
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub datetime: String,
    pub up_count: u32,
    pub down_count: u32,
}

// ============================================================
// 实时行情
// ============================================================

#[derive(Debug, Clone, Serialize)]
pub struct SecurityQuote {
    pub market: u8,
    pub code: String,
    pub active1: u16,
    pub price: f64,
    pub last_close: f64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub servertime: String,
    pub vol: f64,
    pub cur_vol: f64,
    pub amount: f64,
    pub s_vol: f64,
    pub b_vol: f64,
    pub bid1: f64,
    pub bid_vol1: f64,
    pub bid2: f64,
    pub bid_vol2: f64,
    pub bid3: f64,
    pub bid_vol3: f64,
    pub bid4: f64,
    pub bid_vol4: f64,
    pub bid5: f64,
    pub bid_vol5: f64,
    pub ask1: f64,
    pub ask_vol1: f64,
    pub ask2: f64,
    pub ask_vol2: f64,
    pub ask3: f64,
    pub ask_vol3: f64,
    pub ask4: f64,
    pub ask_vol4: f64,
    pub ask5: f64,
    pub ask_vol5: f64,
    pub reversed_bytes0: u32,
    pub reversed_bytes1: u32,
    pub reversed_bytes2: u32,
    pub reversed_bytes3: u32,
    pub reversed_bytes4: u32,
    pub reversed_bytes5: u32,
    pub reversed_bytes6: u32,
    pub reversed_bytes7: u32,
    pub reversed_bytes8: u32,
    pub reversed_bytes9: u32,
    pub active2: u16,
}

// ============================================================
// 证券列表
// ============================================================

#[derive(Debug, Clone, Serialize)]
pub struct SecurityInfo {
    pub code: String,
    pub volunit: u16,
    pub decimal_point: u8,
    pub name: String,
    pub pre_close: f64,
}

// ============================================================
// 分时数据
// ============================================================

#[derive(Debug, Clone, Serialize)]
pub struct MinuteTimePrice {
    pub time: String,
    pub price: f64,
    pub avg_price: f64,
    pub vol: f64,
}

// ============================================================
// 集合竞价 (0x056a)
// ============================================================

/// 集合竞价的一次撮合快照（09:15~09:25 之间每 3 秒一条）。
///
/// 通达信把「已匹配量 + 未匹配量 + 未匹配方向」放在一条记录里，
/// 未匹配量为正表示买方排队，为负表示卖方排队。
#[derive(Debug, Clone, Serialize)]
pub struct CallAuctionTick {
    /// 撮合时刻 HH:MM:SS
    pub time: String,
    /// 参考价/拟成交价（元）
    pub price: f64,
    /// 已匹配量（股）
    pub match_vol: u64,
    /// 未匹配量（股，已取绝对值，方向见 side）
    pub unmatched_vol: u32,
    /// 1 = 买方未匹配（买盘排队），-1 = 卖方未匹配
    pub side: i8,
}

// ============================================================
// 逐笔成交
// ============================================================

#[derive(Debug, Clone, Serialize)]
pub struct TickData {
    pub time: String,
    pub price: f64,
    pub vol: f64,
    pub num: u32,
    pub buyorsell: u32,
    /// 保留字段 (原 extra field，具体含义待确认)
    pub reserved: u32,
}

// ============================================================
// 财务信息
// ============================================================

#[derive(Debug, Clone, Serialize)]
pub struct FinanceInfo {
    pub market: u8,
    pub code: String,
    pub liutongguben: f64,
    pub province: u16,
    pub industry: u16,
    pub updated_date: u32,
    pub ipo_date: u32,
    pub zongguben: f64,
    pub guojiagu: f64,
    pub faqirenfarengu: f64,
    pub farengu: f64,
    pub bgu: f64,
    pub hgu: f64,
    pub zhigonggu: f64,
    pub zongzichan: f64,
    pub liudongzichan: f64,
    pub gudingzichan: f64,
    pub wuxingzichan: f64,
    pub gudongrenshu: f64,
    pub liudongfuzhai: f64,
    pub changqifuzhai: f64,
    pub zibengongjijin: f64,
    pub jingzichan: f64,
    pub zhuyingshouru: f64,
    pub zhuyinglirun: f64,
    pub yingshouzhangkuan: f64,
    pub yingyelirun: f64,
    pub touzishouyu: f64,
    pub jingyingxianjinliu: f64,
    pub zongxianjinliu: f64,
    pub cunhuo: f64,
    pub lirunzonghe: f64,
    pub shuihoulirun: f64,
    pub jinglirun: f64,
    pub weifenpeilirun: f64,
    pub meigujingzichan: f64,
}

// ============================================================
// 除权除息
// ============================================================

#[derive(Debug, Clone, Serialize)]
pub struct XdXrInfo {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub category: u32,
    pub name: String,
    pub fenhong: Option<f64>,
    pub peigujia: Option<f64>,
    pub songzhuangu: Option<f64>,
    pub peigu: Option<f64>,
    pub suogu: Option<f64>,
    pub panqianliutong: Option<f64>,
    pub panhouliutong: Option<f64>,
    pub qianzongguben: Option<f64>,
    pub houzongguben: Option<f64>,
    pub fenshu: Option<f64>,
    pub xingquanjia: Option<f64>,
}

// ============================================================
// 板块元数据
// ============================================================

#[derive(Debug, Clone, Serialize)]
pub struct BlockInfoMeta {
    pub size: u32,
    pub hash_value: String,
}

// ============================================================
// 证券类型和系数
// ============================================================

/// 获取证券类型
///
/// 返回值:
/// - 0: 指数
/// - 1: A股
/// - 2: B股
/// - 3: 场内基金 (ETF/LOF/REITs)
/// - 4: 债券
/// - 5: 场外基金 (传统开放式基金)
///
/// 沪市代码分类规则:
/// - 60/68: A股
/// - 90: B股
/// - 519: 场外基金 (必须在 50/51 之前检查)
/// - 50/51/58: 场内基金
/// - 11/13: 债券
/// - 000: 指数
pub fn get_security_type(market: u8, code: &str) -> u8 {
    if market == 1 {
        // 上海
        if code.starts_with("60") || code.starts_with("68") {
            return 1; // A股
        }
        if code.starts_with("90") {
            return 2; // B股
        }
        // 场外基金: 传统开放式基金 (519xxx) - 必须在场内基金之前检查
        if code.starts_with("519") {
            return 5; // 场外基金
        }
        // 场内基金: ETF/LOF/REITs (50xxx, 51xxx, 58xxx)
        if code.starts_with("50") || code.starts_with("51") || code.starts_with("58") {
            return 3; // 基金
        }
        if code.starts_with("11") || code.starts_with("13") {
            return 4; // 债券
        }
        // 上证指数: 000001, 000300 等
        if code.starts_with("000") {
            return 0; // 指数
        }
    } else if market == 0 {
        // 深圳
        if code.starts_with("39") {
            return 0; // 指数
        }
        if code.starts_with("00") || code.starts_with("30") {
            return 1; // A股
        }
        if code.starts_with("20") {
            return 2; // B股
        }
        if code.starts_with("15") || code.starts_with("16") {
            return 3; // 基金
        }
        if code.starts_with("10") || code.starts_with("12") || code.starts_with("13") {
            return 4; // 债券
        }
    }
    1 // 默认 A股
}

/// 获取价格系数
///
/// 不同证券类型使用不同的价格系数:
/// - 指数/A股: 0.01 (2位小数)
/// - B股: 0.001 (3位小数)
/// - 场内基金 (ETF/LOF/REITs): 0.001 (3位小数)
/// - 债券: 0.0001 (4位小数)
/// - 场外基金 (传统开放式基金): 0.00001 (5位小数)
///
/// 注意: 场外基金 (519xxx) 返回的是单位净值，不是累积净值
pub fn get_security_coefficient(market: u8, code: &str) -> f64 {
    let sec_type = get_security_type(market, code);
    match sec_type {
        0 => 0.01,    // 指数
        1 => 0.01,    // A股
        2 => 0.001,   // B股
        3 => 0.001,   // 场内基金 (ETF/LOF/REITs)
        4 => 0.0001,  // 债券
        5 => 0.00001, // 场外基金 (传统开放式基金)
        _ => 0.01,
    }
}
