use execution_plane::{start_server, ExecutionPlaneError};
use configuration::Settings;
use tokio::signal;
use tracing_subscriber;

#[tokio::main]
async fn main() -> Result<(), ExecutionPlaneError> {
    tracing_subscriber::fmt::init();

    let settings = Settings::load().expect("Failed to load configuration");
    
    tracing::info!("--- Execution Plane Starting ---");
    
    let handles = start_server(settings).await?;

    tracing::info!("Execution Plane server listening...");
    tracing::info!("State snapshot server listening...");
    tracing::info!("User management API available.");
    tracing::info!("Press Ctrl+C to shut down.");

    tokio::select! {
        _ = signal::ctrl_c() => {
            tracing::info!("Ctrl+C received, shutting down.");
        }
        res = handles.tcp_task => {
            tracing::error!("TCP listener task exited unexpectedly: {:?}", res);
        }
        res = handles.http_task => {
            tracing::error!("HTTP server task exited unexpectedly: {:?}", res);
        }
        res = handles.engine_task => {
            tracing::error!("Matching engine task exited unexpectedly: {:?}", res);
        }
        res = handles.sequencer_task => {
            tracing::error!("Sequencer task exited unexpectedly: {:?}", res);
        }
    }

    Ok(())
}
