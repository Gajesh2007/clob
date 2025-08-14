use std::error::Error;
// UDP multicast imports removed; using Redis PubSub for cross-VM counting
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

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
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
    let create_user_url = format!("http://{}/users", http_addr);
    let resp = http_client.post(&create_user_url).send().await?.json::<CreateUserResponse>().await?;
    let user_id = resp.user_id;
    let private_key_bytes = hex::decode(resp.private_key_hex)?;
    let signing_key = SigningKey::from_bytes(&private_key_bytes.try_into().unwrap());
    info!(user_id = %user_id, "Created user");

    for (asset_id, amount) in assets {
        let deposit_url = format!("http://{}/deposit", http_addr);
        let deposit_req = DepositRequest { user_id, asset_id: AssetID(*asset_id), amount: *amount };
    http_client.post(&deposit_url).json(&deposit_req).send().await?.error_for_status()?;
        info!(user_id = %user_id, "Funded user with {} of asset {}", amount, asset_id);
    }
    
    Ok((user_id, signing_key))
}

async fn send_order(stream: &mut TcpStream, signed_order: &SignedOrder) -> Result<(), Box<dyn Error + Send + Sync>> {
    let payload = bincode::serialize(signed_order)?;
    // Use length-prefix framing to send multiple orders on one connection
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
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let http_client = reqwest::Client::new();

    // 0. Clear the execution log from previous runs for a clean state.
    info!("--- Clearing previous state ---");
    let redis_client = redis::Client::open(args.redis_addr.as_str())?;
    let mut redis_conn = redis_client.get_async_connection().await?;
    let _: () = redis_conn.del("execution_log").await?;
    info!("State cleared.");

    // 1. Create and fund a pool of traders.
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

    // 2. Set up counters and run duration.
    let trade_count = Arc::new(AtomicU64::new(0));
    let test_duration = Duration::from_secs(args.duration_secs);
    let tcp_addr = Arc::new(args.tcp_addr);
    let market_id = args.market_id;

    // 3. Listen for trades on Redis Pub/Sub in a separate task to count them.
    let trade_count_clone = trade_count.clone();
    let events_channel = format!("market-events:{}", market_id);
    let mut pubsub_conn = redis_client.get_async_connection().await?.into_pubsub();
    pubsub_conn.subscribe(events_channel).await?;
    
    tokio::spawn(async move {
        let mut message_stream = pubsub_conn.on_message();
        info!("Trade counter started on Redis.");
        while let Ok(Some(msg)) = time::timeout(test_duration + Duration::from_secs(5), message_stream.next()).await {
            let payload: Vec<u8> = msg.get_payload().unwrap();
            if let Ok(MarketEvent::OrderTraded(_)) = bincode::deserialize(&payload) {
                trade_count_clone.fetch_add(1, Ordering::SeqCst);
            }
        }
        info!("Trade counter finished.");
    });

    // 4. Start sending orders from all traders concurrently.
    info!("--- Starting benchmark for {} seconds ---", args.duration_secs);
    let start_time = Instant::now();
    let mut handles = Vec::new();

    for trader in traders {
        let tcp_addr_clone = tcp_addr.clone();
        let handle = tokio::spawn(async move {
            let mut rng = StdRng::from_entropy();
            
            // Each trader gets one persistent connection
            let mut stream = match TcpStream::connect(tcp_addr_clone.as_str()).await {
                Ok(s) => s,
                Err(e) => {
                    error!("Trader failed to connect: {}", e);
                    return;
                }
            };

            while Instant::now() - start_time < test_duration {
                let order = Order {
                    order_id: OrderID(rng.gen()),
                    user_id: trader.user_id,
        market_id: MarketID(market_id),
                    side: trader.side,
                    price: dec!(100.0) + Decimal::from(rng.gen_range(-5..=5)),
        quantity: dec!(1.0),
        order_type: OrderType::Limit,
        timestamp: 0,
    };
                let message_bytes = bincode::serialize(&order).unwrap();
                let signature = trader.signing_key.sign(&message_bytes);
                let signed_order = SignedOrder { order, signature: Signature(signature) };

                if let Err(e) = send_order(&mut stream, &signed_order).await {
                    error!("Failed to send order: {}. Closing connection.", e);
                    break; // Exit the loop if the connection breaks
                }
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await?;
    }

    info!("--- Benchmark finished sending orders ---");
    
    // 6. Wait a moment for final trades to be processed.
    time::sleep(Duration::from_secs(3)).await;

    // 7. Calculate and print results.
    let total_trades = trade_count.load(Ordering::SeqCst);
    let elapsed_secs = start_time.elapsed().as_secs_f64();
    let tps = total_trades as f64 / elapsed_secs;

    println!("\n===========================");
    println!("BENCHMARK RESULTS:");
    println!("Test Duration:  {:.2} seconds", elapsed_secs);
    println!("Total Trades:   {}", total_trades);
    println!("Throughput:     {:.2} TPS", tps);
    println!("===========================");
    
    Ok(())
}
