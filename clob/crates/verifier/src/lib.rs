use std::collections::BTreeMap;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use dashmap::DashMap;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use common_types::{
    Account, UserID, AssetID, OrderBook, Hash, Transaction
};
use matching_engine::MatchingEngine;
use configuration::Settings;
use thiserror::Error;

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
}

pub mod merkle;
use merkle::MerkleTree;

#[derive(Deserialize)]
struct L1Checkpoint {
    state_root: String,
}

pub async fn run_verifier(settings: Settings) -> Result<bool, VerifierError> {
    let mut checkpoint_file = File::open(&settings.verifier.checkpoint_file_path).await?;
    let mut checkpoint_bytes = Vec::new();
    checkpoint_file.read_to_end(&mut checkpoint_bytes).await?;
    let official_checkpoint: L1Checkpoint = serde_json::from_slice(&checkpoint_bytes)?;

    let mut log_file = File::open(&settings.verifier.execution_log_path).await?;
    let mut log_buffer = Vec::new();
    log_file.read_to_end(&mut log_buffer).await?;

    let mut local_order_book = OrderBook::new();
    let local_account_cache: DashMap<UserID, Account> = DashMap::new();
    
    let mut reader = std::io::Cursor::new(&log_buffer);
    while reader.position() < log_buffer.len() as u64 {
        let tx_len = bincode::deserialize_from::<_, u32>(&mut reader)? as usize;
        let tx: Transaction = bincode::deserialize_from(&mut reader)?;

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
    for entry in local_account_cache.iter() {
        let mut hasher = Sha256::new();
        hasher.update(&bincode::serialize(entry.value())?);
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

    println!("Official State Root: 0x{}", official_checkpoint.state_root);
    println!("Local State Root:    0x{}", local_state_root_hex);

    Ok(local_state_root_hex == official_checkpoint.state_root)
}
