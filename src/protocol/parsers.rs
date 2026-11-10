use encoding_rs::GBK;

use crate::constants::{read_u16, read_u32, max_valid_year};
use crate::error::Result;
use crate::error_codes::ErrorCode;
use crate::helpers::{get_price, get_volume};

use super::types::*;

// ============================================================
// 集合竞价 (0x056a)
// ============================================================

/// 解析集合竞价逐笔。
///
/// body 布局：`count(u16)` + count × 16 字节
/// `分钟数(u16) | 价格(f32) | 已匹配量(u32) | 未匹配量(i32) | 秒(u8) | 保留(u8)`
///
/// 未匹配量是有符号的：正=买方排队，负=卖方排队。早期实现只读低 16 位，
/// 未匹配量超过 32767 手时会被截断甚至反向，这里按完整 4 字节读。
pub fn parse_call_auction(body: &[u8]) -> Result<Vec<CallAuctionTick>> {
    if body.len() < 2 {
        return Err(ErrorCode::RESPONSE_LENGTH_MISMATCH.err("body too short for call auction"));
    }

    let count = read_u16(body, 0) as usize;
    let mut pos = 2;
    let mut result = Vec::with_capacity(count);

    for _ in 0..count {
        if pos + 16 > body.len() {
            break;
        }
        let minutes = read_u16(body, pos) as u32;
        let price = f32::from_le_bytes([body[pos + 2], body[pos + 3], body[pos + 4], body[pos + 5]]) as f64;
        let match_vol = read_u32(body, pos + 6) as u64;
        let unmatched_raw = i32::from_le_bytes([
            body[pos + 10],
            body[pos + 11],
            body[pos + 12],
            body[pos + 13],
        ]);
        let second = body[pos + 14];

        result.push(CallAuctionTick {
            time: format!("{:02}:{:02}:{:02}", minutes / 60, minutes % 60, second),
            price: (price * 1000.0).round() / 1000.0,
            match_vol,
            unmatched_vol: unmatched_raw.unsigned_abs(),
            side: if unmatched_raw < 0 { -1 } else { 1 },
        });
        pos += 16;
    }

    Ok(result)
}

// ============================================================
// 解析证券数量
// ============================================================

pub fn parse_security_count(body: &[u8]) -> Result<u16> {
    if body.len() < 2 {
        return Err(ErrorCode::RESPONSE_LENGTH_MISMATCH.err("body too short for count"));
    }
    Ok(read_u16(body, 0))
}

// ============================================================
// 解析证券列表
// ============================================================

pub fn parse_security_list(body: &[u8]) -> Result<Vec<SecurityInfo>> {
    if body.len() < 2 {
        return Err(ErrorCode::RESPONSE_LENGTH_MISMATCH.err("body too short"));
    }

    let count = read_u16(body, 0) as usize;
    let mut pos = 2;
    let record_size = 29; // <6sH8s4sBI4s>

    let mut result = Vec::with_capacity(count);

    for _ in 0..count {
        if pos + record_size > body.len() {
            break;
        }

        // 6 bytes code
        let code_bytes = &body[pos..pos + 6];
        let code = String::from_utf8_lossy(code_bytes)
            .trim_end_matches('\0')
            .to_string();
        pos += 6;

        // u16 volunit
        let volunit = read_u16(body, pos);
        pos += 2;

        // 8 bytes name (GBK)
        let name_bytes = &body[pos..pos + 8];
        let (name, _, _) = GBK.decode(name_bytes);
        let name = name.trim_end_matches('\0').to_string();
        pos += 8;

        // 4 bytes reversed
        pos += 4;

        // u8 decimal_point
        let decimal_point = body[pos];
        pos += 1;

        // u32 pre_close_raw (decoded via get_volume)
        let pre_close_raw = read_u32(body, pos) as i64;
        let pre_close = get_volume(pre_close_raw);
        pos += 4;

        // 4 bytes reversed
        pos += 4;

        result.push(SecurityInfo {
            code,
            volunit,
            decimal_point,
            name,
            pre_close,
        });
    }

    Ok(result)
}

// ============================================================
// 解析K线数据 (个股)
// ============================================================

pub fn parse_security_bars(body: &[u8], category: u8) -> Result<Vec<SecurityBar>> {
    if body.len() < 2 {
        return Err(ErrorCode::RESPONSE_LENGTH_MISMATCH.err("body too short"));
    }

    let count = read_u16(body, 0) as usize;
    let mut pos = 2;
    let mut result = Vec::with_capacity(count);
    let mut pre_diff_base: i64 = 0;

    for _ in 0..count {
        // Bounds check: datetime(4) + 4*price(var) + vol(4) + amount(4) = min 16
        if pos + 16 > body.len() {
            break;
        }

        let mut bar = SecurityBar {
            open: 0.0,
            close: 0.0,
            high: 0.0,
            low: 0.0,
            vol: 0.0,
            amount: 0.0,
            year: 0,
            month: 0,
            day: 0,
            hour: 0,
            minute: 0,
            datetime: String::new(),
        };

        // 日期时间
        let (year, month, day, hour, minute, new_pos) = get_datetime(category, body, pos);
        // 校验日期合法性 — 服务器可能返回损坏数据或无效代码的垃圾数据
        // 无效日期时截断返回已有结果，而非报错
        if year < 1980 || year > max_valid_year() || month < 1 || month > 12 || day < 1 || day > 31 {
            break;
        }
        bar.year = year;
        bar.month = month;
        bar.day = day;
        bar.hour = hour;
        bar.minute = minute;
        pos = new_pos;

        if category < 4 || category == 7 || category == 8 {
            bar.datetime = format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                year, month, day, hour, minute
            );
        } else {
            bar.datetime = format!("{:04}-{:02}-{:02}", year, month, day);
        }

        // 价格: 差分编码 (Python order: open, close, high, low)
        let (price_open_diff, new_pos) = get_price(body, pos);
        bar.open = ((pre_diff_base + price_open_diff) as f64) / 1000.0;
        pos = new_pos;
        let accumulated = pre_diff_base + price_open_diff;

        let (price_close_diff, new_pos) = get_price(body, pos);
        bar.close = ((accumulated + price_close_diff) as f64) / 1000.0;
        pos = new_pos;

        let (price_high_diff, new_pos) = get_price(body, pos);
        bar.high = ((accumulated + price_high_diff) as f64) / 1000.0;
        pos = new_pos;

        let (price_low_diff, new_pos) = get_price(body, pos);
        bar.low = ((accumulated + price_low_diff) as f64) / 1000.0;
        pos = new_pos;

        pre_diff_base = accumulated + price_close_diff;

        // vol (u32) - Python reads vol first
        let vol_raw = read_u32(body, pos) as i64;
        bar.vol = get_volume(vol_raw);
        pos += 4;

        // amount (u32) - Python reads amount (db_vol) second
        let amount_raw = read_u32(body, pos) as i64;
        bar.amount = get_volume(amount_raw);
        pos += 4;

        result.push(bar);
    }

    Ok(result)
}

