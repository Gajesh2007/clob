use verifier::{run_verifier, VerifierError};
use configuration::Settings;

#[tokio::main]
async fn main() -> Result<(), VerifierError> {
    let settings = Settings::load().expect("Failed to load configuration");
    println!("--- Independent Verifier Starting ---");
    match run_verifier(settings).await {
        Ok(true) => println!("✅ SUCCESS: Local state root matches the official checkpoint."),
        Ok(false) => println!("❌ FAILURE: Local state root does NOT match the official checkpoint!"),
        Err(e) => eprintln!("Verifier failed with an error: {}", e),
    }
    Ok(())
}