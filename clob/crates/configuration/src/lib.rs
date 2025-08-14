use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Settings {
    pub execution_plane: ExecutionPlane,
    pub settlement_plane: SettlementPlane,
    pub verifier: Verifier,
    pub multicast: Multicast,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExecutionPlane {
    pub tcp_listen_addr: String,
    pub http_listen_addr: String,
    pub execution_log_path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SettlementPlane {
    pub checkpoint_interval_seconds: u64,
    pub eigenda_proxy_url: String,
    pub checkpoint_file_path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Verifier {
    pub checkpoint_file_path: String,
    pub execution_log_path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Multicast {
    pub addr: String,
    pub bind_addr: String,
}

impl Settings {
    pub fn load() -> Result<Self, config::ConfigError> {
        let config = config::Config::builder()
            .set_default("execution_plane.tcp_listen_addr", "127.0.0.1:8080")?
            .set_default("execution_plane.http_listen_addr", "127.0.0.1:9090")?
            .set_default("execution_plane.execution_log_path", "execution.log")?
            .set_default("settlement_plane.checkpoint_interval_seconds", 5)?
            .set_default("settlement_plane.eigenda_proxy_url", "http://127.0.0.1:3100/put?commitment_mode=standard")?
            .set_default("settlement_plane.checkpoint_file_path", "checkpoint.json")?
            .set_default("verifier.checkpoint_file_path", "checkpoint.json")?
            .set_default("verifier.execution_log_path", "execution.log")?
            .set_default("multicast.addr", "239.0.0.1:9000")?
            .set_default("multicast.bind_addr", "0.0.0.0:0")?
            .add_source(config::File::with_name("config").required(false))
            .build()?;

        config.try_deserialize()
    }
}
