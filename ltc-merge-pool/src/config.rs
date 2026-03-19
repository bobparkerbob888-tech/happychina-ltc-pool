use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub pool: PoolConfig,
    pub stratum: StratumConfig,
    pub database: DatabaseConfig,
    pub coins: Vec<CoinConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PoolConfig {
    pub name: String,
    pub fee_percent: f64,
    pub pplns_window: u64,
    pub block_confirmation_depth: u64,
    pub pool_address: String,
    #[serde(default = "default_api_port")]
    pub api_port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StratumConfig {
    pub ports: Vec<StratumPort>,
    pub listen_address: String,
    pub min_difficulty: f64,
    pub max_difficulty: f64,
    pub vardiff_target_time: f64,
    pub vardiff_retarget_shares: u32,
    pub connection_timeout_secs: u64,
    pub max_connections: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StratumPort {
    pub port: u16,
    pub difficulty: f64,
    pub vardiff: bool,
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CoinConfig {
    pub name: String,
    pub symbol: String,
    pub reward_address: Option<String>,
    pub rpc_url: String,
    pub rpc_user: String,
    pub rpc_password: String,
    pub is_parent: bool,
    pub block_reward: f64,
    pub confirmation_depth: u64,
    pub zmq_hashblock: Option<String>,
}

impl Config {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.coins.is_empty() {
            return Err("No coins configured".into());
        }

        let parent_count = self.coins.iter().filter(|c| c.is_parent).count();
        if parent_count != 1 {
            return Err(format!(
                "Expected exactly 1 parent coin, found {}",
                parent_count
            )
            .into());
        }

        if self.stratum.ports.is_empty() {
            return Err("No stratum ports configured".into());
        }

        if self.pool.fee_percent < 0.0 || self.pool.fee_percent > 100.0 {
            return Err("Fee percent must be between 0 and 100".into());
        }

        Ok(())
    }

    pub fn parent_coin(&self) -> &CoinConfig {
        self.coins
            .iter()
            .find(|c| c.is_parent)
            .expect("No parent coin configured (should have been caught by validate)")
    }

    pub fn aux_coins(&self) -> Vec<&CoinConfig> {
        self.coins.iter().filter(|c| !c.is_parent).collect()
    }

    pub fn coin_by_symbol(&self, symbol: &str) -> Option<&CoinConfig> {
        self.coins
            .iter()
            .find(|c| c.symbol.eq_ignore_ascii_case(symbol))
    }
}

fn default_api_port() -> u16 { 8090 }

