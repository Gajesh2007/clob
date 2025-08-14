use std::error::Error;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use common_types::{SignedOrder, UserID, Deposit, Transaction, AssetID};
use configuration::Settings;
use ed25519_dalek::{Verifier, VerifyingKey, SigningKey};
use dashmap::DashMap;
use thiserror::Error;
use redis::AsyncCommands;
use rand::rngs::OsRng;
use rand::RngCore;
use warp::Filter;
use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;
use tracing::{info, instrument};

#[derive(Error, Debug)]
pub enum IngressError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Bincode error: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("Signature verification failed")]
    Signature(#[from] ed25519_dalek::SignatureError),
    #[error("User public key not found for UserID: {0}")]
    PublicKeyNotFound(UserID),
}

type PublicKeyCache = Arc<DashMap<UserID, VerifyingKey>>;

#[instrument(skip_all, fields(peer_addr = %stream.peer_addr().unwrap()))]
async fn handle_connection(
    mut stream: TcpStream,
    public_key_cache: PublicKeyCache,
    mut redis_conn: redis::aio::MultiplexedConnection,
) -> Result<(), IngressError> {
    loop {
        // Use length-prefix framing to read multiple orders from one connection
        let len = match stream.read_u32().await {
            Ok(len) => len,
            // A "closed" error means the client gracefully disconnected.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };

        let mut buffer = vec![0; len as usize];
        stream.read_exact(&mut buffer).await?;

        let signed_order: SignedOrder = bincode::deserialize(&buffer)?;
        let order = signed_order.order;

        let public_key = public_key_cache.get(&order.user_id).ok_or(IngressError::PublicKeyNotFound(order.user_id))?;
        let message_bytes = bincode::serialize(&order)?;
        public_key.verify(&message_bytes, &signed_order.signature.0)?;
        
        let channel = format!("market:{}", order.market_id.0);
        let tx = Transaction::SignedOrder(signed_order);
        let payload = bincode::serialize(&tx)?;
        
        redis::pipe()
            .publish(channel, payload.clone())
            .rpush("execution_log", payload)
            .query_async(&mut redis_conn)
            .await?;
    }
    
    Ok(())
}

#[derive(Serialize)]
struct CreateUserResponse { user_id: UserID, private_key_hex: String }
#[derive(Deserialize)]
struct DepositRequest { user_id: UserID, asset_id: AssetID, amount: Decimal }

async fn run_http_server(
    public_key_cache: PublicKeyCache,
    mut redis_conn: redis::aio::MultiplexedConnection,
    addr: std::net::SocketAddr,
) {
    let create_user_route = warp::post()
        .and(warp::path("users"))
        .and(warp::any().map(move || public_key_cache.clone()))
        .map(|pks: PublicKeyCache| {
            let mut csprng = OsRng;
            let mut secret_bytes = [0u8; 32];
            csprng.fill_bytes(&mut secret_bytes);
            let signing_key = SigningKey::from_bytes(&secret_bytes);
            let verifying_key = (&signing_key).into();
            let user_id = UserID(rand::random());
            pks.insert(user_id, verifying_key);
            warp::reply::json(&CreateUserResponse { user_id, private_key_hex: hex::encode(signing_key.to_bytes()) })
        });

    let redis_conn_clone = redis_conn.clone();
    let deposit_route = warp::post()
        .and(warp::path("deposit"))
        .and(warp::any().map(move || redis_conn_clone.clone()))
        .and(warp::body::json())
        .and_then(|mut client: redis::aio::MultiplexedConnection, req: DepositRequest| async move {
            let deposit = Deposit { user_id: req.user_id, asset_id: req.asset_id, amount: req.amount };
            let tx = Transaction::Deposit(deposit);
            let payload = bincode::serialize(&tx).unwrap();

            redis::pipe()
                .publish("deposits", payload.clone())
                .rpush("execution_log", payload)
                .query_async(&mut client)
                .await
                .map_err(|_| warp::reject())?;

            Ok::<_, warp::Rejection>(warp::reply::with_status("Deposit processed", warp::http::StatusCode::OK))
        });

    warp::serve(create_user_route.or(deposit_route)).run(addr).await;
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let settings = Settings::load()?;
    let public_key_cache: PublicKeyCache = Arc::new(DashMap::new());
    
    info!(redis_addr = %settings.redis_addr, "Connecting to Redis");
    let redis_client = redis::Client::open(settings.redis_addr.as_str())?;
    let redis_conn = redis_client.get_multiplexed_async_connection().await?;

    let http_addr = settings.execution_plane.http_listen_addr.parse()?;
    tokio::spawn(run_http_server(public_key_cache.clone(), redis_conn.clone(), http_addr));

    let listener = TcpListener::bind(&settings.execution_plane.tcp_listen_addr).await?;
    info!("Ingress-Verifier listening on {}", &settings.execution_plane.tcp_listen_addr);

    loop {
        let (socket, _) = listener.accept().await?;
        let pk_clone = public_key_cache.clone();
        let redis_clone = redis_conn.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(socket, pk_clone, redis_clone).await {
                if e.to_string().contains("early eof") {
                    // This is a graceful disconnect, not an error.
                    tracing::debug!("Client disconnected gracefully.");
                } else {
                    tracing::error!("Connection error: {}", e);
                }
            }
        });
    }
}