// ============================================================
// 解析K线数据 (指数)
// ============================================================

pub fn parse_index_bars(body: &[u8], category: u8) -> Result<Vec<IndexBar>> {
    if body.len() < 2 {
        return Err(ErrorCode::RESPONSE_LENGTH_MISMATCH.err("body too short"));
    }

    let count = read_u16(body, 0) as usize;
    let mut pos = 2;
    let mut result = Vec::with_capacity(count);
    let mut pre_diff_base: i64 = 0;

    for _ in 0..count {
        // datetime(4) + 4*price(var) + vol(4) + amount(4) + up_count(2) + down_count(2) = min 24
        if pos + 24 > body.len() {
            break;
        }
        let mut bar = IndexBar {
            open: 0.0,
            close: 0.0,
            high: 0.0,
            low: 0.0,
            vol: 0.0,
            amount: 0.0,
            year: 0,
            month: 0,
            day: 0,
            hour: 0,
            minute: 0,
            datetime: String::new(),
            up_count: 0,
            down_count: 0,
        };

        // 日期时间
        let (year, month, day, hour, minute, new_pos) = get_datetime(category, body, pos);
        // 校验日期合法性 — 服务器可能返回损坏数据或无效代码的垃圾数据
        // 无效日期时截断返回已有结果，而非报错
        if year < 1980 || year > max_valid_year() || month < 1 || month > 12 || day < 1 || day > 31 {
            break;
        }
        bar.year = year;
        bar.month = month;
        bar.day = day;
        bar.hour = hour;
        bar.minute = minute;
        pos = new_pos;

        if category < 4 || category == 7 || category == 8 {
            bar.datetime = format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                year, month, day, hour, minute
            );
        } else {
            bar.datetime = format!("{:04}-{:02}-{:02}", year, month, day);
        }

        // 价格: 差分编码 (Python order: open, close, high, low)
        let (price_open_diff, new_pos) = get_price(body, pos);
        bar.open = ((pre_diff_base + price_open_diff) as f64) / 1000.0;
        pos = new_pos;

        let accumulated = pre_diff_base + price_open_diff;

        let (price_close_diff, new_pos) = get_price(body, pos);
        bar.close = ((accumulated + price_close_diff) as f64) / 1000.0;
        pos = new_pos;

        let (price_high_diff, new_pos) = get_price(body, pos);
        bar.high = ((accumulated + price_high_diff) as f64) / 1000.0;
        pos = new_pos;

        let (price_low_diff, new_pos) = get_price(body, pos);
        bar.low = ((accumulated + price_low_diff) as f64) / 1000.0;
        pos = new_pos;

        pre_diff_base = accumulated + price_close_diff;

        let vol_raw = read_u32(body, pos) as i64;
        bar.vol = get_volume(vol_raw);
        pos += 4;

        let amount_raw = read_u32(body, pos) as i64;
        bar.amount = get_volume(amount_raw);
        pos += 4;

        // up_count, down_count (u16 each)
        bar.up_count = read_u16(body, pos) as u32;
        pos += 2;
        bar.down_count = read_u16(body, pos) as u32;
        pos += 2;

        result.push(bar);
    }

    Ok(result)
}

// ============================================================
// 解析分时数据
// ============================================================

