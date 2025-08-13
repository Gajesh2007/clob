use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::error::Error;
use common_types::{StateSnapshot, Order};
use sha2::{Digest, Sha256};
use reqwest::Client;
use serde::{Serialize, Deserialize};
use serde_json;

mod merkle;
use merkle::{MerkleTree, Hash};

const CHECKPOINT_INTERVAL_SECONDS: u64 = 5;
const EIGENDA_PROXY_URL: &str = "http://127.0.0.1:3100/put?commitment_mode=standard";
const STATE_SNAPSHOT_URL: &str = "http://127.0.0.1:9090/state";
const CHECKPOINT_FILE: &str = "checkpoint.json";

#[derive(Serialize, Deserialize)]
struct L1Checkpoint {
    state_root: String,
    da_certificate: String,
}

/// Fetches the current state snapshot from the execution plane.
async fn fetch_state_snapshot(client: &Client) -> Result<StateSnapshot, Box<dyn Error + Send + Sync>> {
    println!("Fetching state snapshot from execution plane...");
    let snapshot = client.get(STATE_SNAPSHOT_URL)
        .send()
        .await?
        .error_for_status()?
        .json::<StateSnapshot>()
        .await?;
    println!("Successfully fetched state snapshot.");
    Ok(snapshot)
}

/// Submits a batch of data to a Data Availability layer like EigenDA.
async fn submit_batch_to_da(client: &Client, batch: &[u8]) -> Result<Vec<u8>, reqwest::Error> {
    if batch.is_empty() {
        println!("No transaction data to submit to DA.");
        return Ok(Vec::new());
    }
    println!("Submitting batch of size {} bytes to EigenDA proxy...", batch.len());
    let response = client.post(EIGENDA_PROXY_URL)
        .header("Content-Type", "application/octet-stream")
        .body(batch.to_vec())
        .send()
        .await?;
    let response = response.error_for_status()?;
    let certificate = response.bytes().await?.to_vec();
    println!("Batch successfully submitted. Received DA certificate of size {} bytes.", certificate.len());
    Ok(certificate)
}

/// Simulates submitting a state checkpoint to the L1 settlement contract.
async fn submit_checkpoint_to_l1(state_root: [u8; 32], da_certificate: &[u8]) -> Result<(), Box<dyn Error + Send + Sync>> {
    let checkpoint = L1Checkpoint {
        state_root: hex::encode(state_root),
        da_certificate: hex::encode(da_certificate),
    };
    let mut file = File::create(CHECKPOINT_FILE).await?;
    file.write_all(&serde_json::to_vec_pretty(&checkpoint)?).await?;

    println!("\n--- L1 STATE SUBMISSION ---");
    println!("State Root: 0x{}", checkpoint.state_root);
    println!("DA Certificate (hex): 0x{}", checkpoint.da_certificate);
    println!("Wrote checkpoint to {}", CHECKPOINT_FILE);
    println!("--- L1 SUBMISSION COMPLETE ---\n");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    println!("Settlement Plane starting...");

    let mut log_file = File::open("execution.log").await?;
    let mut log_buffer = Vec::new();
    log_file.read_to_end(&mut log_buffer).await?;
    let mut log_cursor = log_buffer.len();
    println!("Opened execution.log and read {} existing bytes.", log_cursor);

    let http_client = Client::new();
    let mut checkpoint_timer = tokio::time::interval(tokio::time::Duration::from_secs(CHECKPOINT_INTERVAL_SECONDS));

    loop {
        checkpoint_timer.tick().await;

        log_file.read_to_end(&mut log_buffer).await?;
        let new_log_bytes = &log_buffer[log_cursor..];
        
        if new_log_bytes.is_empty() {
            println!("No new transactions; skipping checkpoint.");
            continue;
        }

        match submit_batch_to_da(&http_client, new_log_bytes).await {
            Ok(da_certificate) => {
                let snapshot = fetch_state_snapshot(&http_client).await?;

                let mut leaves: Vec<Hash> = Vec::new();
                for (_user_id, account) in &snapshot.accounts {
                    let mut hasher = Sha256::new();
                    hasher.update(&bincode::serialize(account)?);
                    leaves.push(hasher.finalize().into());
                }
                for (_price, price_level) in &snapshot.order_book.bids {
                    for order in price_level {
                        let mut hasher = Sha256::new();
                        hasher.update(&bincode::serialize(order)?);
                        leaves.push(hasher.finalize().into());
                    }
                }
                for (_price, price_level) in &snapshot.order_book.asks {
                    for order in price_level {
                        let mut hasher = Sha256::new();
                        hasher.update(&bincode::serialize(order)?);
                        leaves.push(hasher.finalize().into());
                    }
                }

                let merkle_tree = MerkleTree::new(&leaves);
                let state_root = merkle_tree.root();
                println!("Calculated new state root: 0x{}", hex::encode(state_root));

                submit_checkpoint_to_l1(state_root, &da_certificate).await?;

                log_cursor = log_buffer.len();
            }
            Err(e) => {
                eprintln!("Failed to submit batch to EigenDA: {}. Will retry on next cycle.", e);
            }
        }
    }
}