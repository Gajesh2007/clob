//! Benchmark client and load generator.
//!
//! Responsibilities
//! - Creates synthetic users via HTTP (`/users`) and funds them via `/deposit`
//! - Opens TCP connections to the ingress (`tcp_addr`) and streams signed orders
//! - Subscribes to `market-events:{id}` on Redis to measure end-to-end latency
//! - Prints throughput and latency percentiles at the end of the run
//!
use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::time::{self, Instant, Duration};
use tokio::io::AsyncWriteExt;
use futures_util::stream::StreamExt;
use common_types::{
    Order, SignedOrder, Side, OrderType, OrderID, UserID, MarketID, Signature, AssetID, MarketEvent
};
use ed25519_dalek::{SigningKey, Signer};
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;
use rand::Rng;
use rand::rngs::StdRng;
use rand::SeedableRng;
use redis::AsyncCommands;
use clap::Parser;
use tracing::{info, error};
use dashmap::DashMap;
use hdrhistogram::Histogram;
use tokio::sync::Mutex;
use url::Url;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
/// Command-line arguments for configuring the benchmark run.
struct Args {
    #[arg(long, default_value_t = String::from("127.0.0.1:9090"))]
    http_addr: String,
    #[arg(long, default_value_t = String::from("127.0.0.1:8080"))]
    tcp_addr: String,
    #[arg(long, default_value_t = String::from("redis://127.0.0.1/"))]
    redis_addr: String,
    #[arg(long, default_value_t = 1)]
    market_id: u32,
    #[arg(long, default_value_t = 10)]
    num_traders: u32,
    #[arg(long, default_value_t = 30)]
    duration_secs: u64,
    #[arg(long, default_value_t = String::from(""))]
    gateway_addr: String,
    #[arg(long, value_parser = ["redis", "gateway_ws"], default_value_t = String::from("redis"))]
    event_source: String,
}

#[derive(Deserialize, Debug)]
struct CreateUserResponse {
    user_id: UserID,
    private_key_hex: String,
}

#[derive(Serialize)]
struct DepositRequest {
    user_id: UserID,
    asset_id: AssetID,
    amount: Decimal,
}

async fn create_and_fund_user(
    http_client: &reqwest::Client,
    http_addr: &str,
    assets: &[(u32, Decimal)],
) -> Result<(UserID, SigningKey), Box<dyn Error>> {
    // Create a new user and receive a private key for signing orders
    let create_user_url = format!("http://{}/users", http_addr);
    let resp = http_client.post(&create_user_url).send().await?.json::<CreateUserResponse>().await?;
    let user_id = resp.user_id;
    let private_key_bytes = hex::decode(resp.private_key_hex)?;
    let signing_key = SigningKey::from_bytes(&private_key_bytes.try_into().unwrap());
    info!(user_id = %user_id, "Created user");

    for (asset_id, amount) in assets {
        // Seed the account with balances to enable trading
        let deposit_url = format!("http://{}/deposit", http_addr);
        let deposit_req = DepositRequest { user_id, asset_id: AssetID(*asset_id), amount: *amount };
    http_client.post(&deposit_url).json(&deposit_req).send().await?.error_for_status()?;
        info!(user_id = %user_id, "Funded user with {} of asset {}", amount, asset_id);
    }
    
    Ok((user_id, signing_key))
}

/// Send a single signed order over the length-prefixed TCP protocol.
async fn send_order(stream: &mut TcpStream, signed_order: &SignedOrder) -> Result<(), Box<dyn Error + Send + Sync>> {
    let payload = bincode::serialize(signed_order)?;
    let len = payload.len() as u32;
    stream.write_u32(len).await?;
    stream.write_all(&payload).await?;
    Ok(())
}

struct Trader {
    user_id: UserID,
    signing_key: SigningKey,
    side: Side,
}

