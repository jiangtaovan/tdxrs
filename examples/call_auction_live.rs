//! 集合竞价（0x056a）实盘验证：cargo run --example call_auction_live

use tdxrs::net::client::TdxHqClient;

fn main() {
    let client = TdxHqClient::new();
    match client.connect("110.41.147.114", 7709, Some(5.0)) {
        Ok(true) => println!("[OK] connected"),
        other => {
            println!("[FAIL] {:?}", other);
            return;
        }
    }

    for code in ["600000", "000001"] {
        let market = if code.starts_with('6') { 1u8 } else { 0u8 };
        match client.get_call_auction(market, code) {
            Ok(rows) => {
                println!("\n{} 集合竞价 {} 条", code, rows.len());
                for r in rows.iter().take(5) {
                    println!(
                        "  {}  {:>8.3}  匹配 {:>8}  未匹配 {:>8}  {}",
                        r.time,
                        r.price,
                        r.match_vol,
                        r.unmatched_vol,
                        if r.side > 0 { "买盘排队" } else { "卖盘排队" }
                    );
                }
                if let Some(last) = rows.last() {
                    println!("  最后一条: {} {:>8.3} 匹配 {}", last.time, last.price, last.match_vol);
                }
            }
            Err(e) => println!("{} 失败: {}", code, e),
        }
    }
}