/// 根据分时数据索引计算时间字符串
///
/// TDX 分时数据每天 240 个点，开盘集合竞价视为无有效数据点:
/// - 上午 120 个: 09:31 ~ 11:30 (index 0-119)，不含 09:30
/// - 下午 120 个: 13:01 ~ 15:00 (index 120-239)，不含 13:00
pub fn minute_time_from_index(index: usize) -> String {
    let total = if index < 120 {
        9 * 60 + 31 + index           // 09:31 + index → 09:31 ~ 11:30
    } else {
        13 * 60 + 1 + (index - 120)   // 13:01 + (index-120) → 13:01 ~ 15:00
    };
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// 解析当日分时数据
///
/// ⚠️ 已知问题: TDX 实时分时 API (命令码 0x051d) 的数据格式与历史分时 API 不同，
/// 且数据编码存在异常（价格差分编码在某些记录会重置）。
///
/// 建议使用 `get_history_minute_time_data` API 替代，传入今日日期即可获取当日数据，
/// 该 API 数据格式稳定且已验证正确。
///
/// 当前实现基于逆向分析，头部偏移 13 字节，但部分场景下价格可能异常。
pub fn parse_minute_time_data(body: &[u8], market: u8, code: &str) -> Result<Vec<MinuteTimePrice>> {
    let coefficient = super::types::get_security_coefficient(market, code);

    if body.len() < 14 {
        return Err(ErrorCode::RESPONSE_LENGTH_MISMATCH.err("body too short"));
    }

    let count = read_u16(body, 0) as usize;
    // 实时分时数据头部: 2(count) + 2(padding) + 1(indicator) + 6(stock_code) + 2(unknown) = 13 bytes
    // 注意: 此偏移量基于逆向分析，可能不完全准确
    let mut pos = 13;
    let mut result = Vec::with_capacity(count);

    let mut pre_diff_base: i64 = 0;
    let mut cum_amount: f64 = 0.0;
    let mut cum_vol: f64 = 0.0;

    for i in 0..count {
        let (price_diff, new_pos) = get_price(body, pos);
        pre_diff_base += price_diff;
        let price = (pre_diff_base as f64) * coefficient;
        pos = new_pos;

        // reversed1 (skipped)
        let (_, new_pos) = get_price(body, pos);
        pos = new_pos;

        let (vol_diff, new_pos) = get_price(body, pos);
        let vol = vol_diff as f64;
        pos = new_pos;

        // 均价 = 累计金额 / 累计成交量
        cum_amount += price * vol;
        cum_vol += vol;
        let avg_price = if cum_vol > 0.0 { cum_amount / cum_vol } else { price };

        let time = minute_time_from_index(i);
        result.push(MinuteTimePrice { time, price, avg_price, vol });
    }

    // 倒序排列：最新记录在前
    result.reverse();
    Ok(result)
}

// ============================================================
// 解析历史分时数据
// ============================================================

pub fn parse_history_minute_time_data(
    body: &[u8],
    market: u8,
    code: &str,
) -> Result<Vec<MinuteTimePrice>> {
    let coefficient = super::types::get_security_coefficient(market, code);

    // 跳过 6 bytes header
    let mut pos = 6;

    let mut result = Vec::new();
    let mut pre_diff_base: i64 = 0;
    let mut cum_amount: f64 = 0.0;
    let mut cum_vol: f64 = 0.0;
    let mut index: usize = 0;

    // get_price 在越界时返回 (0, data.len())，不会 panic
    while pos < body.len() {
        let old_pos = pos;
        let (price_diff, new_pos) = get_price(body, pos);
        pre_diff_base += price_diff;
        let price = (pre_diff_base as f64) * coefficient;
        pos = new_pos;

        // reversed1 (skipped)
        let (_, new_pos) = get_price(body, pos);
        pos = new_pos;

        let (vol_diff, new_pos) = get_price(body, pos);
        let vol = vol_diff as f64;
        pos = new_pos;

        // 防止无限循环 (get_price 越界时 pos 不变)
        if pos == old_pos {
            break;
        }

        // 均价 = 累计金额 / 累计成交量
        cum_amount += price * vol;
        cum_vol += vol;
        let avg_price = if cum_vol > 0.0 { cum_amount / cum_vol } else { price };

        let time = minute_time_from_index(index);
        result.push(MinuteTimePrice { time, price, avg_price, vol });
        index += 1;
    }

    // 倒序排列：最新记录在前
    result.reverse();
    Ok(result)
}

// ============================================================
// 解析逐笔成交
// ============================================================

pub fn parse_transaction_data(body: &[u8]) -> Result<Vec<TickData>> {
    parse_transaction_data_with_coefficient(body, 0.01)
}

pub fn parse_transaction_data_with_coefficient(body: &[u8], coefficient: f64) -> Result<Vec<TickData>> {
    if body.len() < 2 {
        return Err(ErrorCode::RESPONSE_LENGTH_MISMATCH.err("body too short"));
    }

    let count = read_u16(body, 0) as usize;
    let mut pos = 2;
    let mut result = Vec::with_capacity(count);

    let mut last_price: i64 = 0;

    for _ in 0..count {
        // time (u16 minutes)
        let minutes = read_u16(body, pos) as u32;
        pos += 2;
        let hour = minutes / 60;
        let minute = minutes % 60;
        let time = format!("{:02}:{:02}", hour, minute);

        // price (delta encoded)
        let (price_diff, new_pos) = get_price(body, pos);
        last_price += price_diff;
        let price = last_price as f64 * coefficient;
        pos = new_pos;

        // vol
        let (vol, new_pos) = get_price(body, pos);
        let vol = vol as f64;
        pos = new_pos;

        // num
        let (num, new_pos) = get_price(body, pos);
        let num = num as u32;
        pos = new_pos;

        // buyorsell
        let (buyorsell, new_pos) = get_price(body, pos);
        let buyorsell = buyorsell as u32;
        pos = new_pos;

        // reserved (原 extra field，具体含义待确认)
        let (reserved, new_pos) = get_price(body, pos);
        let reserved = reserved as u32;
        pos = new_pos;

        result.push(TickData {
            time,
            price,
            vol,
            num,
            buyorsell,
            reserved,
        });
    }

    Ok(result)
}

// ============================================================
// 解析历史逐笔成交
// ============================================================

pub fn parse_history_transaction_data(body: &[u8]) -> Result<Vec<TickData>> {
    parse_history_transaction_data_with_coefficient(body, 0.01)
}

pub fn parse_history_transaction_data_with_coefficient(body: &[u8], coefficient: f64) -> Result<Vec<TickData>> {
    if body.len() < 6 {
        return Err(ErrorCode::RESPONSE_LENGTH_MISMATCH.err("body too short"));
    }

    // 跳过 2 bytes count + 4 bytes header
    let mut pos = 6;

    let mut result = Vec::new();
    let mut last_price: i64 = 0;

    while pos + 6 < body.len() {
        // time (u16 minutes)
        let minutes = read_u16(body, pos) as u32;
        pos += 2;
        let hour = minutes / 60;
        let minute = minutes % 60;
        let time = format!("{:02}:{:02}", hour, minute);

        // price (delta encoded)
        let (price_diff, new_pos) = get_price(body, pos);
        last_price += price_diff;
        let price = last_price as f64 * coefficient;
        pos = new_pos;

        // vol
        let (vol, new_pos) = get_price(body, pos);
        let vol = vol as f64;
        pos = new_pos;

        // buyorsell
        let (buyorsell, new_pos) = get_price(body, pos);
        let buyorsell = buyorsell as u32;
        pos = new_pos;

        // reserved (原 extra field，具体含义待确认)
        let (reserved, new_pos) = get_price(body, pos);
        let reserved = reserved as u32;
        pos = new_pos;

        result.push(TickData {
            time,
            price,
            vol,
            num: 0,
            buyorsell,
            reserved,
        });
    }

    Ok(result)
}

// ============================================================
// 解析实时行情 (最复杂的解析器)
// ============================================================

pub fn parse_security_quotes(body: &[u8]) -> Result<Vec<SecurityQuote>> {
    if body.len() < 4 {
        return Err(ErrorCode::RESPONSE_LENGTH_MISMATCH.err("body too short"));
    }

    let mut pos = 0;
    pos += 2; // skip b1 cb

    let count = read_u16(body, pos) as usize;
    pos += 2;

    let mut result = Vec::with_capacity(count);

    for _ in 0..count {
        // 边界保护: 每条记录至少需要 ~30 字节，不足则提前终止
        if pos + 30 > body.len() {
            break;
        }

        // market (u8) + code (6 bytes) + active1 (u16)
        let market = body[pos];
        pos += 1;

        let code_bytes = &body[pos..pos + 6];
        let code = String::from_utf8_lossy(code_bytes)
            .trim_end_matches('\0')
            .to_string();
        pos += 6;

        let active1 = read_u16(body, pos);
        pos += 2;

        let coefficient = super::types::get_security_coefficient(market, &code);

        // price (base price, delta encoded)
        let (price_raw, new_pos) = get_price(body, pos);
        pos = new_pos;

        // last_close (diff from price)
        let (last_close_diff, new_pos) = get_price(body, pos);
        pos = new_pos;

        // open (diff from price)
        let (open_diff, new_pos) = get_price(body, pos);
        pos = new_pos;

        // high (diff from price)
        let (high_diff, new_pos) = get_price(body, pos);
        pos = new_pos;

        // low (diff from price)
        let (low_diff, new_pos) = get_price(body, pos);
        pos = new_pos;

        // reversed_bytes0 (get_price as i64, used for servertime)
        let (reversed_bytes0, new_pos) = get_price(body, pos);
        pos = new_pos;

        let (reversed_bytes1, new_pos) = get_price(body, pos);
        pos = new_pos;

        // vol (get_price)
        let (vol, new_pos) = get_price(body, pos);
        pos = new_pos;

        // cur_vol (get_price)
        let (cur_vol, new_pos) = get_price(body, pos);
        pos = new_pos;

        // amount (u32 raw, use get_volume)
        let amount_raw = read_u32(body, pos) as i64;
        let amount = get_volume(amount_raw);
        pos += 4;

        // s_vol (get_price)
        let (s_vol, new_pos) = get_price(body, pos);
        pos = new_pos;

        // b_vol (get_price)
        let (b_vol, new_pos) = get_price(body, pos);
        pos = new_pos;

        // reversed_bytes2, reversed_bytes3
        let (reversed_bytes2, new_pos) = get_price(body, pos);
        pos = new_pos;
        let (reversed_bytes3, new_pos) = get_price(body, pos);
        pos = new_pos;

        // bid1-ask5: interleaved pairs (bid, ask, bid_vol, ask_vol) x 5
        let mut bid_prices = [0.0f64; 5];
        let mut ask_prices = [0.0f64; 5];
        let mut bid_vols = [0.0f64; 5];
        let mut ask_vols = [0.0f64; 5];

        for i in 0..5 {
            let (diff, new_pos) = get_price(body, pos);
            bid_prices[i] = ((price_raw + diff) as f64) * coefficient;
            pos = new_pos;

            let (diff, new_pos) = get_price(body, pos);
            ask_prices[i] = ((price_raw + diff) as f64) * coefficient;
            pos = new_pos;

            let (vol, new_pos) = get_price(body, pos);
            bid_vols[i] = vol as f64;
            pos = new_pos;

            let (vol, new_pos) = get_price(body, pos);
            ask_vols[i] = vol as f64;
            pos = new_pos;
        }

        // reversed_bytes4 (u16)
        let reversed_bytes4 = read_u16(body, pos) as u32;
        pos += 2;

        // reversed_bytes5, reversed_bytes6, reversed_bytes7, reversed_bytes8
        let (reversed_bytes5, new_pos) = get_price(body, pos);
        pos = new_pos;
        let (reversed_bytes6, new_pos) = get_price(body, pos);
        pos = new_pos;
        let (reversed_bytes7, new_pos) = get_price(body, pos);
        pos = new_pos;
        let (reversed_bytes8, new_pos) = get_price(body, pos);
        pos = new_pos;

        // reversed_bytes9 (i16) + active2 (u16)
        let reversed_bytes9 = i16::from_le_bytes([body[pos], body[pos + 1]]);
        pos += 2;
        let active2 = read_u16(body, pos);
        pos += 2;

        // format servertime from reversed_bytes0
        let ts = reversed_bytes0 as u64;
        let servertime = if ts == 0 {
            format!("reversed_bytes0:{}", ts)
        } else {
            let ts_str = format!("{}", ts);
            if ts_str.len() >= 8 {
                let hhmm = &ts_str[..ts_str.len() - 6];
                let mm_ss = &ts_str[ts_str.len() - 6..];
                format!("{}:{}:{}", hhmm, &mm_ss[..2], &mm_ss[2..])
            } else {
                format!("{}", ts)
            }
        };

        let price = (price_raw as f64) * coefficient;
        let last_close = ((price_raw + last_close_diff) as f64) * coefficient;
        let open = ((price_raw + open_diff) as f64) * coefficient;
        let high = ((price_raw + high_diff) as f64) * coefficient;
        let low = ((price_raw + low_diff) as f64) * coefficient;

        result.push(SecurityQuote {
            market,
            code,
            active1,
            price,
            last_close,
            open,
            high,
            low,
            servertime,
            vol: vol as f64,
            cur_vol: cur_vol as f64,
            amount,
            s_vol: s_vol as f64,
            b_vol: b_vol as f64,
            bid1: bid_prices[0],
            bid_vol1: bid_vols[0],
            bid2: bid_prices[1],
            bid_vol2: bid_vols[1],
            bid3: bid_prices[2],
            bid_vol3: bid_vols[2],
            bid4: bid_prices[3],
            bid_vol4: bid_vols[3],
            bid5: bid_prices[4],
            bid_vol5: bid_vols[4],
            ask1: ask_prices[0],
            ask_vol1: ask_vols[0],
            ask2: ask_prices[1],
            ask_vol2: ask_vols[1],
            ask3: ask_prices[2],
            ask_vol3: ask_vols[2],
            ask4: ask_prices[3],
            ask_vol4: ask_vols[3],
            ask5: ask_prices[4],
            ask_vol5: ask_vols[4],
            reversed_bytes0: reversed_bytes0 as u32,
            reversed_bytes1: reversed_bytes1 as u32,
            reversed_bytes2: reversed_bytes2 as u32,
            reversed_bytes3: reversed_bytes3 as u32,
            reversed_bytes4,
            reversed_bytes5: reversed_bytes5 as u32,
            reversed_bytes6: reversed_bytes6 as u32,
            reversed_bytes7: reversed_bytes7 as u32,
            reversed_bytes8: reversed_bytes8 as u32,
            reversed_bytes9: reversed_bytes9 as u32,
            active2,
        });
    }

    Ok(result)
}

// ============================================================
// 解析财务信息
// ============================================================

pub fn parse_finance_info(body: &[u8], market: u8, code: &str) -> Result<FinanceInfo> {
    // Python skips: 2 bytes count + 1 byte market + 6 bytes code = 9 bytes
    // Struct: fHHII + 30*f = 136 bytes
    if body.len() < 9 + 136 {
        return Err(ErrorCode::RESPONSE_LENGTH_MISMATCH.err("body too short for finance info"));
    }

    let mut pos = 9; // skip count(2) + market(1) + code(6)

    // f32 liutongguben — TDX 原始值 (单位不固定，由用户自行判断)
    let liutongguben = f32::from_le_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]) as f64;
    pos += 4;

    // u16 province
    let province = read_u16(body, pos);
    pos += 2;

    // u16 industry
    let industry = read_u16(body, pos);
    pos += 2;

    // u32 updated_date
    let updated_date = read_u32(body, pos);
    pos += 4;

    // u32 ipo_date
    let ipo_date = read_u32(body, pos);
    pos += 4;

    // 30 个 f32 字段 — 全部返回 TDX 原始值，不做单位转换
    let mut fields = Vec::with_capacity(30);
    for _ in 0..30 {
        let val = f32::from_le_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]) as f64;
        fields.push(val);
        pos += 4;
    }

    Ok(FinanceInfo {
        market,
        code: code.to_string(),
        liutongguben,
        province,
        industry,
        updated_date,
        ipo_date,
        zongguben: fields[0],
        guojiagu: fields[1],
        faqirenfarengu: fields[2],
        farengu: fields[3],
        bgu: fields[4],
        hgu: fields[5],
        zhigonggu: fields[6],
        zongzichan: fields[7],
        liudongzichan: fields[8],
        gudingzichan: fields[9],
        wuxingzichan: fields[10],
        gudongrenshu: fields[11],
        liudongfuzhai: fields[12],
        changqifuzhai: fields[13],
        zibengongjijin: fields[14],
        jingzichan: fields[15],
        zhuyingshouru: fields[16],
        zhuyinglirun: fields[17],
        yingshouzhangkuan: fields[18],
        yingyelirun: fields[19],
        touzishouyu: fields[20],
        jingyingxianjinliu: fields[21],
        zongxianjinliu: fields[22],
        cunhuo: fields[23],
        lirunzonghe: fields[24],
        shuihoulirun: fields[25],
        jinglirun: fields[26],
        weifenpeilirun: fields[27],
        meigujingzichan: fields[28],
    })
}