#[tokio::main]
/// Entry point: sets up users, generates orders, and reports metrics.
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let http_client = reqwest::Client::new();

    info!("--- Clearing previous state ---");
    let redis_client = redis::Client::open(args.redis_addr.as_str())?;
    let mut redis_conn = redis_client.get_multiplexed_async_connection().await?;
    let _: () = redis_conn.del("execution_log").await?;
    info!("State cleared.");

    info!("--- Setting up benchmark traders ---");
    let mut traders = Vec::new();
    for i in 0..args.num_traders {
        let (buy_user_id, buy_signing_key) = create_and_fund_user(&http_client, &args.http_addr, &[(1, dec!(1_000_000.0)), (2, dec!(10_000.0))]).await?;
        let (sell_user_id, sell_signing_key) = create_and_fund_user(&http_client, &args.http_addr, &[(1, dec!(1_000_000.0)), (2, dec!(10_000.0))]).await?;
        traders.push(Trader { user_id: buy_user_id, signing_key: buy_signing_key, side: Side::Buy });
        traders.push(Trader { user_id: sell_user_id, signing_key: sell_signing_key, side: Side::Sell });
        info!("Created trader pair {}", i + 1);
    }
    info!("--------------------------\n");

    info!("--- Allowing 2s for deposits to be processed ---");
    time::sleep(Duration::from_secs(2)).await;

    let trade_count = Arc::new(AtomicU64::new(0));
    let orders_sent_count = Arc::new(AtomicU64::new(0));
    let test_duration = Duration::from_secs(args.duration_secs);
    let tcp_addr = Arc::new(args.tcp_addr);
    let market_id = args.market_id;

    let pending_orders = Arc::new(DashMap::<OrderID, Instant>::new());
    let latency_histogram = Arc::new(Mutex::new(Histogram::<u64>::new(2).unwrap()));

    let trade_count_clone = trade_count.clone();
    let pending_orders_clone = pending_orders.clone();
    let latency_hist_clone = latency_histogram.clone();
    match args.event_source.as_str() {
        "redis" => {
            let pattern = "market-events:*".to_string();
            // Robust subscription with simple retry loop (pattern subscribe to avoid channel drift)
            let mut pubsub_conn = loop {
                match redis_client.get_async_connection().await {
                    Ok(c) => {
                        let mut ps = c.into_pubsub();
                        if let Err(e) = ps.psubscribe(pattern.as_str()).await {
                            tracing::error!(error = %e, "Failed to psubscribe to events; retrying");
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            continue;
                        }
                        break ps;
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to get Redis connection; retrying");
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
            };
            tokio::spawn(async move {
                let mut message_stream = pubsub_conn.on_message();
                info!("Trade counter and latency recorder started on Redis (psubscribe market-events:*)");
                while let Ok(Some(msg)) = time::timeout(test_duration + Duration::from_secs(5), message_stream.next()).await {
                    // Filter for the configured market id from the channel name
                    let channel_name: String = msg.get_channel_name().to_string();
                    let market_ok = channel_name.split(':').nth(1).and_then(|s| s.parse::<u32>().ok()).map(|id| id == market_id).unwrap_or(false);
                    if !market_ok { continue; }
                    let payload: Vec<u8> = match msg.get_payload() { Ok(p) => p, Err(_) => continue };
                    if let Ok(MarketEvent::OrderTraded(trade)) = bincode::deserialize(&payload) {
                        trade_count_clone.fetch_add(1, Ordering::SeqCst);
                        if let Some((_, start_time)) = pending_orders_clone.remove(&trade.maker_order_id) {
                            let latency = start_time.elapsed().as_micros() as u64;
                            let _ = latency_hist_clone.lock().await.record(latency);
                        }
                        if let Some((_, start_time)) = pending_orders_clone.remove(&trade.taker_order_id) {
                            let latency = start_time.elapsed().as_micros() as u64;
                            let _ = latency_hist_clone.lock().await.record(latency);
                        }
                    }
                }
                info!("Trade counter finished.");
            });
        }
        "gateway_ws" => {
            use tokio_tungstenite::tungstenite::Message;
            let base = if args.gateway_addr.is_empty() { "127.0.0.1:9100" } else { &args.gateway_addr };
            let url = Url::parse(&format!("ws://{}/ws?market_id={}&format=binary", base, market_id)).unwrap();
            tokio::spawn(async move {
                let (ws_stream, _) = match tokio_tungstenite::connect_async(url).await {
                    Ok(t) => t,
                    Err(e) => {
                        error!("Gateway WS connect error: {}", e);
                        return;
                    }
                };
                let (_, mut read) = ws_stream.split();
                info!("Trade counter and latency recorder started via gateway WS.");
                while let Ok(Some(msg)) = time::timeout(test_duration + Duration::from_secs(5), read.next()).await {
                    match msg {
                        Ok(Message::Binary(payload)) => {
                            if let Ok(MarketEvent::OrderTraded(trade)) = bincode::deserialize(&payload) {
                                trade_count_clone.fetch_add(1, Ordering::SeqCst);
                                if let Some((_, start_time)) = pending_orders_clone.remove(&trade.maker_order_id) {
                                    let latency = start_time.elapsed().as_micros() as u64;
                                    latency_hist_clone.lock().await.record(latency).unwrap();
                                }
                                if let Some((_, start_time)) = pending_orders_clone.remove(&trade.taker_order_id) {
                                    let latency = start_time.elapsed().as_micros() as u64;
                                    latency_hist_clone.lock().await.record(latency).unwrap();
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
                info!("Trade counter finished.");
            });
        }
        other => {
            panic!("unsupported event_source: {}", other);
        }
    }

    info!("--- Starting benchmark for {} seconds ---", args.duration_secs);
    let start_time = Instant::now();
    let mut handles = Vec::new();

    for trader in traders {
        let tcp_addr_clone = tcp_addr.clone();
        let pending_orders_clone = pending_orders.clone();
        let orders_sent_clone = orders_sent_count.clone();
        let handle = tokio::spawn(async move {
            let mut rng = StdRng::from_entropy();
            let mut stream = match TcpStream::connect(tcp_addr_clone.as_str()).await {
                Ok(s) => s,
                Err(e) => {
                    error!("Trader failed to connect: {}", e);
                    return;
                }
            };

            while Instant::now() - start_time < test_duration {
                let price_offset = match trader.side {
                    Side::Buy  => Decimal::from(rng.gen_range(-100..=20)), // Range: 99.00 to 100.20
                    Side::Sell => Decimal::from(rng.gen_range(-20..=100)), // Range: 99.80 to 101.00
                } / dec!(100);
                let price = dec!(100.0) + price_offset;
                let order = Order {
                    order_id: OrderID(rng.gen()),
                    user_id: trader.user_id,
                    market_id: MarketID(market_id),
                    side: trader.side,
                    price,
                    quantity: dec!(1.0),
                    order_type: OrderType::Limit,
                    timestamp: 0,
                };
                // Sign and send the order over the TCP length-prefixed protocol
                let message_bytes = bincode::serialize(&order).unwrap();
                let signature = trader.signing_key.sign(&message_bytes);
                let signed_order = SignedOrder { order, signature: Signature(signature) };

                pending_orders_clone.insert(order.order_id, Instant::now());
                orders_sent_clone.fetch_add(1, Ordering::SeqCst);
                
                if let Err(e) = send_order(&mut stream, &signed_order).await {
                    error!("Failed to send order: {}. Closing connection.", e);
                    pending_orders_clone.remove(&order.order_id);
                    orders_sent_clone.fetch_sub(1, Ordering::SeqCst);
                    break;
                }
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await?;
    }

    info!("--- Benchmark finished sending orders ---");
    
    time::sleep(Duration::from_secs(3)).await;

    let total_trades = trade_count.load(Ordering::SeqCst);
    let total_orders_sent = orders_sent_count.load(Ordering::SeqCst);
    let elapsed_secs = start_time.elapsed().as_secs_f64();
    let tps = total_trades as f64 / elapsed_secs;
    let success_rate = if total_orders_sent > 0 { (total_trades * 2) as f64 / total_orders_sent as f64 * 100.0 } else { 0.0 };

    let hist = latency_histogram.lock().await;
    println!("\n========================================");
    println!("           BENCHMARK RESULTS");
    println!("----------------------------------------");
    println!("Test Duration:      {:.2} seconds", elapsed_secs);
    println!("Throughput:         {:.2} TPS", tps);
    println!("----------------------------------------");
    println!("Order Execution:");
    println!("Total Orders Sent:  {}", total_orders_sent);
    println!("Total Trades:       {}", total_trades);
    println!("Success Rate:       {:.2}%", success_rate);
    println!("----------------------------------------");
    println!("Latency (microseconds):");
    println!("Mean:               {:.2}", hist.mean());
    println!("Min:                {}", hist.min());
    println!("Max:                {}", hist.max());
    println!("p50 (Median):       {}", hist.value_at_percentile(50.0));
    println!("p90:                {}", hist.value_at_percentile(90.0));
    println!("p99:                {}", hist.value_at_percentile(99.0));
    println!("p99.9:              {}", hist.value_at_percentile(99.9));
    println!("========================================");
    
    Ok(())
}
