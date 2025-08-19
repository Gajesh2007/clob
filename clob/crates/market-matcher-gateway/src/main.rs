use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::collections::HashSet;
use futures::{StreamExt, SinkExt};
use tokio::sync::broadcast;
use warp::Filter;
use configuration::Settings;
use tracing::{info, error};
use serde::Deserialize;
use common_types::MarketEvent;
use warp::http::StatusCode;

#[derive(Clone)]
struct OutEvent {
    market_id: u32,
    json: String,
    bytes: Arc<Vec<u8>>,
}

#[derive(Deserialize, Clone)]
struct ClientParams {
    market_id: Option<String>, // comma-separated list or single id
    format: Option<String>,    // "json" or "binary"
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let settings = Settings::load().expect("failed to load settings");
    let addr: SocketAddr = settings.gateway.http_listen_addr.parse().expect("invalid gateway addr");
    let redis_client = redis::Client::open(settings.redis_addr.as_str()).expect("invalid redis addr");

    // Fan-out hub: each market gets a broadcast channel of encoded MarketEvent frames
    // Channel size keeps a short buffer to avoid memory growth; slow clients will drop
    let (tx, _rx) = broadcast::channel::<Arc<OutEvent>>(1024);

    // Spawn Redis subscriber task
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        loop {
            let mut pubsub = match redis_client.get_async_connection().await {
                Ok(conn) => conn.into_pubsub(),
                Err(e) => {
                    error!(error = %e, "gateway: redis conn failed; retrying");
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
            };
            if let Err(e) = pubsub.psubscribe("market-events:*").await {
                error!(error = %e, "gateway: psubscribe failed; retrying");
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
            let mut stream = pubsub.on_message();
            info!("gateway: subscribed to market-events:* from Redis");
            while let Some(msg) = stream.next().await {
                let payload: Vec<u8> = match msg.get_payload() { Ok(p) => p, Err(_) => continue };
                let channel: String = msg.get_channel_name().to_string();
                let market_id = channel.split(':').nth(1).and_then(|s| s.parse::<u32>().ok());
                if market_id.is_none() { continue; }
                let event: MarketEvent = match bincode::deserialize(&payload) { Ok(e) => e, Err(_) => continue };
                let json = match serde_json::to_string(&event) { Ok(s) => s, Err(_) => continue };
                let out = OutEvent { market_id: market_id.unwrap(), json, bytes: Arc::new(payload) };
                let _ = tx_clone.send(Arc::new(out));
            }
            error!("gateway: pubsub stream ended; reconnecting");
        }
    });

    // SSE endpoint: /sse streams newline-delimited base64 frames
    let tx_sse = tx.clone();
    let sse_route = warp::path("sse")
        .and(warp::get())
        .and(warp::query::<ClientParams>())
        .map(move |params: ClientParams| {
            let mut rx = tx_sse.subscribe();
            let filter_set: Option<HashSet<u32>> = params.market_id.as_ref().map(|s| {
                s.split(',').filter_map(|p| p.trim().parse::<u32>().ok()).collect::<HashSet<u32>>()
            });
            let stream = async_stream::stream! {
                loop {
                    match rx.recv().await {
                        Ok(out) => {
                            if let Some(ref set) = filter_set {
                                if !set.contains(&out.market_id) { continue; }
                            }
                            // SSE is text; send JSON line
                            yield Ok::<_, Infallible>(warp::sse::Event::default().data(out.json.clone()));
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => { continue; }
                        Err(_) => break,
                    }
                }
            };
            warp::sse::reply(warp::sse::keep_alive().stream(stream))
        });

    // WebSocket endpoint: /ws relays binary frames
    let tx_ws = tx.clone();
    let ws_route = warp::path("ws")
        .and(warp::ws())
        .and(warp::query::<ClientParams>())
        .map(move |ws: warp::ws::Ws, params: ClientParams| {
            let tx_ws_inner = tx_ws.clone();
            ws.on_upgrade(move |websocket| async move {
                let (mut ws_tx, mut ws_rx) = websocket.split();
                let mut rx = tx_ws_inner.subscribe();
                let filter_set: Option<HashSet<u32>> = params.market_id.as_ref().map(|s| {
                    s.split(',').filter_map(|p| p.trim().parse::<u32>().ok()).collect::<HashSet<u32>>()
                });
                let send_json = matches!(params.format.as_deref(), Some("json"));
                // Forward broadcast to ws client
                let send_task = tokio::spawn(async move {
                    loop {
                        match rx.recv().await {
                            Ok(out) => {
                                if let Some(ref set) = filter_set {
                                    if !set.contains(&out.market_id) { continue; }
                                }
                                let msg = if send_json {
                                    warp::ws::Message::text(out.json.clone())
                                } else {
                                    warp::ws::Message::binary((&*out.bytes).clone())
                                };
                                if ws_tx.send(msg).await.is_err() { break; }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => { continue; }
                            Err(_) => break,
                        }
                    }
                });
                // Drain client messages (ignore)
                let _ = tokio::spawn(async move {
                    while let Some(_msg) = ws_rx.next().await {}
                });
                let _ = send_task.await;
            })
        });

    // Health endpoint
    let healthz = warp::path("healthz").and(warp::get()).map(|| warp::reply::with_status("ok", StatusCode::OK));

    let routes = healthz.or(sse_route).or(ws_route).with(warp::cors().allow_any_origin());
    info!(%addr, "gateway: listening");
    warp::serve(routes).run(addr).await;
}


