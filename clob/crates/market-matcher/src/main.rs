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
use common_types::{MarketID, OrderBook, MarketEvent, Account, UserID, AssetID, Transaction, should_sample, make_perf_event};
use matching_engine::MatchingEngine;
use configuration::Settings;
use thiserror::Error;
use dashmap::DashMap;
use tokio::sync::Mutex as AsyncMutex;
use std::collections::BTreeMap;
use futures_util::StreamExt;
use redis::AsyncCommands;
use tracing::{self, Level};
use tracing_subscriber::FmtSubscriber;
use tokio::time::{sleep, Duration};
use rust_decimal::Decimal;

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
type UserLockMap = Arc<DashMap<UserID, Arc<AsyncMutex<()>>>>;
type ReservedMap = Arc<DashMap<(UserID, AssetID), Decimal>>;

fn get_reserved_amount(reserved: &ReservedMap, user_id: UserID, asset_id: AssetID) -> Decimal {
    reserved.get(&(user_id, asset_id)).map(|v| *v).unwrap_or(Decimal::ZERO)
}

fn add_reserved(reserved: &ReservedMap, user_id: UserID, asset_id: AssetID, amount: Decimal) {
    let key = (user_id, asset_id);
    let mut entry = reserved.entry(key).or_insert(Decimal::ZERO);
    *entry += amount;
}

fn sub_reserved(reserved: &ReservedMap, user_id: UserID, asset_id: AssetID, amount: Decimal) {
    let key = (user_id, asset_id);
    let mut entry = reserved.entry(key).or_insert(Decimal::ZERO);
    *entry -= amount;
    if entry.is_sign_negative() { *entry = Decimal::ZERO; }
}

fn get_user_lock(locks: &UserLockMap, user_id: UserID) -> Arc<AsyncMutex<()>> {
    if let Some(lock) = locks.get(&user_id) { return lock.clone(); }
    let lock = Arc::new(AsyncMutex::new(()));
    let _ = locks.insert(user_id, lock.clone());
    lock
}

