//! Settlement plane library.
//!
//! Responsibilities
//! - Accumulates transactions from Redis Pub/Sub (`market:*`, `deposits`)
//! - Periodically submits batches to a DA endpoint and receives a certificate
//! - Reconstructs state from the `execution_log` and writes a checkpoint file
//! - Publishes the latest checkpoint JSON to Redis key `checkpoint:latest`
//!
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use common_types::{Hash, Transaction, OrderBook, Account, UserID, AssetID};
use sha2::{Digest, Sha256};
use reqwest::Client;
use serde::{Serialize, Deserialize};
use serde_json;
use configuration::Settings;
use thiserror::Error;
use futures_util::StreamExt;
use matching_engine::MatchingEngine;
use std::collections::BTreeMap;
use dashmap::DashMap;
use redis::AsyncCommands;
use tracing::{info, error, debug};
use tokio::time::{sleep, Duration};

#[derive(Error, Debug)]
pub enum SettlementPlaneError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Bincode error: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("Verifier encountered a transaction for an unknown user: {0}")]
    UnknownUser(UserID),
}

pub mod merkle;
use merkle::MerkleTree;

#[derive(Serialize, Deserialize)]
struct L1Checkpoint {
    state_root: String,
    da_certificate: String,
}

/// Rebuild deterministic state (accounts and order book) from the execution log.
async fn reconstruct_state_from_log(redis_client: &redis::Client) -> Result<(DashMap<UserID, Account>, OrderBook), SettlementPlaneError> {
    // Fetch the append-only execution log which serves as the canonical DA
    let mut redis_conn = redis_client.get_multiplexed_async_connection().await?;
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
                    // In a real system, we might want to handle this more gracefully,
                    // but for this simulation, erroring out is fine.
                    return Err(SettlementPlaneError::UnknownUser(order.user_id));
                }
                // Replay deterministic matching to update balances
                let events = local_order_book.process_order(order);
                for event in &events {
                    if let common_types::MarketEvent::OrderTraded(trade) = event {
                        let mut maker_account = local_account_cache.get_mut(&trade.maker_user_id).unwrap();
                        let mut taker_account = local_account_cache.get_mut(&trade.taker_user_id).unwrap();
                        let asset1 = AssetID(1); // Assuming asset 1 is the quote currency
                        let asset2 = AssetID(2); // Assuming asset 2 is the base currency
                        let total_price = trade.price * trade.quantity;
                        
                        // Taker (buy) gives asset1, gets asset2; Maker (sell) does the inverse
                        *taker_account.balances.entry(asset1).or_default() -= total_price;
                        *taker_account.balances.entry(asset2).or_default() += trade.quantity;
                        *maker_account.balances.entry(asset1).or_default() += total_price;
                        *maker_account.balances.entry(asset2).or_default() -= trade.quantity;
                    }
                }
            }
        }
    }
    Ok((local_account_cache, local_order_book))
}


/// POST a batch of transaction bytes to the DA proxy and return the certificate.
async fn submit_batch_to_da(client: &Client, url: &str, batch: &[u8]) -> Result<Vec<u8>, SettlementPlaneError> {
    if batch.is_empty() {
        return Ok(Vec::new());
    }
    let response = client.post(url)
        .header("Content-Type", "application/octet-stream")
        .body(batch.to_vec())
        .send()
        .await?
        .error_for_status()?;
    let certificate = response.bytes().await?.to_vec();
    Ok(certificate)
}

/// Persist a JSON checkpoint containing the state root and DA certificate.
async fn submit_checkpoint_to_l1(file_path: &str, state_root: [u8; 32], da_certificate: &[u8]) -> Result<(), SettlementPlaneError> {
    let checkpoint = L1Checkpoint {
        state_root: hex::encode(state_root),
        da_certificate: hex::encode(da_certificate),
    };
    let mut file = File::create(file_path).await?;
    file.write_all(&serde_json::to_vec_pretty(&checkpoint)?).await?;
    Ok(())
}

