use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};
use tokio::fs::File;
use tokio::task::{JoinHandle, JoinError};
use std::net::SocketAddr;
use std::sync::Arc;
use std::convert::Infallible;
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicU64, Ordering};
use common_types::{
    Order, SignedOrder, UserID, AssetID, MarketEvent, StateSnapshot, OrderBook, Transaction, Deposit
};
use matching_engine::MatchingEngine;
use ed25519_dalek::{Verifier, VerifyingKey};
use dashmap::DashMap;
use warp::{Filter, Rejection, Reply};
use warp::http::StatusCode;
use configuration::Settings;
use thiserror::Error;
use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;
use std::error::Error;
use tracing::{info, warn, error, instrument};

pub mod user_management;
use user_management::UserManager;

#[derive(Error, Debug)]
pub enum ExecutionPlaneError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Bincode error: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("Network address parsing error: {0}")]
    AddrParse(#[from] std::net::AddrParseError),
    #[error("User account not found for UserID: {0}")]
    UserNotFound(UserID),
    #[error("User public key not found for UserID: {0}")]
    PublicKeyNotFound(UserID),
    #[error("Insufficient funds for UserID: {0}")]
    InsufficientFunds(UserID),
    #[error("Signature verification failed")]
    Signature(#[from] ed25519_dalek::SignatureError),
    #[error("Channel send error")]
    ChannelSend,
    #[error("Task join error: {0}")]
    Join(#[from] JoinError),
}

pub type AccountCache = Arc<DashMap<UserID, common_types::Account>>;
pub type PublicKeyCache = Arc<DashMap<UserID, VerifyingKey>>;
pub type SharedOrderBook = Arc<Mutex<OrderBook>>;

pub struct ServerHandles {
    pub tcp_task: JoinHandle<()>,
    pub http_task: JoinHandle<()>,
    pub engine_task: JoinHandle<Result<(), ExecutionPlaneError>>,
    pub sequencer_task: JoinHandle<Result<(), ExecutionPlaneError>>,
    pub metrics_task: JoinHandle<()>,
}

#[instrument(skip_all, fields(peer_addr = %stream.peer_addr().unwrap()))]
async fn handle_connection(
    mut stream: TcpStream,
    transaction_sender: mpsc::Sender<Transaction>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer).await?;
    let signed_order: SignedOrder = bincode::deserialize(&buffer)?;
    info!(order_id = %signed_order.order.order_id.0, user_id = %signed_order.order.user_id, "Received new order");
    transaction_sender.send(Transaction::SignedOrder(signed_order)).await?;
    Ok(())
}

#[instrument(skip_all)]
async fn run_sequencer_task(
    mut transaction_receiver: mpsc::Receiver<Transaction>,
    order_sender: mpsc::Sender<Order>,
    account_cache: AccountCache,
    public_key_cache: PublicKeyCache,
    log_path: String,
    tx_counter: Arc<AtomicU64>,
) -> Result<(), ExecutionPlaneError> {
    let mut log_file = File::create(&log_path).await?;

    while let Some(transaction) = transaction_receiver.recv().await {
        tx_counter.fetch_add(1, Ordering::Relaxed);
        let tx_bytes = bincode::serialize(&transaction)?;
        log_file.write_u32(tx_bytes.len() as u32).await?;
        log_file.write_all(&tx_bytes).await?;

        match transaction {
            Transaction::Deposit(deposit) => {
                info!(user_id = %deposit.user_id, asset_id = %deposit.asset_id.0, amount = %deposit.amount, "Processing deposit");
                let mut account = account_cache.get_mut(&deposit.user_id).ok_or(ExecutionPlaneError::UserNotFound(deposit.user_id))?;
                *account.balances.entry(deposit.asset_id).or_default() += deposit.amount;
            }
            Transaction::SignedOrder(signed_order) => {
                let order = signed_order.order;
                info!(order_id = %order.order_id.0, user_id = %order.user_id, "Processing order");
                let user_account = account_cache.get(&order.user_id).ok_or(ExecutionPlaneError::UserNotFound(order.user_id))?;
                let required_asset = if order.side == common_types::Side::Buy { AssetID(1) } else { AssetID(2) };
                let required_amount = if order.side == common_types::Side::Buy { order.price * order.quantity } else { order.quantity };
                let balance = user_account.balances.get(&required_asset).ok_or(ExecutionPlaneError::InsufficientFunds(order.user_id))?;
                if *balance < required_amount {
                    warn!(user_id = %order.user_id, "Insufficient funds");
                    continue;
                }

                let public_key = public_key_cache.get(&order.user_id).ok_or(ExecutionPlaneError::PublicKeyNotFound(order.user_id))?;
                let message_bytes = bincode::serialize(&order)?;
                if let Err(e) = public_key.verify(&message_bytes, &signed_order.signature.0) {
                    warn!(user_id = %order.user_id, "Signature verification failed: {}", e);
                    continue;
                }
                
                order_sender.send(order).await.map_err(|_| ExecutionPlaneError::ChannelSend)?;
            }
        }
    }
    Ok(())
}

#[instrument(skip_all)]
async fn run_matching_engine(
    mut order_receiver: mpsc::Receiver<Order>,
    account_cache: AccountCache,
    order_book: SharedOrderBook,
    settings: configuration::Multicast,
) -> Result<(), ExecutionPlaneError> {
    let multicast_addr = settings.addr.parse::<SocketAddr>()?;
    let udp_socket = UdpSocket::bind(&settings.bind_addr).await?;
    udp_socket.connect(multicast_addr).await?;
    
    while let Some(order) = order_receiver.recv().await {
        let mut book = order_book.lock().await;
        let events = book.process_order(order);
        drop(book);

        for event in &events {
            if let MarketEvent::OrderTraded(trade) = event {
                info!(trade_id = %trade.trade_id.0, "Processing trade");
                let mut maker_account = account_cache.get_mut(&trade.maker_user_id).ok_or(ExecutionPlaneError::UserNotFound(trade.maker_user_id))?;
                let mut taker_account = account_cache.get_mut(&trade.taker_user_id).ok_or(ExecutionPlaneError::UserNotFound(trade.taker_user_id))?;
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
    Ok(())
}

#[derive(Serialize)]
struct CreateUserResponse { user_id: UserID, private_key_hex: String }
#[derive(Deserialize)]
struct DepositRequest { user_id: UserID, asset_id: AssetID, amount: Decimal }

#[instrument(skip_all)]
async fn run_http_server(
    transaction_sender: mpsc::Sender<Transaction>,
    user_manager: Arc<UserManager>,
    account_cache: AccountCache,
    order_book: SharedOrderBook,
    addr: SocketAddr,
) {
    let state_route = warp::path("state")
        .and(warp::any().map(move || account_cache.clone()))
        .and(warp::any().map(move || order_book.clone()))
        .and_then(|accounts: AccountCache, book: SharedOrderBook| async move {
            let book_lock = book.lock().await;
            let snapshot = StateSnapshot {
                accounts: accounts.iter().map(|entry| (entry.key().clone(), entry.value().clone())).collect(),
                order_book: book_lock.clone(),
            };
            Ok::<_, Infallible>(warp::reply::json(&snapshot))
        });

    let create_user_route = warp::post()
        .and(warp::path("users"))
        .and(warp::any().map(move || user_manager.clone()))
        .map(|manager: Arc<UserManager>| {
            let (user_id, private_key) = manager.create_user();
            info!(user_id = %user_id, "Created new user");
            warp::reply::json(&CreateUserResponse { user_id, private_key_hex: hex::encode(private_key) })
        });

    let deposit_route = warp::post()
        .and(warp::path("deposit"))
        .and(warp::any().map(move || transaction_sender.clone()))
        .and(warp::body::json())
        .and_then(|sender: mpsc::Sender<Transaction>, req: DepositRequest| async move {
            let deposit = Deposit { user_id: req.user_id, asset_id: req.asset_id, amount: req.amount };
            sender.send(Transaction::Deposit(deposit)).await.map_err(|_| warp::reject())?;
            Ok::<_, Rejection>(warp::reply::with_status("Deposit processed", StatusCode::OK))
        });

    let routes = state_route.or(create_user_route).or(deposit_route);
    warp::serve(routes).run(addr).await;
}

async fn run_metrics_task(tx_counter: Arc<AtomicU64>, interval_seconds: u64) {
    let mut interval = tokio::time::interval(Duration::from_secs(interval_seconds));
    let mut last_count = 0;
    loop {
        interval.tick().await;
        let current_count = tx_counter.load(Ordering::Relaxed);
        let tx_since_last = current_count - last_count;
        let tps = tx_since_last as f64 / interval_seconds as f64;
        info!(tps = tps, "System Metrics");
        last_count = current_count;
    }
}

pub async fn start_server(settings: Settings) -> Result<ServerHandles, ExecutionPlaneError> {
    let public_key_cache: PublicKeyCache = Arc::new(DashMap::new());
    let account_cache: AccountCache = Arc::new(DashMap::new());
    let user_manager = Arc::new(UserManager::new(account_cache.clone(), public_key_cache.clone()));

    let order_book = Arc::new(Mutex::new(OrderBook::new()));
    let (transaction_sender, transaction_receiver) = mpsc::channel(1024);
    let (order_sender, order_receiver) = mpsc::channel(1024);
    
    let tx_counter = Arc::new(AtomicU64::new(0));
    let sequencer_task = tokio::spawn(run_sequencer_task(transaction_receiver, order_sender, account_cache.clone(), public_key_cache.clone(), settings.execution_plane.execution_log_path, tx_counter.clone()));
    let engine_task = tokio::spawn(run_matching_engine(order_receiver, account_cache.clone(), order_book.clone(), settings.multicast));
    
    let http_addr = settings.execution_plane.http_listen_addr.parse()?;
    let http_task = tokio::spawn(run_http_server(transaction_sender.clone(), user_manager, account_cache.clone(), order_book.clone(), http_addr));

    let listener = TcpListener::bind(&settings.execution_plane.tcp_listen_addr).await?;
    let tcp_task = tokio::spawn(async move {
        info!("TCP listener started");
        loop {
            match listener.accept().await {
                Ok((socket, _)) => {
                    tokio::spawn(handle_connection(socket, transaction_sender.clone()));
                }
                Err(e) => error!("Failed to accept new connection: {}", e),
            }
        }
    });

    let metrics_task = tokio::spawn(run_metrics_task(tx_counter, 5));

    Ok(ServerHandles { tcp_task, http_task, engine_task, sequencer_task, metrics_task })
}
