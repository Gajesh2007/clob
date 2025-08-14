use std::collections::BTreeMap;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use reqwest::Client;
use dashmap::DashMap;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use common_types::{
    Account, UserID, AssetID, OrderBook, Hash, Transaction
};
use matching_engine::MatchingEngine;
use configuration::Settings;
use thiserror::Error;
use redis::AsyncCommands;
use tracing::info;

#[derive(Error, Debug)]
pub enum VerifierError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Bincode deserialization error: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("JSON deserialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Verifier encountered a transaction for an unknown user: {0}")]
    UnknownUser(UserID),
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),
}

pub mod merkle;
use merkle::MerkleTree;

#[derive(Deserialize)]
struct L1Checkpoint {
    state_root: String,
    #[serde(default)]
    da_certificate: Option<String>,
}

pub async fn run_verifier(settings: Settings) -> Result<bool, VerifierError> {
    // Try Redis first for the latest checkpoint, fall back to file if missing
    let redis_client = redis::Client::open("redis://127.0.0.1/")?;
    let mut redis_conn = redis_client.get_async_connection().await?;
    let checkpoint_json: Option<String> = redis::cmd("GET").arg("checkpoint:latest").query_async(&mut redis_conn).await?;
    let official_checkpoint: L1Checkpoint = if let Some(json) = checkpoint_json {
        serde_json::from_str(&json)?
    } else {
        let mut checkpoint_file = File::open(&settings.verifier.checkpoint_file_path).await?;
        let mut checkpoint_bytes = Vec::new();
        checkpoint_file.read_to_end(&mut checkpoint_bytes).await?;
        serde_json::from_slice(&checkpoint_bytes)?
    };

    // Best-effort: verify DA availability by fetching the blob via EigenDA proxy using the cert.
    if let Some(cert_hex) = official_checkpoint.da_certificate.as_ref() {
        let put_url = &settings.settlement_plane.eigenda_proxy_url;
        // Replace "/put" with "/get/<hex>" while keeping query params (e.g., commitment_mode)
        let get_url = put_url.replacen("/put", &format!("/get/{}", cert_hex), 1);
        if let Ok(client) = Client::builder().build() {
            match client.get(get_url.clone()).send().await {
                Ok(resp) if resp.status().is_success() => {
                    tracing::info!("Fetched blob from EigenDA proxy successfully for verification");
                }
                Ok(resp) => {
                    tracing::warn!(status = %resp.status(), "EigenDA proxy GET did not succeed; falling back to Redis-only verification");
                }
                Err(err) => {
                    tracing::warn!(error = %err, "EigenDA proxy GET failed; falling back to Redis-only verification");
                }
            }
        }
    } else {
        tracing::warn!("No DA certificate present in checkpoint; skipping EigenDA fetch and using Redis-only verification");
    }

    // The verifier now needs to fetch the log from the DA layer.
    // For this simulation, we assume the DA layer is Redis itself,
    // and we can read the log by fetching all messages from a list.
    let log_entries: Vec<Vec<u8>> = redis_conn.lrange("execution_log", 0, -1).await?;

    let mut local_order_book = OrderBook::new();
    let local_account_cache: DashMap<UserID, Account> = DashMap::new();
    
    for entry in log_entries {
        let tx: Transaction = bincode::deserialize(&entry)?;
        match tx {
            Transaction::Deposit(deposit) => {
                local_account_cache.entry(deposit.user_id).or_insert_with(|| Account {
                    user_id: deposit.user_id,
                    balances: BTreeMap::new(),
                });
                let mut account = local_account_cache.get_mut(&deposit.user_id).unwrap();
                *account.balances.entry(deposit.asset_id).or_default() += deposit.amount;
            }
            Transaction::SignedOrder(signed_order) => {
                let order = signed_order.order;
                if !local_account_cache.contains_key(&order.user_id) {
                    return Err(VerifierError::UnknownUser(order.user_id));
                }
                let events = local_order_book.process_order(order);
                for event in &events {
                    if let common_types::MarketEvent::OrderTraded(trade) = event {
                        let mut maker_account = local_account_cache.get_mut(&trade.maker_user_id).unwrap();
                        let mut taker_account = local_account_cache.get_mut(&trade.taker_user_id).unwrap();
                        let asset1 = AssetID(1);
                        let asset2 = AssetID(2);
                        let total_price = trade.price * trade.quantity;
                        *maker_account.balances.entry(asset1).or_default() += total_price;
                        *maker_account.balances.entry(asset2).or_default() -= trade.quantity;
                        *taker_account.balances.entry(asset1).or_default() -= total_price;
                        *taker_account.balances.entry(asset2).or_default() += trade.quantity;
                    }
                }
            }
        }
    }

    let mut leaves: Vec<Hash> = Vec::new();
    // Deterministic ordering: sort accounts by UserID to match settlement-plane
    let mut account_entries: Vec<(UserID, Account)> = local_account_cache
        .iter()
        .map(|e| (*e.key(), e.value().clone()))
        .collect();
    account_entries.sort_by_key(|(user_id, _)| *user_id);
    for (_user_id, account) in account_entries {
        let mut hasher = Sha256::new();
        hasher.update(&bincode::serialize(&account)?);
        leaves.push(hasher.finalize().into());
    }
    for (_price, price_level) in &local_order_book.bids {
        for order in price_level {
            let mut hasher = Sha256::new();
            hasher.update(&bincode::serialize(order)?);
            leaves.push(hasher.finalize().into());
        }
    }
    for (_price, price_level) in &local_order_book.asks {
        for order in price_level {
            let mut hasher = Sha256::new();
            hasher.update(&bincode::serialize(order)?);
            leaves.push(hasher.finalize().into());
        }
    }

    let local_merkle_tree = MerkleTree::new(&leaves);
    let local_state_root = local_merkle_tree.root();
    let local_state_root_hex = hex::encode(local_state_root);

    info!("Official State Root: 0x{}", official_checkpoint.state_root);
    info!("Local State Root:    0x{}", local_state_root_hex);

    Ok(local_state_root_hex == official_checkpoint.state_root)
}
