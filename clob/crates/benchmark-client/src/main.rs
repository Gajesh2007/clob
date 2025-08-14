use std::error::Error;
use std::net::{Ipv4Addr, SocketAddr};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::{Instant, Duration};
use tokio::io::AsyncWriteExt;
use common_types::{
    Order, SignedOrder, Side, OrderType, OrderID, UserID, MarketID, Signature, AssetID, MarketEvent
};
use ed25519_dalek::{SigningKey, Signer};
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;
use rand::Rng;
use redis::AsyncCommands;

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
    asset_id: u32,
    amount: Decimal,
) -> Result<(UserID, SigningKey), Box<dyn Error>> {
    // 1. Create user
    let create_user_url = format!("{}/users", http_addr);
    let resp = http_client.post(&create_user_url).send().await?.json::<CreateUserResponse>().await?;
    let user_id = resp.user_id;
    let private_key_bytes = hex::decode(resp.private_key_hex)?;
    let signing_key = SigningKey::from_bytes(&private_key_bytes.try_into().unwrap());
    println!("Created user {}.", user_id);

    // 2. Fund user
    let deposit_url = format!("{}/deposit", http_addr);
    let deposit_req = DepositRequest { user_id, asset_id: AssetID(asset_id), amount };
    http_client.post(&deposit_url).json(&deposit_req).send().await?.error_for_status()?;
    println!("Funded user {} with {} of asset {}.", user_id, amount, asset_id);
    
    Ok((user_id, signing_key))
}

async fn send_order(tcp_addr: &str, signed_order: &SignedOrder) -> Result<(), Box<dyn Error>> {
    let payload = bincode::serialize(signed_order)?;
    let mut stream = TcpStream::connect(tcp_addr).await?;
    stream.write_all(&payload).await?;
    stream.shutdown().await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let market_id: u32 = if args.len() > 1 { args[1].parse()? } else { 1 };

    let http_client = reqwest::Client::new();
    let ingress_http_addr = "http://127.0.0.1:9090";
    let ingress_tcp_addr = "127.0.0.1:8080";
    let redis_addr = "redis://127.0.0.1/";

    // 0. Clear the execution log from previous runs for a clean state.
    println!("--- Clearing previous state ---");
    let redis_client = redis::Client::open(redis_addr)?;
    let mut redis_conn = redis_client.get_async_connection().await?;
    let _: () = redis_conn.del("execution_log").await?;
    println!("State cleared.");

    // 1. Create and fund two users.
    println!("--- Setting up benchmark ---");
    let (maker_user_id, maker_signing_key) = create_and_fund_user(&http_client, ingress_http_addr, 2, dec!(100.0)).await?;
    let (taker_user_id, taker_signing_key) = create_and_fund_user(&http_client, ingress_http_addr, 1, dec!(10000.0)).await?;
    println!("--------------------------\n");

    // 2. Set up UDP listener for multicast.
    let multicast_addr = "239.0.0.1:9000".parse::<SocketAddr>()?;
    let bind_addr = "0.0.0.0:9000".parse::<SocketAddr>()?;
    let std_socket = std::net::UdpSocket::bind(bind_addr)?;
    if let SocketAddr::V4(addr) = multicast_addr {
        std_socket.join_multicast_v4(addr.ip(), &Ipv4Addr::UNSPECIFIED)?;
    } else {
        return Err("Multicast address must be IPv4".into());
    }
    let udp_socket = UdpSocket::from_std(std_socket)?;
    println!("Listening for market data on {}", multicast_addr);

    // 3. Maker places a SELL order.
    let maker_order = Order {
        order_id: OrderID(rand::thread_rng().gen()),
        user_id: maker_user_id,
        market_id: MarketID(market_id),
        side: Side::Sell,
        price: dec!(100.0),
        quantity: dec!(1.0),
        order_type: OrderType::Limit,
        timestamp: 0,
    };
    let maker_message_bytes = bincode::serialize(&maker_order)?;
    let maker_signature = maker_signing_key.sign(&maker_message_bytes);
    let maker_signed_order = SignedOrder { order: maker_order, signature: Signature(maker_signature) };
    
    println!("\nMaker submitting SELL order {:?}...", maker_order.order_id);
    send_order(ingress_tcp_addr, &maker_signed_order).await?;

    // 4. Wait for the maker's order to be placed on the book.
    let mut recv_buf = [0u8; 1024];
    println!("Waiting for order placement confirmation...");
    loop {
        let len = udp_socket.recv(&mut recv_buf).await?;
        if let Ok(MarketEvent::OrderPlaced { order_id, .. }) = bincode::deserialize(&recv_buf[..len]) {
            if order_id == maker_order.order_id {
                println!("Maker's order {:?} confirmed on book.", maker_order.order_id);
                break;
            }
        }
    }

    // 5. Taker places a matching BUY order.
    let taker_order = Order {
        order_id: OrderID(rand::thread_rng().gen()),
        user_id: taker_user_id,
        market_id: MarketID(market_id),
        side: Side::Buy,
        price: dec!(100.0),
        quantity: dec!(1.0),
        order_type: OrderType::Limit,
        timestamp: 0,
    };
    let taker_message_bytes = bincode::serialize(&taker_order)?;
    let taker_signature = taker_signing_key.sign(&taker_message_bytes);
    let taker_signed_order = SignedOrder { order: taker_order, signature: Signature(taker_signature) };

    println!("\nTaker submitting matching BUY order {:?}...", taker_order.order_id);
    
    // 6. Send the order and measure latency.
    let start_time = Instant::now();
    send_order(ingress_tcp_addr, &taker_signed_order).await?;

    println!("Waiting for trade confirmation...");
    loop {
        let len = tokio::time::timeout(Duration::from_secs(5), udp_socket.recv(&mut recv_buf)).await??;
        if let Ok(MarketEvent::OrderTraded(trade)) = bincode::deserialize(&recv_buf[..len]) {
            if trade.maker_order_id == maker_order.order_id && trade.taker_order_id == taker_order.order_id {
                let latency = Instant::now() - start_time;
                println!("\nReceived trade confirmation ({} bytes).\n", len);
                println!("===========================");
                println!("BENCHMARK RESULTS:");
                println!("Latency: {:?}", latency);
                println!("===========================");
                break;
            }
        }
    }
    
    Ok(())
}
