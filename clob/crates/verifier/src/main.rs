use std::error::Error;
use std::collections::BTreeMap;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use dashmap::DashMap;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use common_types::{
    Account, Order, UserID, AssetID, OrderBook, Hash
};
use matching_engine::MatchingEngine;

mod merkle;
use merkle::MerkleTree;

const CHECKPOINT_FILE: &str = "checkpoint.json";
const EXECUTION_LOG_FILE: &str = "execution.log";

#[derive(Deserialize)]
struct L1Checkpoint {
    state_root: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    println!("--- Independent Verifier Starting ---");

    // 1. Read the official checkpoint from the L1 (simulated).
    println!("Reading checkpoint from {}...", CHECKPOINT_FILE);
    let mut checkpoint_file = File::open(CHECKPOINT_FILE).await?;
    let mut checkpoint_bytes = Vec::new();
    checkpoint_file.read_to_end(&mut checkpoint_bytes).await?;
    let official_checkpoint: L1Checkpoint = serde_json::from_slice(&checkpoint_bytes)?;
    println!("Official State Root: 0x{}", official_checkpoint.state_root);

    // 2. Read the execution log from the DA layer (simulated).
    println!("Reading execution log from {}...", EXECUTION_LOG_FILE);
    let mut log_file = File::open(EXECUTION_LOG_FILE).await?;
    let mut log_buffer = Vec::new();
    log_file.read_to_end(&mut log_buffer).await?;
    println!("Read {} bytes from execution log.", log_buffer.len());

    // 3. Initialize a local, clean-room state.
    let mut local_order_book = OrderBook::new();
    let local_account_cache: DashMap<UserID, Account> = DashMap::new();

    // 4. Replay the entire execution log.
    println!("Replaying execution log...");
    let mut reader = std::io::Cursor::new(&log_buffer);
    while reader.position() < log_buffer.len() as u64 {
        let _order_len = bincode::deserialize_from::<_, u32>(&mut reader)?;
        let order: Order = bincode::deserialize_from(&mut reader)?;

        local_account_cache.entry(order.user_id).or_insert_with(|| Account {
            user_id: order.user_id,
            balances: BTreeMap::new(),
        });

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
    println!("Log replay complete.");

    // 5. Calculate the Merkle root of the final local state.
    println!("Calculating local state root...");
    let mut leaves: Vec<Hash> = Vec::new();
    for entry in local_account_cache.iter() {
        let mut hasher = Sha256::new();
        hasher.update(&bincode::serialize(entry.value()).unwrap());
        leaves.push(hasher.finalize().into());
    }
    for (_price, price_level) in &local_order_book.bids {
        for order in price_level {
            let mut hasher = Sha256::new();
            hasher.update(&bincode::serialize(order).unwrap());
            leaves.push(hasher.finalize().into());
        }
    }
    for (_price, price_level) in &local_order_book.asks {
        for order in price_level {
            let mut hasher = Sha256::new();
            hasher.update(&bincode::serialize(order).unwrap());
            leaves.push(hasher.finalize().into());
        }
    }

    let local_merkle_tree = MerkleTree::new(&leaves);
    let local_state_root = local_merkle_tree.root();
    let local_state_root_hex = hex::encode(local_state_root);
    println!("Local State Root:    0x{}", local_state_root_hex);

    // 6. Compare the roots.
    println!("\n--- VERIFICATION ---");
    if local_state_root_hex == official_checkpoint.state_root {
        println!("✅ SUCCESS: Local state root matches the official checkpoint.");
    } else {
        println!("❌ FAILURE: Local state root does NOT match the official checkpoint!");
    }
    println!("--------------------");

    Ok(())
}
