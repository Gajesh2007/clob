use std::error::Error;
use std::sync::Arc;
use tokio::net::UdpSocket;
use common_types::{Order, MarketID, OrderBook, MarketEvent, Account, UserID, AssetID, Transaction};
use matching_engine::MatchingEngine;
use configuration::Settings;
use thiserror::Error;
use dashmap::DashMap;
use std::collections::BTreeMap;
use futures_util::StreamExt;
use tracing;

#[derive(Error, Debug)]
pub enum MatcherError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Bincode error: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("Configuration error: {0}")]
    Config(#[from] config::ConfigError),
    #[error("User account not found for UserID: {0}")]
    UserNotFound(UserID),
}

type AccountCache = Arc<DashMap<UserID, Account>>;

async fn run_single_market(
    market_id: MarketID,
    settings: Settings,
    redis_client: redis::Client,
    account_cache: AccountCache,
) -> Result<(), MatcherError> {
    let mut order_book = OrderBook::new();
    let multicast_addr: std::net::SocketAddr = settings.multicast.addr.parse().unwrap();
    let udp_socket = UdpSocket::bind(&settings.multicast.bind_addr).await?;
    udp_socket.connect(multicast_addr).await?;

    let mut pubsub = redis_client.get_async_connection().await?.into_pubsub();
    let channel = format!("market:{}", market_id.0);
    pubsub.subscribe(&channel).await?;
    
    tracing::info!(market_id = %market_id.0, "Matching engine started for market");

    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        if let Ok(Transaction::SignedOrder(signed_order)) = bincode::deserialize(msg.get_payload_bytes()) {
            let order = signed_order.order;
            let user_account = account_cache.get(&order.user_id).ok_or(MatcherError::UserNotFound(order.user_id))?;
            let required_asset = if order.side == common_types::Side::Buy { AssetID(1) } else { AssetID(2) };
            let required_amount = if order.side == common_types::Side::Buy { order.price * order.quantity } else { order.quantity };
            let balance = user_account.balances.get(&required_asset).ok_or(MatcherError::UserNotFound(order.user_id))?;
            if *balance < required_amount {
                tracing::warn!(user_id = %order.user_id, "Insufficient funds");
                continue;
            }

            let events = order_book.process_order(order);
            for event in &events {
                if let MarketEvent::OrderTraded(trade) = event {
                    let mut maker_account = account_cache.get_mut(&trade.maker_user_id).unwrap();
                    let mut taker_account = account_cache.get_mut(&trade.taker_user_id).unwrap();
                    let asset1 = AssetID(1);
                    let asset2 = AssetID(2);
                    let total_price = trade.price * trade.quantity;
                    *taker_account.balances.get_mut(&asset1).unwrap() -= total_price;
                    *taker_account.balances.get_mut(&asset2).unwrap() += trade.quantity;
                    *maker_account.balances.get_mut(&asset1).unwrap() += total_price;
                    *maker_account.balances.get_mut(&asset2).unwrap() -= trade.quantity;
                }
                let event_bytes = bincode::serialize(&event)?;
                udp_socket.send(&event_bytes).await?;
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let settings = Settings::load()?;
    
    let redis_addr = std::env::var("REDIS_ADDR").unwrap_or_else(|_| "redis://127.0.0.1/".to_string());
    let redis_client = redis::Client::open(redis_addr)?;
    let account_cache: AccountCache = Arc::new(DashMap::new());

    let acc_cache_clone = account_cache.clone();
    let redis_client_clone = redis_client.clone();
    tokio::spawn(async move {
        let mut pubsub = redis_client_clone.get_async_connection().await.unwrap().into_pubsub();
        pubsub.subscribe("deposits").await.unwrap();
        let mut deposit_stream = pubsub.on_message();
        while let Some(msg) = deposit_stream.next().await {
            if let Ok(Transaction::Deposit(deposit)) = bincode::deserialize(msg.get_payload_bytes()) {
                acc_cache_clone.entry(deposit.user_id).or_insert_with(|| Account {
                    user_id: deposit.user_id,
                    balances: BTreeMap::new(),
                });
                let mut account = acc_cache_clone.get_mut(&deposit.user_id).unwrap();
                *account.balances.entry(deposit.asset_id).or_default() += deposit.amount;
                tracing::info!(user_id = %deposit.user_id, "Processed deposit");
            }
        }
    });

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: market-matcher <market_id_1> <market_id_2> ...");
        return Ok(());
    }

    let mut handles = vec![];
    for market_id_str in args.iter().skip(1) {
        let market_id = MarketID(market_id_str.parse()?);
        let handle = tokio::spawn(run_single_market(
            market_id,
            settings.clone(),
            redis_client.clone(),
            account_cache.clone(),
        ));
        handles.push(handle);
    }

    for handle in handles {
        handle.await??;
    }

    Ok(())
}