/// Run the settlement plane event loop.
///
/// Collects Pub/Sub messages, submits periodic DA batches, reconstructs state
/// from the log, computes the Merkle root, writes `checkpoint.json`, and
/// publishes the latest checkpoint to Redis.
pub async fn run_settlement_plane(settings: Settings) -> Result<(), SettlementPlaneError> {
    info!("Running settlement plane");
    let http_client = Client::new();
    info!(redis_addr = %settings.redis_log_addr, "Connecting to Redis (log)");
    let redis_client = redis::Client::open(settings.redis_log_addr.as_str())?;
    let mut transaction_batch = Vec::new();
    let mut batch_count: u64 = 0;
    let mut checkpoint_timer = tokio::time::interval(tokio::time::Duration::from_secs(settings.settlement_plane.checkpoint_interval_seconds));

    loop {
        // Use standard async connection for PubSub; multiplexed connections don't support into_pubsub
        let mut pubsub = match redis_client.get_async_connection().await {
            Ok(conn) => conn.into_pubsub(),
            Err(e) => {
                error!(error = %e, "PubSub connection acquisition failed; retrying");
                sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        if let Err(e) = pubsub.psubscribe("market:*").await {
            error!(error = %e, "Failed to psubscribe market:*; retrying");
            sleep(Duration::from_millis(500)).await;
            continue;
        }
        if let Err(e) = pubsub.subscribe("deposits").await {
            error!(error = %e, "Failed to subscribe deposits; retrying");
            sleep(Duration::from_millis(500)).await;
            continue;
        }
        let mut message_stream = pubsub.on_message();
        info!("Subscribed to PubSub (market:* and deposits)");

        loop {
            tokio::select! {
                msg = message_stream.next() => {
                    match msg {
                        Some(msg) => {
                            let channel = msg.get_channel_name().to_string();
                            let payload: Vec<u8> = msg.get_payload()?;
                            transaction_batch.extend_from_slice(&payload);
                            batch_count += 1;
                            debug!(channel = %channel, batch_bytes = transaction_batch.len(), batch_count = batch_count, "Received PubSub message and queued for checkpoint");
                        }
                        None => {
                            error!("PubSub stream ended; reconnecting");
                            break;
                        }
                    }
                }
                _ = checkpoint_timer.tick() => {
                    if transaction_batch.is_empty() {
                        debug!("Checkpoint tick: no transactions accumulated; skipping");
                        continue;
                    }
                    let batch_to_submit = std::mem::take(&mut transaction_batch);
                    let count_to_submit = std::mem::replace(&mut batch_count, 0);

                    info!(bytes = batch_to_submit.len(), count = count_to_submit, url = %settings.settlement_plane.eigenda_proxy_url, "Submitting batch to DA");
                    match submit_batch_to_da(&http_client, &settings.settlement_plane.eigenda_proxy_url, &batch_to_submit).await {
                        Ok(da_certificate) => {
                            info!(certificate_bytes = da_certificate.len(), "DA accepted batch; reconstructing state and writing checkpoint");
                            let (accounts, order_book) = reconstruct_state_from_log(&redis_client).await?;
                            let mut leaves: Vec<Hash> = Vec::new();
                            // Deterministic ordering: sort accounts by UserID
                            let mut account_entries: Vec<(UserID, Account)> = accounts
                                .iter()
                                .map(|e| (*e.key(), e.value().clone()))
                                .collect();
                            account_entries.sort_by_key(|(user_id, _)| *user_id);
                            for (_user_id, account) in account_entries {
                                let mut hasher = Sha256::new();
                                hasher.update(&bincode::serialize(&account)?);
                                leaves.push(hasher.finalize().into());
                            }
                            for (_price, price_level) in &order_book.bids {
                                for order in price_level {
                                    let mut hasher = Sha256::new();
                                    hasher.update(&bincode::serialize(order)?);
                                    leaves.push(hasher.finalize().into());
                                }
                            }
                            for (_price, price_level) in &order_book.asks {
                                for order in price_level {
                                    let mut hasher = Sha256::new();
                                    hasher.update(&bincode::serialize(order)?);
                                    leaves.push(hasher.finalize().into());
                                }
                            }
                            let merkle_tree = MerkleTree::new(&leaves);
                            let state_root = merkle_tree.root();
                            let state_root_hex = hex::encode(state_root);
                            info!(leaves = leaves.len(), state_root = %format!("0x{}", state_root_hex), path = %settings.settlement_plane.checkpoint_file_path, "Writing checkpoint");
                            submit_checkpoint_to_l1(&settings.settlement_plane.checkpoint_file_path, state_root, &da_certificate).await?;
                            // Also publish checkpoint to Redis for external verifiers
                            let l1_checkpoint = L1Checkpoint { state_root: state_root_hex.clone(), da_certificate: hex::encode(&da_certificate) };
                            let checkpoint_json = serde_json::to_string_pretty(&l1_checkpoint)?;
                            let mut conn = redis_client.get_multiplexed_async_connection().await?;
                            let _: () = redis::cmd("SET").arg("checkpoint:latest").arg(&checkpoint_json).query_async(&mut conn).await?;
                            info!("Published latest checkpoint to Redis key 'checkpoint:latest'");
                        }
                        Err(e) => {
                            error!("Failed to process checkpoint: {}. Re-queuing batch.", e);
                            transaction_batch = batch_to_submit;
                            batch_count = count_to_submit;
                        }
                    }
                }
            }
        }
        // reconnect loop continues
        sleep(Duration::from_millis(500)).await;
    }
}