// ============================================================
// 解析除权除息
// ============================================================

pub fn parse_xdxr_info(body: &[u8]) -> Result<Vec<XdXrInfo>> {
    // Python: pos=0, pos+=9 (skip 9 bytes), read count at pos=9
    if body.len() < 11 {
        return Err(ErrorCode::RESPONSE_LENGTH_MISMATCH.err("body too short"));
    }

    let mut pos = 9;
    let count = read_u16(body, pos) as usize;
    pos += 2;

    let mut result = Vec::with_capacity(count);

    for _ in 0..count {
        // 7(skip) + 1(skip) + 4(datetime) + 1(category) + 16(data) = 29 bytes per record
        if pos + 29 > body.len() {
            break;
        }

        // Python: pos += 7 (skip unknown bytes)
        pos += 7;
        // Python: pos += 1 (skip 1 byte)
        pos += 1;

        // datetime (category 9 → YYYYMMDD u32)
        let (year, month, day, _hour, _minute, new_pos) = get_datetime(9, body, pos);
        pos = new_pos;

        // category (u8)
        let category = body[pos] as u32;
        pos += 1;

        // 16 bytes data (parsed differently by category)
        let mut fenhong = None;
        let mut peigujia = None;
        let mut songzhuangu = None;
        let mut peigu = None;
        let mut suogu = None;
        let mut panqianliutong = None;
        let mut panhouliutong = None;
        let mut qianzongguben = None;
        let mut houzongguben = None;
        let mut fenshu = None;
        let mut xingquanjia = None;

        if pos + 16 <= body.len() {
            let d = &body[pos..pos + 16];
            if category == 1 {
                fenhong = Some(f32::from_le_bytes([d[0], d[1], d[2], d[3]]) as f64);
                peigujia = Some(f32::from_le_bytes([d[4], d[5], d[6], d[7]]) as f64);
                songzhuangu = Some(f32::from_le_bytes([d[8], d[9], d[10], d[11]]) as f64);
                peigu = Some(f32::from_le_bytes([d[12], d[13], d[14], d[15]]) as f64);
            } else if category == 11 || category == 12 {
                suogu = Some(f32::from_le_bytes([d[8], d[9], d[10], d[11]]) as f64);
            } else if category == 13 || category == 14 {
                xingquanjia = Some(f32::from_le_bytes([d[0], d[1], d[2], d[3]]) as f64);
                fenshu = Some(f32::from_le_bytes([d[8], d[9], d[10], d[11]]) as f64);
            } else {
                let pqlt_raw = read_u32(d, 0);
                let qzgb_raw = read_u32(d, 4);
                let phlt_raw = read_u32(d, 8);
                let hzgb_raw = read_u32(d, 12);
                panqianliutong = Some(if pqlt_raw == 0 { 0.0 } else { get_volume(pqlt_raw as i64) });
                panhouliutong = Some(if phlt_raw == 0 { 0.0 } else { get_volume(phlt_raw as i64) });
                qianzongguben = Some(if qzgb_raw == 0 { 0.0 } else { get_volume(qzgb_raw as i64) });
                houzongguben = Some(if hzgb_raw == 0 { 0.0 } else { get_volume(hzgb_raw as i64) });
            }
        }
        pos += 16;

        let name = match category {
            1 => "除权除息",
            2 => "送配股上市",
            3 => "非流通股上市",
            4 => "未知股本变动",
            5 => "股本变化",
            6 => "增发新股",
            7 => "股份回购",
            8 => "增发新股上市",
            9 => "转配股上市",
            10 => "可转债上市",
            11 => "扩缩股",
            12 => "非流通股缩股",
            13 => "送认购权证",
            14 => "送认沽权证",
            _ => "未知",
        }
        .to_string();

        result.push(XdXrInfo {
            year,
            month,
            day,
            category,
            name,
            fenhong,
            peigujia,
            songzhuangu,
            peigu,
            suogu,
            panqianliutong,
            panhouliutong,
            qianzongguben,
            houzongguben,
            fenshu,
            xingquanjia,
        });
    }

    Ok(result)
}

