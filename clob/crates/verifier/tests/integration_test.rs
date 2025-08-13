use std::error::Error;
use tokio::time::{sleep, Duration};
use tokio::process::Command;
use std::process::Stdio;
use common_types::{Order, SignedOrder, Side, OrderType, OrderID, UserID, MarketID, Signature, AssetID};
use ed25519_dalek::{SigningKey, Signer};
use rust_decimal_macros::dec;
use configuration::Settings;
use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;
use redis::AsyncCommands;
use tokio::io::AsyncWriteExt;
use redis_test::{RedisServer, TestConfig};

#[derive(Deserialize)]
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

#[tokio::test]
async fn test_full_system_integration() -> Result<(), Box<dyn Error + Send + Sync>> {
    let server = RedisServer::new(TestConfig::default())?;
    let redis_addr = format!("redis://{}", server.addr());
    
    let mut settings = Settings::load().expect("Failed to load configuration");
    // Override the redis address for all components
    std::env::set_var("REDIS_ADDR", &redis_addr);

    // 1. Clean up from previous runs.
    let _ = tokio::fs::remove_file(&settings.verifier.checkpoint_file_path).await;

    // 2. Start the servers as background processes.
    let mut ingress_handle = Command::new("cargo")
        .args(&["run", "--release", "--bin", "ingress-verifier"])
        .env("REDIS_ADDR", &redis_addr)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut matcher_handle = Command::new("cargo")
        .args(&["run", "--release", "--bin", "market-matcher", "1"]) // Test with market 1
        .env("REDIS_ADDR", &redis_addr)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut settlement_handle = Command::new("cargo")
        .args(&["run", "--release", "--bin", "settlement-plane"])
        .env("REDIS_ADDR", &redis_addr)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    sleep(Duration::from_secs(3)).await; // Give servers time to start
    let http_client = reqwest::Client::new();

    // 3. Create and fund a new user via the API.
    let create_user_url = format!("http://{}/users", &settings.execution_plane.http_listen_addr);
    let resp = http_client.post(&create_user_url).send().await?.json::<CreateUserResponse>().await?;
    let user_id = resp.user_id;
    let private_key = hex::decode(resp.private_key_hex)?;
    
    let deposit_url = format!("http://{}/deposit", &settings.execution_plane.http_listen_addr);
    let deposit_req = DepositRequest { user_id, asset_id: AssetID(1), amount: dec!(10000) };
    http_client.post(&deposit_url).json(&deposit_req).send().await?.error_for_status()?;

    // 4. Send a transaction.
    let signing_key = SigningKey::from_bytes(&private_key.try_into().unwrap());
    let order = Order {
        order_id: OrderID(1), user_id, market_id: MarketID(1), side: Side::Buy,
        price: dec!(100.0), quantity: dec!(1.0), order_type: OrderType::Limit, timestamp: 0,
    };
    let message_bytes = bincode::serialize(&order)?;
    let signature = signing_key.sign(&message_bytes);
    let signed_order = SignedOrder { order, signature: Signature(signature) };
    let payload = bincode::serialize(&signed_order)?;
    tokio::net::TcpStream::connect(&settings.execution_plane.tcp_listen_addr).await?.write_all(&payload).await?;

    // 5. Wait for settlement.
    sleep(Duration::from_secs(settings.settlement_plane.checkpoint_interval_seconds + 1)).await;

    // 6. Run the verifier.
    let verifier_output = Command::new("cargo")
        .args(&["run", "--release", "--bin", "verifier"])
        .env("REDIS_ADDR", &redis_addr)
        .output()
        .await?;
    
    let output_str = String::from_utf8(verifier_output.stdout)?;
    assert!(output_str.contains("✅ SUCCESS"), "Verification failed! Output: {}", output_str);

    // 7. Clean up.
    ingress_handle.kill().await?;
    matcher_handle.kill().await?;
    settlement_handle.kill().await?;

    Ok(())
}