async fn run_single_market(
    market_id: MarketID,
    _settings: Settings,
    redis_client: redis::Client,
    account_cache: AccountCache,
    user_locks: UserLockMap,
    reserved: ReservedMap,
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
            tracing::debug!(order_id = %order.order_id.0, user_id = %order.user_id, side = ?order.side, price = %order.price, "Received order from Redis");
            if should_sample(order.order_id.0) {
                let ev = make_perf_event(order.order_id.0, order.market_id.0, 30);
                let mut c = redis_client.get_multiplexed_async_connection().await?;
                let _ : Result<(), _> = redis::cmd("RPUSH").arg("perf:events").arg(bincode::serialize(&ev).unwrap()).query_async(&mut c).await;
            }

            // Enforce available funds by reserving under user lock (drop before matching)
            {
                let taker_lock = get_user_lock(&user_locks, order.user_id);
                let _guard = taker_lock.lock().await;
                let required_asset = if order.side == common_types::Side::Buy { AssetID(1) } else { AssetID(2) };
                let required_amount = if order.side == common_types::Side::Buy { order.price * order.quantity } else { order.quantity };
                let current_balance = account_cache
                    .get(&order.user_id)
                    .and_then(|acc| acc.balances.get(&required_asset).copied())
                    .unwrap_or(Decimal::ZERO);
                let already_reserved = get_reserved_amount(&reserved, order.user_id, required_asset);
                let available = current_balance - already_reserved;
                if available < required_amount {
                    tracing::warn!(order_id = %order.order_id.0, user_id = %order.user_id, need = %required_amount, available = %available, asset = %required_asset.0, "Order rejected: insufficient available funds");
                    continue;
                }
                add_reserved(&reserved, order.user_id, required_asset, required_amount);
                tracing::debug!(order_id = %order.order_id.0, user_id = %order.user_id, reserved = %required_amount, asset = %required_asset.0, "Reserved available funds for order");
            }

            let events = order_book.process_order(order);
            if should_sample(order.order_id.0) {
                let ev = make_perf_event(order.order_id.0, order.market_id.0, 40);
                let mut c = redis_client.get_multiplexed_async_connection().await?;
                let _ : Result<(), _> = redis::cmd("RPUSH").arg("perf:events").arg(bincode::serialize(&ev).unwrap()).query_async(&mut c).await;
            }
            for event in &events {
                if let MarketEvent::OrderTraded(trade) = event {
                    tracing::debug!(trade_id = trade.trade_id.0, maker_order_id = trade.maker_order_id.0, taker_order_id = trade.taker_order_id.0, "Processed trade");

                    let asset1 = AssetID(1);
                    let asset2 = AssetID(2);
                    let total_price = trade.price * trade.quantity;

                    // Per-user locks to avoid shard deadlocks and ensure consistent updates
                    let taker_is_buy = matches!(order.side, common_types::Side::Buy);
                    if trade.maker_user_id == trade.taker_user_id {
                        let lock = get_user_lock(&user_locks, trade.maker_user_id);
                        let _g = lock.lock().await;
                        if let Some(mut account) = account_cache.get_mut(&trade.maker_user_id) {
                            if taker_is_buy {
                                // Buyer (taker) and seller (maker) are the same user
                                *account.balances.entry(asset1).or_default() -= total_price;
                                *account.balances.entry(asset2).or_default() += trade.quantity;
                                *account.balances.entry(asset1).or_default() += total_price;
                                *account.balances.entry(asset2).or_default() -= trade.quantity;
                            } else {
                                // Seller (taker) and buyer (maker) are the same user
                                *account.balances.entry(asset2).or_default() -= trade.quantity;
                                *account.balances.entry(asset1).or_default() += total_price;
                                *account.balances.entry(asset2).or_default() += trade.quantity;
                                *account.balances.entry(asset1).or_default() -= total_price;
                            }
                        } else {
                            tracing::error!(trade_id = trade.trade_id.0, user_id = %trade.maker_user_id, "CRITICAL: Account not found for a self-trade; skipping balance update");
                        }
                    } else {
                        // Lock acquisition in a stable order by user id to prevent deadlocks
                        let (first, second) = if trade.maker_user_id.0 < trade.taker_user_id.0 {
                            (trade.maker_user_id, trade.taker_user_id)
                        } else {
                            (trade.taker_user_id, trade.maker_user_id)
                        };
                        let first_lock = get_user_lock(&user_locks, first);
                        let second_lock = get_user_lock(&user_locks, second);
                        let _g1 = first_lock.lock().await;
                        let _g2 = second_lock.lock().await;
                        // Apply updates according to the taker's side
                        if taker_is_buy {
                            if let Some(mut taker_account) = account_cache.get_mut(&trade.taker_user_id) {
                                *taker_account.balances.entry(asset1).or_default() -= total_price;
                                *taker_account.balances.entry(asset2).or_default() += trade.quantity;
                            }
                            // Release reservations for executed portions
                            sub_reserved(&reserved, trade.taker_user_id, asset1, total_price);
                            sub_reserved(&reserved, trade.maker_user_id, asset2, trade.quantity);
                            if let Some(mut maker_account) = account_cache.get_mut(&trade.maker_user_id) {
                                *maker_account.balances.entry(asset1).or_default() += total_price;
                                *maker_account.balances.entry(asset2).or_default() -= trade.quantity;
                            }
                        } else {
                            if let Some(mut taker_account) = account_cache.get_mut(&trade.taker_user_id) {
                                *taker_account.balances.entry(asset2).or_default() -= trade.quantity;
                                *taker_account.balances.entry(asset1).or_default() += total_price;
                            }
                            sub_reserved(&reserved, trade.taker_user_id, asset2, trade.quantity);
                            sub_reserved(&reserved, trade.maker_user_id, asset1, total_price);
                            if let Some(mut maker_account) = account_cache.get_mut(&trade.maker_user_id) {
                                *maker_account.balances.entry(asset2).or_default() += trade.quantity;
                                *maker_account.balances.entry(asset1).or_default() -= total_price;
                            }
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
                if let common_types::MarketEvent::OrderTraded(trade) = event {
                    if should_sample(trade.taker_order_id.0) {
                        let ev = make_perf_event(trade.taker_order_id.0, trade.market_id.0, 50);
                        let mut c = redis_client.get_multiplexed_async_connection().await?;
                        let _ : Result<(), _> = redis::cmd("RPUSH").arg("perf:events").arg(bincode::serialize(&ev).unwrap()).query_async(&mut c).await;
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
    
    tracing::info!(redis_addr = %settings.redis_pubsub_addr, "Connecting to Redis (pubsub)");
    let redis_client = redis::Client::open(settings.redis_pubsub_addr.as_str())?;
    let account_cache: AccountCache = Arc::new(DashMap::new());
    let user_locks: UserLockMap = Arc::new(DashMap::new());
    let reserved: ReservedMap = Arc::new(DashMap::new());

    let acc_cache_clone = account_cache.clone();
    let user_locks_clone = user_locks.clone();
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
                    let lock = get_user_lock(&user_locks_clone, deposit.user_id);
                    let _g = lock.lock().await;
                    acc_cache_clone.entry(deposit.user_id).or_insert_with(|| Account { user_id: deposit.user_id, balances: BTreeMap::new() });
                    if let Some(mut account) = acc_cache_clone.get_mut(&deposit.user_id) {
                        *account.balances.entry(deposit.asset_id).or_default() += deposit.amount;
                    }
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
            user_locks.clone(),
            reserved.clone(),
        ));
        handles.push(handle);
    }

    for handle in handles {
        handle.await??;
    }

    Ok(())
}

