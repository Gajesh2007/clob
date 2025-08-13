use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use common_types::{StateSnapshot, Hash};
use sha2::{Digest, Sha256};
use reqwest::Client;
use serde::{Serialize, Deserialize};
use serde_json;
use configuration::Settings;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SettlementPlaneError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Bincode serialization error: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

pub mod merkle;
use merkle::MerkleTree;

#[derive(Serialize, Deserialize)]
struct L1Checkpoint {
    state_root: String,
    da_certificate: String,
}

async fn fetch_state_snapshot(client: &Client, url: &str) -> Result<StateSnapshot, SettlementPlaneError> {
    let snapshot = client.get(url).send().await?.error_for_status()?.json::<StateSnapshot>().await?;
    Ok(snapshot)
}

async fn submit_batch_to_da(client: &Client, url: &str, batch: &[u8]) -> Result<Vec<u8>, SettlementPlaneError> {
    if batch.is_empty() {
        return Ok(Vec::new());
    }
    println!("Submitting batch of size {} bytes to EigenDA proxy...", batch.len());
    let response = client.post(url)
        .header("Content-Type", "application/octet-stream")
        .body(batch.to_vec())
        .send()
        .await?
        .error_for_status()?;
    let certificate = response.bytes().await?.to_vec();
    println!("Batch successfully submitted. Received DA certificate of size {} bytes.", certificate.len());
    Ok(certificate)
}

async fn submit_checkpoint_to_l1(file_path: &str, state_root: [u8; 32], da_certificate: &[u8]) -> Result<(), SettlementPlaneError> {
    let checkpoint = L1Checkpoint {
        state_root: hex::encode(state_root),
        da_certificate: hex::encode(da_certificate),
    };
    let mut file = File::create(file_path).await?;
    file.write_all(&serde_json::to_vec_pretty(&checkpoint)?).await?;
    Ok(())
}

pub async fn run_settlement_plane(settings: Settings) -> Result<(), SettlementPlaneError> {
    let mut log_file = File::open(&settings.verifier.execution_log_path).await?;
    let mut log_buffer = Vec::new();
    log_file.read_to_end(&mut log_buffer).await?;
    let mut log_cursor = log_buffer.len();

    let http_client = Client::new();
    let mut checkpoint_timer = tokio::time::interval(tokio::time::Duration::from_secs(settings.settlement_plane.checkpoint_interval_seconds));

    loop {
        checkpoint_timer.tick().await;
        log_file.read_to_end(&mut log_buffer).await?;
        let new_log_bytes = &log_buffer[log_cursor..];
        
        match submit_batch_to_da(&http_client, &settings.settlement_plane.eigenda_proxy_url, new_log_bytes).await {
            Ok(da_certificate) => {
                let snapshot = fetch_state_snapshot(&http_client, &settings.settlement_plane.state_snapshot_url).await?;
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
                submit_checkpoint_to_l1(&settings.settlement_plane.checkpoint_file_path, state_root, &da_certificate).await?;
                log_cursor = log_buffer.len();
            }
            Err(e) => eprintln!("Failed to process checkpoint: {}. Will retry on next cycle.", e),
        }
    }
}
