//! Market matcher service.
//!
//! Responsibilities
//! - Subscribes to Redis `market:{id}` channels and processes incoming orders
//! - Maintains an in-memory [`common_types::OrderBook`] per market
//! - Emits `MarketEvent::OrderTraded` and `OrderPlaced` events
//! - Publishes events to Redis channel `market-events:{id}` for real-time consumers
//! - Listens for `deposits` to maintain a local account cache for prelim checks
//!
use std::error::Error;
use std::sync::Arc;
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
use tokio::time::{sleep, Duration};

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

    // Create a separate Redis connection for publishing events
    let mut publisher_conn = redis_client.get_multiplexed_async_connection().await?;
    let events_channel = format!("market-events:{}", market_id.0);

    tracing::info!(market_id = %market_id.0, "Matching engine started for market");
    
    // Robust PubSub loop with auto-reconnect
    let channel = format!("market:{}", market_id.0);
    loop {
        let mut pubsub_conn = match redis_client.get_async_connection().await {
            Ok(conn) => conn.into_pubsub(),
            Err(e) => {
                tracing::error!(error = %e, "Failed to get Redis connection for PubSub; retrying");
                sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        if let Err(e) = pubsub_conn.subscribe(&channel).await {
            tracing::error!(error = %e, channel = %channel, "Failed to subscribe to market channel; retrying");
            sleep(Duration::from_millis(500)).await;
            continue;
        }

        tracing::info!(channel = %channel, "Subscribed to market channel");
        let mut stream = pubsub_conn.on_message();
        while let Some(msg) = stream.next().await {
        // Deserialize orders from Redis Pub/Sub and process through the engine
        if let Ok(Transaction::SignedOrder(signed_order)) = bincode::deserialize(msg.get_payload_bytes()) {
            let order = signed_order.order;
            tracing::info!(order_id = %order.order_id.0, user_id = %order.user_id, side = ?order.side, price = %order.price, "Received order from Redis");

            let user_account = match account_cache.get(&order.user_id) {
                Some(acc) => acc,
                None => {
                    // Soft reject if account is not present; deposit events should
                    // warm the cache before orders arrive.
                    tracing::warn!(order_id = %order.order_id.0, user_id = %order.user_id, "Order rejected: User account not found in cache.");
                    continue;
                }
            };
            
            let required_asset = if order.side == common_types::Side::Buy { AssetID(1) } else { AssetID(2) };
            let required_amount = if order.side == common_types::Side::Buy { order.price * order.quantity } else { order.quantity };
            
            let balance = user_account.balances.get(&required_asset).copied().unwrap_or_default();
            if balance < required_amount {
                // Guard against obvious insufficient funds based on cached balance
                tracing::warn!(order_id = %order.order_id.0, user_id = %order.user_id, asset_id = required_asset.0, required_amount = %required_amount, balance = %balance, "Order rejected: Insufficient funds");
                continue;
            }
            tracing::info!(order_id = %order.order_id.0, user_id = %order.user_id, "Sufficient funds found, processing order.");

            let events = order_book.process_order(order);
            for event in &events {
                if let MarketEvent::OrderTraded(trade) = event {
                    tracing::info!(trade_id = trade.trade_id.0, maker_order_id = trade.maker_order_id.0, taker_order_id = trade.taker_order_id.0, "Processed trade");

                    if trade.maker_user_id == trade.taker_user_id {
                        if let Some(mut account) = account_cache.get_mut(&trade.maker_user_id) {
                            // Self-trade: update the single account. The net effect
                            // on balances is zero, but we apply both sides to maintain
                            // logical consistency with the generated trade event.
                            let asset1 = AssetID(1);
                            let asset2 = AssetID(2);
                            let total_price = trade.price * trade.quantity;

                            // Taker side of the trade
                            *account.balances.entry(asset1).or_default() -= total_price;
                            *account.balances.entry(asset2).or_default() += trade.quantity;

                            // Maker side of the trade
                            *account.balances.entry(asset1).or_default() += total_price;
                            *account.balances.entry(asset2).or_default() -= trade.quantity;
                        } else {
                             tracing::error!(trade_id = trade.trade_id.0, "CRITICAL: Account not found for a self-trade. UserID: {}. Skipping balance update.", trade.maker_user_id);
                        }
                    } else {
                        // Different users, get both accounts
                        let maker_account_opt = account_cache.get_mut(&trade.maker_user_id);
                        let taker_account_opt = account_cache.get_mut(&trade.taker_user_id);

                        if maker_account_opt.is_some() && taker_account_opt.is_some() {
                            let mut maker_account = maker_account_opt.unwrap();
                            let mut taker_account = taker_account_opt.unwrap();

                            let asset1 = AssetID(1);
                            let asset2 = AssetID(2);
                            let total_price = trade.price * trade.quantity;
                            
                            *taker_account.balances.entry(asset1).or_default() -= total_price;
                            *taker_account.balances.entry(asset2).or_default() += trade.quantity;
                            *maker_account.balances.entry(asset1).or_default() += total_price;
                            *maker_account.balances.entry(asset2).or_default() -= trade.quantity;
                        } else {
                            tracing::error!(trade_id = trade.trade_id.0, "CRITICAL: Account not found for a processed trade. Maker found: {}, Taker found: {}. Skipping balance update.", maker_account_opt.is_some(), taker_account_opt.is_some());
                        }
                    }
                }
                let event_bytes = bincode::serialize(&event)?;
                // Publish to Redis with one reconnect-and-retry on failure
                if let Err(e) = redis::cmd("PUBLISH")
                    .arg(events_channel.as_str())
                    .arg(&event_bytes)
                    .query_async::<_, ()>(&mut publisher_conn)
                    .await
                {
                    tracing::error!(error = %e, "Redis publish failed; attempting to reconnect publisher and retry");
                    match redis_client.get_multiplexed_async_connection().await {
                        Ok(conn) => {
                            publisher_conn = conn;
                            if let Err(e2) = redis::cmd("PUBLISH")
                                .arg(events_channel.as_str())
                                .arg(&event_bytes)
                                .query_async::<_, ()>(&mut publisher_conn)
                                .await
                            {
                                tracing::error!(error = %e2, "Redis publish retry failed; dropping event");
                            }
                        }
                        Err(e2) => {
                            tracing::error!(error = %e2, "Failed to re-establish publisher connection; dropping event");
                        }
                    }
                }
            }
        }
        }
        tracing::warn!(channel = %channel, "PubSub stream ended; attempting to reconnect");
        sleep(Duration::from_millis(500)).await;
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
        // Maintain local account cache by consuming deposit broadcasts with auto-reconnect
        loop {
            let mut pubsub = match redis_client_clone.get_async_connection().await {
                Ok(conn) => conn.into_pubsub(),
                Err(e) => {
                    tracing::error!(error = %e, "Failed to get Redis connection for deposits PubSub; retrying");
                    sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };
            if let Err(e) = pubsub.subscribe("deposits").await {
                tracing::error!(error = %e, "Failed to subscribe to deposits; retrying");
                sleep(Duration::from_millis(500)).await;
                continue;
            }
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
            tracing::warn!("Deposits PubSub stream ended; attempting to reconnect");
            sleep(Duration::from_millis(500)).await;
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

