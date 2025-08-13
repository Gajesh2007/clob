use settlement_plane::{run_settlement_plane, SettlementPlaneError};
use std::error::Error;
use configuration::Settings;
use tokio::signal;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let settings = Settings::load().expect("Failed to load configuration");
    println!("--- Settlement Plane Starting ---");
    println!("Press Ctrl+C to shut down.");

    tokio::select! {
        res = run_settlement_plane(settings) => {
            if let Err(e) = res {
                eprintln!("Settlement plane failed: {}", e);
            }
        }
        _ = signal::ctrl_c() => {
            println!("Ctrl+C received, shutting down.");
        }
    }

    Ok(())
}
