use std::error::Error;
use std::sync::Arc;
use tokio::net::UdpSocket;
use common_types::{MarketID, OrderBook, MarketEvent, Account, UserID, AssetID, Transaction};
use matching_engine::MatchingEngine;
use configuration::Settings;
use thiserror::Error;
use dashmap::DashMap;
use std::collections::BTreeMap;
use futures_util::StreamExt;
use redis::AsyncCommands;
use tracing::{self, Level};
use tracing_subscriber::FmtSubscriber;

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

    let mut pubsub_conn = redis_client.get_async_connection().await?.into_pubsub();
    let channel = format!("market:{}", market_id.0);
    pubsub_conn.subscribe(&channel).await?;
    
    // Create a separate Redis connection for publishing events
    let mut publisher_conn = redis_client.get_async_connection().await?;
    let events_channel = format!("market-events:{}", market_id.0);

    tracing::info!(market_id = %market_id.0, "Matching engine started for market");

    let mut stream = pubsub_conn.on_message();
    while let Some(msg) = stream.next().await {
        if let Ok(Transaction::SignedOrder(signed_order)) = bincode::deserialize(msg.get_payload_bytes()) {
            let order = signed_order.order;
            tracing::info!(user_id = %order.user_id, order_id = order.order_id.0, "Received order");

            let user_account = match account_cache.get(&order.user_id) {
                Some(acc) => acc,
                None => {
                    tracing::warn!(user_id = %order.user_id, "Order rejected: User account not found.");
                    continue;
                }
            };
            
            let required_asset = if order.side == common_types::Side::Buy { AssetID(1) } else { AssetID(2) };
            let required_amount = if order.side == common_types::Side::Buy { order.price * order.quantity } else { order.quantity };
            
            let balance = user_account.balances.get(&required_asset).copied().unwrap_or_default();
            if balance < required_amount {
                tracing::warn!(user_id = %order.user_id, "Insufficient funds for order");
                continue;
            }

            let events = order_book.process_order(order);
            for event in &events {
                if let MarketEvent::OrderTraded(trade) = event {
                    tracing::info!(trade_id = trade.trade_id.0, maker_order_id = trade.maker_order_id.0, taker_order_id = trade.taker_order_id.0, "Processed trade");
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
                // Publish to both UDP and Redis
                udp_socket.send(&event_bytes).await?;
                publisher_conn.publish(events_channel.as_str(), &event_bytes).await?;
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_writer(std::io::stdout)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let settings = Settings::load()?;
    
    tracing::info!(redis_addr = %settings.redis_addr, "Connecting to Redis");
    let redis_client = redis::Client::open(settings.redis_addr.as_str())?;
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

