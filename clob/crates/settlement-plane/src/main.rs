use settlement_plane::run_settlement_plane;
use std::error::Error;
use configuration::Settings;
use tokio::signal;
use tracing::{info, error, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_writer(std::io::stdout)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let settings = Settings::load().expect("Failed to load configuration");
    info!("--- Settlement Plane Starting ---");
    info!("Press Ctrl+C to shut down.");

    tokio::select! {
        res = run_settlement_plane(settings) => {
            if let Err(e) = res {
                error!("Settlement plane failed: {}", e);
            }
        }
        _ = signal::ctrl_c() => {
            info!("Ctrl+C received, shutting down.");
        }
    }

    Ok(())
}
