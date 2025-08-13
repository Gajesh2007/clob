use std::error::Error;
use tokio::time::{sleep, Duration};
use tokio::net::TcpStream;
use tokio::io::AsyncWriteExt;
use common_types::{Order, SignedOrder, Side, OrderType, OrderID, UserID, MarketID, Signature, AssetID};
use ed25519_dalek::{SigningKey, Signer};
use rust_decimal_macros::dec;
use execution_plane::start_server;
use settlement_plane::run_settlement_plane;
use verifier::run_verifier;
use configuration::Settings;
use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;

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
    let settings = Settings::load().expect("Failed to load configuration");

    // 1. Clean up from previous runs.
    let _ = tokio::fs::remove_file(&settings.execution_plane.execution_log_path).await;
    let _ = tokio::fs::remove_file(&settings.verifier.checkpoint_file_path).await;

    // 2. Start the servers.
    let server_handles = start_server(settings.clone()).await?;
    let settlement_handle = tokio::spawn(run_settlement_plane(settings.clone()));

    sleep(Duration::from_secs(1)).await;
    let client = reqwest::Client::new();

    // 3. Create and fund a new user via the API.
    let create_user_url = format!("http://{}/users", &settings.execution_plane.http_listen_addr);
    let resp = client.post(&create_user_url).send().await?.json::<CreateUserResponse>().await?;
    let user_id = resp.user_id;
    let private_key = hex::decode(resp.private_key_hex)?;
    
    let deposit_url = format!("http://{}/deposit", &settings.execution_plane.http_listen_addr);
    let deposit_req_1 = DepositRequest {
        user_id,
        asset_id: AssetID(1),
        amount: dec!(10000),
    };
    client.post(&deposit_url).json(&deposit_req_1).send().await?.error_for_status()?;
    let deposit_req_2 = DepositRequest {
        user_id,
        asset_id: AssetID(2),
        amount: dec!(100),
    };
    client.post(&deposit_url).json(&deposit_req_2).send().await?.error_for_status()?;


    // 4. Act as the new client and send a transaction.
    let signing_key = SigningKey::from_bytes(&private_key.try_into().unwrap());
    let order = Order {
        order_id: OrderID(1),
        user_id,
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

    let mut stream = TcpStream::connect(&settings.execution_plane.tcp_listen_addr).await?;
    stream.write_all(&payload).await?;

    // 5. Wait for settlement to run.
    println!("Transaction sent. Waiting for settlement checkpoint...");
    sleep(Duration::from_secs(settings.settlement_plane.checkpoint_interval_seconds + 1)).await;

    // 6. Run the verifier.
    let verification_result = run_verifier(settings).await?;

    // 7. Assert success and clean up.
    assert!(verification_result, "Verification failed!");
    
    server_handles.tcp_task.abort();
    server_handles.http_task.abort();
    server_handles.engine_task.abort();
    server_handles.sequencer_task.abort();
    settlement_handle.abort();

    Ok(())
}