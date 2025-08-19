//! Configuration loader for the CLOB system.
//!
//! This crate centralizes runtime settings for all services. It provides sane
//! defaults and supports overrides via an optional `config.toml` file and
//! environment variables prefixed with `CLOB_` (nested fields separated using
//! `__`). For example, `CLOB_EXECUTION_PLANE__TCP_LISTEN_ADDR=0.0.0.0:8080`.
//!
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
/// Top-level settings consumed by services.
pub struct Settings {
    pub redis_addr: String,
    pub execution_plane: ExecutionPlane,
    pub settlement_plane: SettlementPlane,
    pub verifier: Verifier,
    pub gateway: Gateway,
}

#[derive(Debug, Deserialize, Clone)]
/// TCP/HTTP endpoints and logging for the ingress plane.
pub struct ExecutionPlane {
    pub tcp_listen_addr: String,
    pub http_listen_addr: String,
    pub execution_log_path: String,
}

#[derive(Debug, Deserialize, Clone)]
/// Checkpointing cadence and DA endpoint for the settlement plane.
pub struct SettlementPlane {
    pub checkpoint_interval_seconds: u64,
    pub eigenda_proxy_url: String,
    pub checkpoint_file_path: String,
}

#[derive(Debug, Deserialize, Clone)]
/// Verifier settings for reading checkpoints and logs.
pub struct Verifier {
    pub checkpoint_file_path: String,
    pub execution_log_path: String,
}

#[derive(Debug, Deserialize, Clone)]
/// Market data gateway settings.
pub struct Gateway {
    pub http_listen_addr: String,
}

impl Settings {
    /// Load settings from defaults, `config.toml` (optional), and environment.
    pub fn load() -> Result<Self, config::ConfigError> {
        let config = config::Config::builder()
            .set_default("redis_addr", "redis://10.10.0.2:6379/")?
            .set_default("execution_plane.tcp_listen_addr", "0.0.0.0:8080")?
            .set_default("execution_plane.http_listen_addr", "0.0.0.0:9090")?
            .set_default("execution_plane.execution_log_path", "execution.log")?
            .set_default("settlement_plane.checkpoint_interval_seconds", 5)?
            .set_default("settlement_plane.eigenda_proxy_url", "http://10.10.0.3:3100/put?commitment_mode=standard")?
            .set_default("settlement_plane.checkpoint_file_path", "checkpoint.json")?
            .set_default("verifier.checkpoint_file_path", "checkpoint.json")?
            .set_default("verifier.execution_log_path", "execution.log")?
            .set_default("gateway.http_listen_addr", "127.0.0.1:9100")?
            .add_source(config::File::with_name("config").required(false))
            .add_source(
                config::Environment::with_prefix("CLOB")
                    .prefix_separator("_")
                    .separator("__"),
            )
            .build()?;

        config.try_deserialize()
    }
}