// ============================================================
// 解析板块元数据
// ============================================================

pub fn parse_block_info_meta(body: &[u8]) -> Result<BlockInfoMeta> {
    if body.len() < 38 {
        return Err(ErrorCode::RESPONSE_LENGTH_MISMATCH.err("body too short for block meta"));
    }

    let size = read_u32(body, 0);

    // 1 byte separator
    // 32 bytes hash
    let hash_bytes = &body[5..37];
    let hash_value: String = hash_bytes.iter().map(|b| format!("{:02x}", b)).collect();

    Ok(BlockInfoMeta {
        size,
        hash_value,
    })
}

// ============================================================
// 解析板块数据 (返回原始字节)
// ============================================================

pub fn parse_block_info(body: &[u8]) -> Result<Vec<u8>> {
    // 跳过前 4 bytes header
    if body.len() > 4 {
        Ok(body[4..].to_vec())
    } else {
        Ok(Vec::new())
    }
}

// ============================================================
// 辅助函数: 日期时间解码
// ============================================================

fn get_datetime(category: u8, buffer: &[u8], pos: usize) -> (u32, u32, u32, u32, u32, usize) {
    if category < 4 || category == 7 || category == 8 {
        // 分钟级: u16 date + u16 minutes
        let zip_day = read_u16(buffer, pos) as u32;
        let minutes = read_u16(buffer, pos + 2) as u32;

        let year = (zip_day >> 11) + 2004;
        let month = (zip_day % 2048) / 100;
        let day = (zip_day % 2048) % 100;
        let hour = minutes / 60;
        let minute = minutes % 60;

        (year, month, day, hour, minute, pos + 4)
    } else {
        // 日/周/月级: u32 date (YYYYMMDD)
        let zip_day = read_u32(buffer, pos);
        let year = zip_day / 10000;
        let month = (zip_day % 10000) / 100;
        let day = zip_day % 100;

        (year, month, day, 0, 0, pos + 4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_security_count ---

    #[test]
    fn test_security_count_empty() {
        assert!(parse_security_count(&[]).is_err());
    }

    #[test]
    fn test_security_count_one_byte() {
        assert!(parse_security_count(&[0x01]).is_err());
    }

    #[test]
    fn test_security_count_zero() {
        let result = parse_security_count(&[0x00, 0x00]).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_security_count_normal() {
        let result = parse_security_count(&[0xE8, 0x03]).unwrap(); // 1000
        assert_eq!(result, 1000);
    }

    // --- parse_security_list ---

    #[test]
    fn test_security_list_empty_body() {
        assert!(parse_security_list(&[]).is_err());
    }

    #[test]
    fn test_security_list_one_byte() {
        assert!(parse_security_list(&[0x01]).is_err());
    }

    #[test]
    fn test_security_list_zero_count() {
        let result = parse_security_list(&[0x00, 0x00]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_security_list_truncated_record() {
        // count=1 but only 10 bytes (need 29)
        let mut data = vec![0x01, 0x00];
        data.extend_from_slice(&[0u8; 10]);
        let result = parse_security_list(&data).unwrap();
        assert!(result.is_empty()); // breaks early
    }

    #[test]
    fn test_security_list_one_record() {
        // count=1, record_size=29
        let mut data = vec![0x01, 0x00];
        let mut record = vec![0u8; 29];
        // code: "600519\0"
        record[..6].copy_from_slice(b"600519");
        // name: GBK encoded "贵州茅台\0\0\0\0"
        let (gbk_bytes, _, _) = GBK.encode("贵州茅台");
        record[8..8 + gbk_bytes.len()].copy_from_slice(&gbk_bytes);
        data.extend_from_slice(&record);
        let result = parse_security_list(&data).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].code, "600519");
        assert_eq!(result[0].name, "贵州茅台");
    }

    // --- parse_security_bars ---

    #[test]
    fn test_security_bars_empty() {
        assert!(parse_security_bars(&[], 4).is_err());
    }

    #[test]
    fn test_security_bars_zero_count() {
        let result = parse_security_bars(&[0x00, 0x00], 4).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_security_bars_truncated() {
        // count=1 but only 5 bytes (need at least 16)
        let mut data = vec![0x01, 0x00];
        data.extend_from_slice(&[0u8; 5]);
        let result = parse_security_bars(&data, 4).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_security_bars_daily_format() {
        // category=4 (daily): u32 date + 4*price(var) + vol(4) + amount(4)
        // Build minimal: date=20260429, then 4 zero prices (0x00), vol=0, amount=0
        let mut data = vec![0x01, 0x00]; // count=1
        data.extend_from_slice(&20260429u32.to_le_bytes()); // date
        data.extend_from_slice(&[0x00; 16]); // 4 prices(var=1B each) + vol(4) + amount(4)
        let result = parse_security_bars(&data, 4).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].year, 2026);
        assert_eq!(result[0].month, 4);
        assert_eq!(result[0].day, 29);
    }

    // --- parse_index_bars ---

    #[test]
    fn test_index_bars_empty() {
        assert!(parse_index_bars(&[], 4).is_err());
    }

    #[test]
    fn test_index_bars_zero_count() {
        let result = parse_index_bars(&[0x00, 0x00], 4).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_index_bars_truncated() {
        let mut data = vec![0x01, 0x00];
        data.extend_from_slice(&[0u8; 10]);
        let result = parse_index_bars(&data, 4).unwrap();
        assert!(result.is_empty());
    }

    // --- parse_minute_time_data ---

    #[test]
    fn test_minute_time_empty() {
        assert!(parse_minute_time_data(&[], 1, "600519").is_err());
    }

    #[test]
    fn test_minute_time_too_short() {
        assert!(parse_minute_time_data(&[0x00], 1, "600519").is_err());
    }

    #[test]
    fn test_minute_time_zero_count() {
        // 头部: 2(count) + 2(padding) + 1(indicator) + 6(stock_code) + 2(unknown) = 13 bytes
        // 需要至少 14 字节 (13 头部 + 1 数据)
        let body = [0x00, 0x00, 0x00, 0x00, 0x01, 0x36, 0x30, 0x30, 0x35, 0x31, 0x39, 0x00, 0x00, 0x00];
        let result = parse_minute_time_data(&body, 1, "600519").unwrap();
        assert!(result.is_empty());
    }

    // --- parse_history_minute_time_data ---

    #[test]
    fn test_history_minute_time_empty() {
        let result = parse_history_minute_time_data(&[], 1, "600519").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_history_minute_time_short_header() {
        // Less than 6 bytes header
        let result = parse_history_minute_time_data(&[0u8; 5], 1, "600519").unwrap();
        assert!(result.is_empty());
    }

    // --- parse_transaction_data ---

    #[test]
    fn test_transaction_empty() {
        assert!(parse_transaction_data(&[]).is_err());
    }

    #[test]
    fn test_transaction_one_byte() {
        assert!(parse_transaction_data(&[0x01]).is_err());
    }

    #[test]
    fn test_transaction_zero_count() {
        let result = parse_transaction_data(&[0x00, 0x00]).unwrap();
        assert!(result.is_empty());
    }

    // --- parse_history_transaction_data ---

    #[test]
    fn test_history_transaction_short() {
        assert!(parse_history_transaction_data(&[0u8; 5]).is_err());
    }

    #[test]
    fn test_history_transaction_empty_body() {
        let result = parse_history_transaction_data(&[0u8; 6]).unwrap();
        assert!(result.is_empty());
    }

    // --- parse_security_quotes ---

    #[test]
    fn test_quotes_empty() {
        assert!(parse_security_quotes(&[]).is_err());
    }

    #[test]
    fn test_quotes_too_short() {
        assert!(parse_security_quotes(&[0x00, 0x00, 0x00]).is_err());
    }

    #[test]
    fn test_quotes_zero_count() {
        // b1 cb (2 bytes) + count=0 (2 bytes)
        let result = parse_security_quotes(&[0x00, 0x00, 0x00, 0x00]).unwrap();
        assert!(result.is_empty());
    }

    // --- parse_finance_info ---

    #[test]
    fn test_finance_empty() {
        assert!(parse_finance_info(&[], 1, "600519").is_err());
    }

    #[test]
    fn test_finance_short() {
        assert!(parse_finance_info(&[0u8; 50], 1, "600519").is_err());
    }

    #[test]
    fn test_finance_valid() {
        // 9 bytes header + 136 bytes struct = 145 bytes minimum
        let mut data = vec![0u8; 145];
        // Set liutongguben (first f32 at pos 9)
        let liutongguben: f32 = 10.0;
        data[9..13].copy_from_slice(&liutongguben.to_le_bytes());
        let result = parse_finance_info(&data, 1, "600519").unwrap();
        assert_eq!(result.code, "600519");
        assert_eq!(result.market, 1);
        // raw value, no unit conversion
        assert!((result.liutongguben - 10.0).abs() < 0.1);
    }

    // --- parse_xdxr_info ---

    #[test]
    fn test_xdxr_empty() {
        assert!(parse_xdxr_info(&[]).is_err());
    }

    #[test]
    fn test_xdxr_short() {
        assert!(parse_xdxr_info(&[0u8; 10]).is_err());
    }

    #[test]
    fn test_xdxr_zero_count() {
        /* 9 bytes header + 2 bytes count=0 */
        let data = vec![0u8; 11];
        let result = parse_xdxr_info(&data).unwrap();
        assert!(result.is_empty());
    }

    // --- parse_block_info_meta ---

    #[test]
    fn test_block_meta_empty() {
        assert!(parse_block_info_meta(&[]).is_err());
    }

    #[test]
    fn test_block_meta_short() {
        assert!(parse_block_info_meta(&[0u8; 30]).is_err());
    }

    #[test]
    fn test_block_meta_valid() {
        let mut data = vec![0u8; 38];
        data[0..4].copy_from_slice(&1000u32.to_le_bytes()); // size
        data[5..37].copy_from_slice(&[0xAB; 32]); // hash
        let result = parse_block_info_meta(&data).unwrap();
        assert_eq!(result.size, 1000);
        assert_eq!(result.hash_value.len(), 64); // 32 bytes * 2 hex chars
    }

    // --- parse_block_info ---

    #[test]
    fn test_block_info_empty() {
        let result = parse_block_info(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_block_info_short_header() {
        let result = parse_block_info(&[0u8; 3]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_block_info_valid() {
        let mut data = vec![0u8; 4]; // 4 byte header
        data.extend_from_slice(&[0x42; 10]); // 10 bytes payload
        let result = parse_block_info(&data).unwrap();
        assert_eq!(result.len(), 10);
        assert!(result.iter().all(|&b| b == 0x42));
    }

    // --- parse_call_auction ---

    fn call_auction_record(minutes: u16, price: f32, matched: u32, unmatched: i32, second: u8) -> Vec<u8> {
        let mut rec = Vec::with_capacity(16);
        rec.extend_from_slice(&minutes.to_le_bytes());
        rec.extend_from_slice(&price.to_le_bytes());
        rec.extend_from_slice(&matched.to_le_bytes());
        rec.extend_from_slice(&unmatched.to_le_bytes());
        rec.push(second);
        rec.push(0);
        rec
    }

    #[test]
    fn test_call_auction_empty() {
        assert!(parse_call_auction(&[]).is_err());
        assert!(parse_call_auction(&[0x00]).is_err());
    }

    #[test]
    fn test_call_auction_zero_count() {
        let rows = parse_call_auction(&[0x00, 0x00]).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn test_call_auction_two_ticks() {
        let mut body = vec![0x02, 0x00];
        body.extend(call_auction_record(9 * 60 + 15, 10.03, 100, 62, 0));
        body.extend(call_auction_record(9 * 60 + 25, 10.06, 3866, -293, 3));

        let rows = parse_call_auction(&body).unwrap();
        assert_eq!(rows.len(), 2);

        assert_eq!(rows[0].time, "09:15:00");
        assert!((rows[0].price - 10.03).abs() < 1e-6);
        assert_eq!(rows[0].match_vol, 100);
        assert_eq!(rows[0].unmatched_vol, 62);
        assert_eq!(rows[0].side, 1);

        assert_eq!(rows[1].time, "09:25:03");
        // 负数未匹配 → 卖方排队，量取绝对值
        assert_eq!(rows[1].side, -1);
        assert_eq!(rows[1].unmatched_vol, 293);
    }

    #[test]
    fn test_call_auction_large_unmatched_keeps_sign() {
        // 109912 手 = 0x0001AD58，低 16 位会被读成负数，必须整 4 字节读
        let mut body = vec![0x01, 0x00];
        body.extend(call_auction_record(9 * 60 + 20, 9.55, 5000, 109_912, 12));
        let rows = parse_call_auction(&body).unwrap();
        assert_eq!(rows[0].side, 1);
        assert_eq!(rows[0].unmatched_vol, 109_912);
    }

    #[test]
    fn test_call_auction_truncated_record() {
        let mut body = vec![0x02, 0x00];
        body.extend(call_auction_record(9 * 60 + 15, 10.03, 100, 62, 0));
        body.extend_from_slice(&[0u8; 5]); // 第二条只有半截
        let rows = parse_call_auction(&body).unwrap();
        assert_eq!(rows.len(), 1);
    }
}
