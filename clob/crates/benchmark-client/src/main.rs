use std::error::Error;
use std::net::{Ipv4Addr, SocketAddr};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::Instant;
use tokio::io::AsyncWriteExt;
use common_types::{Order, SignedOrder, Side, OrderType, OrderID, UserID, MarketID, Signature};
use ed25519_dalek::{SigningKey, Signer};
use rust_decimal_macros::dec;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 1. Get private key from command line arguments.
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: benchmark-client <private_key_hex>");
        return Ok(())
    }
    let private_key_bytes = hex::decode(&args[1])?;
    let signing_key = SigningKey::from_bytes(&private_key_bytes.try_into().unwrap());

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

    // 3. Create and sign a sample order.
    let order = Order {
        order_id: OrderID(999), // A sample ID
        user_id: UserID(1),     // The demo user ID
        market_id: MarketID(1),
        side: Side::Buy,
        price: dec!(100.0),
        quantity: dec!(1.0),
        order_type: OrderType::Limit,
        timestamp: 0,
    };
    let message_bytes = bincode::serialize(&order)?;
    let signature = signing_key.sign(&message_bytes);
    let signed_order = SignedOrder { order, signature: Signature(signature) };
    let payload = bincode::serialize(&signed_order)?;

    // 4. Connect to the execution plane.
    let server_addr = "127.0.0.1:8080";
    let mut stream = TcpStream::connect(server_addr).await?;
    println!("Connected to execution plane at {}", server_addr);

    // 5. Send the order and measure latency.
    println!("\nSending order and measuring round-trip latency...");
    let start_time = Instant::now();
    
    stream.write_all(&payload).await?;

    let mut recv_buf = [0u8; 1024];
    let len = udp_socket.recv(&mut recv_buf).await?;
    
    let end_time = Instant::now();
    let latency = end_time - start_time;

    println!("Received market data event ({} bytes).", len);
    println!("✅ End-to-end latency: {:?}", latency);

    Ok(())
}
