use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};
use tokio::fs::File;
use std::error::Error;
use std::net::SocketAddr;
use std::collections::BTreeMap;
use std::sync::Arc;
use common_types::{Account, Order, SignedOrder, UserID, AssetID, MarketEvent, StateSnapshot, OrderBook};
use matching_engine::MatchingEngine;
use ed25519_dalek::{SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use rand::RngCore;
use dashmap::DashMap;
use rust_decimal_macros::dec;
use hex;
use warp::Filter;
use serde::Serialize;

// A thread-safe error type for our tasks.
type TaskError = Box<dyn Error + Send + Sync>;

// The high-performance, concurrent, in-memory state for our exchange.
type AccountCache = Arc<DashMap<UserID, Account>>;
type PublicKeyCache = Arc<DashMap<UserID, VerifyingKey>>;
type SharedOrderBook = Arc<Mutex<OrderBook>>;
type LogFile = Arc<Mutex<File>>;

/// Handles an individual client connection.
async fn handle_connection(
    mut stream: TcpStream,
    order_sender: mpsc::Sender<Order>,
    account_cache: AccountCache,
    public_key_cache: PublicKeyCache,
    log_file: LogFile,
) -> Result<(), TaskError> {
    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer).await?;

    let signed_order: SignedOrder = bincode::deserialize(&buffer)?;
    let order = signed_order.order;
    
    // --- PRE-TRADE RISK CHECK ---
    let user_account = account_cache.get(&order.user_id).ok_or("User account not found")?;
    let required_asset = if order.side == common_types::Side::Buy { AssetID(1) } else { AssetID(2) };
    let required_amount = if order.side == common_types::Side::Buy { order.price * order.quantity } else { order.quantity };
    let balance = user_account.balances.get(&required_asset).ok_or("User has no balance for this asset")?;
    if *balance < required_amount {
        return Err(format!("Insufficient funds: balance={}, required={}", balance, required_amount).into());
    }
    println!("Risk check PASSED for user {:?}", order.user_id);

    // --- SIGNATURE VERIFICATION ---
    let public_key = public_key_cache.get(&order.user_id).ok_or("User public key not found")?;
    let message_bytes = bincode::serialize(&order)?;
    public_key.verify(&message_bytes, &signed_order.signature.0)?;
    println!("Signature VERIFIED for user {:?}", order.user_id);

    // --- APPEND TO EXECUTION LOG ---
    {
        let mut log = log_file.lock().await;
        let order_bytes = bincode::serialize(&order)?;
        log.write_u32(order_bytes.len() as u32).await?;
        log.write_all(&order_bytes).await?;
    }
    println!("Order {:?} written to execution log.", order.order_id);

    order_sender.send(order).await.map_err(|e| e.to_string())?;
    
    Ok(())
}

/// The main task for the matching engine.
async fn run_matching_engine(
    mut order_receiver: mpsc::Receiver<Order>,
    account_cache: AccountCache,
    order_book: SharedOrderBook,
) -> Result<(), TaskError> {
    let multicast_addr = "239.0.0.1:9000".parse::<SocketAddr>()?;
    let udp_socket = UdpSocket::bind("0.0.0.0:0").await?;
    udp_socket.connect(multicast_addr).await?;
    
    println!("Matching engine task started.");
    while let Some(order) = order_receiver.recv().await {
        let mut book = order_book.lock().await;
        let events = book.process_order(order);
        drop(book);

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
                
                println!("Updated balances for trade {}", trade.trade_id.0);
            }

            let event_bytes = bincode::serialize(&event)?;
            udp_socket.send(&event_bytes).await?;
        }
    }
    Ok(())
}

/// The main task for the state snapshot HTTP server.
async fn run_http_server(account_cache: AccountCache, order_book: SharedOrderBook) {
    let state_route = warp::path("state")
        .and(warp::any().map(move || account_cache.clone()))
        .and(warp::any().map(move || order_book.clone()))
        .and_then(|accounts: AccountCache, book: SharedOrderBook| async move {
            let book_lock = book.lock().await;
            let snapshot = StateSnapshot {
                accounts: accounts.iter().map(|entry| (entry.key().clone(), entry.value().clone())).collect(),
                order_book: book_lock.clone(),
            };
            Ok::<_, warp::Rejection>(warp::reply::json(&snapshot))
        });

    println!("State snapshot server listening on 127.0.0.1:9090");
    warp::serve(state_route).run(([127, 0, 0, 1], 9090)).await;
}

#[tokio::main]
async fn main() -> Result<(), TaskError> {
    // --- DEMO USER & ACCOUNT SETUP ---
    let user_id = UserID(1);
    let mut csprng = OsRng;
    let mut secret_bytes = [0u8; 32];
    csprng.fill_bytes(&mut secret_bytes);
    let signing_key = SigningKey::from_bytes(&secret_bytes);
    let verifying_key: VerifyingKey = (&signing_key).into();

    let public_key_cache: PublicKeyCache = Arc::new(DashMap::new());
    public_key_cache.insert(user_id, verifying_key);

    let account_cache: AccountCache = Arc::new(DashMap::new());
    let mut balances = BTreeMap::new();
    balances.insert(AssetID(1), dec!(1_000_000));
    balances.insert(AssetID(2), dec!(1000));
    let user_account = Account { user_id, balances };
    account_cache.insert(user_id, user_account);

    println!("--- DEMO USER SETUP ---");
    println!("User ID: {:?}", user_id.0);
    println!("Private Key (for client): {:?}", hex::encode(secret_bytes));
    println!("Initial Balances: {:?}", account_cache.get(&user_id).unwrap().balances);
    println!("-----------------------");

    // --- SHARED STATE & CHANNEL SETUP ---
    let log_file = Arc::new(Mutex::new(File::create("execution.log").await?));
    let order_book = Arc::new(Mutex::new(OrderBook::new()));
    let (order_sender, order_receiver) = mpsc::channel(1024);
    
    // --- START BACKGROUND TASKS ---
    let me_account_cache = account_cache.clone();
    let me_order_book = order_book.clone();
    tokio::spawn(async move {
        if let Err(e) = run_matching_engine(order_receiver, me_account_cache, me_order_book).await {
            eprintln!("Matching engine task failed: {}", e);
        }
    });

    let http_account_cache = account_cache.clone();
    let http_order_book = order_book.clone();
    tokio::spawn(run_http_server(http_account_cache, http_order_book));

    // --- START TCP LISTENER ---
    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    println!("Execution Plane server listening on 127.0.0.1:8080");

    loop {
        let (socket, _addr) = listener.accept().await?;
        let sender_clone = order_sender.clone();
        let acc_cache_clone = account_cache.clone();
        let pk_cache_clone = public_key_cache.clone();
        let log_clone = log_file.clone();
        
        tokio::spawn(async move {
            if let Err(e) = handle_connection(socket, sender_clone, acc_cache_clone, pk_cache_clone, log_clone).await {
                eprintln!("Error handling connection: {}", e);
            }
        });
    }
}
