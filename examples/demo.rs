//! tdxrs Rust 使用示例
//!
//! 运行方式:
//!   cargo run --example demo
//!
//! 前提: 需要网络连接到 TDX 服务器 (默认 218.75.126.9:7709)

use tdxrs::net::client::TdxHqClient;

fn main() {
    println!("=== tdxrs Rust Demo ===\n");

    // 1. 创建客户端并连接
    let client = TdxHqClient::new();
    match client.connect("139.9.52.158", 7709, Some(5.0)) {
        Ok(true) => println!("[OK] Connected to server"),
        Ok(false) => println!("[FAIL] Connection rejected"),
        Err(e) => {
            println!("[ERROR] {}", e);
            println!("  Trying connect_to_any...");
            match client.connect_to_any(None) {
                Ok(true) => println!("[OK] Connected via failover"),
                _ => {
                    println!("[FAIL] No server available");
                    return;
                }
            }
        }
    }

    // 2. 获取证券数量
    let sh_count = client.get_security_count(1).unwrap_or(0);
    let sz_count = client.get_security_count(0).unwrap_or(0);
    println!("\n--- Security Count ---");
    println!("  Shanghai: {}", sh_count);
    println!("  Shenzhen: {}", sz_count);

    // 3. 获取贵州茅台日K (最近5条)
    println!("\n--- 600519 Daily K-line (last 5) ---");
    match client.get_security_bars(4, 1, "600519", 0, 5, 0) {
        Ok(bars) => {
            for bar in &bars {
                println!(
                    "  {} O={:.2} C={:.2} H={:.2} L={:.2} V={:.0}",
                    bar.datetime, bar.open, bar.close, bar.high, bar.low, bar.vol
                );
            }
        }
        Err(e) => println!("  Error: {}", e),
    }

    // 4. 获取实时行情
    println!("\n--- Real-time Quotes ---");
    match client.get_security_quotes(&[(1, "600519"), (0, "000858")]) {
        Ok(quotes) => {
            for q in &quotes {
                println!(
                    "  {} Price={:.2} Vol={:.0} Amount={:.0}",
                    q.code, q.price, q.vol, q.amount
                );
            }
        }
        Err(e) => println!("  Error: {}", e),
    }

    // 5. 获取上证指数分时
    println!("\n--- 000001 Intraday (first 5 ticks) ---");
    match client.get_minute_time_data(1, "000001") {
        Ok(data) => {
            for d in data.iter().take(5) {
                println!("  Price={:.2} Vol={:.0}", d.price, d.vol);
            }
            println!("  ... total {} ticks", data.len());
        }
        Err(e) => println!("  Error: {}", e),
    }

    // 6. 连接池状态
    let stats = client.pool_stats();
    println!("\n--- Pool Stats ---");
    println!(
        "  idle={} active={} total={} max={}",
        stats.idle, stats.active, stats.total, stats.max_size
    );

    // 7. 断开连接
    client.disconnect();
    println!("\n[OK] Disconnected");
}
