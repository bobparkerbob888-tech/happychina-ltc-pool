use crate::types::*;
use log::{debug, error, warn};
use reqwest::Client;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// JSON-RPC client for Bitcoin/Litecoin-family coin daemons
#[derive(Clone)]
pub struct RpcClient {
    url: String,
    user: String,
    password: String,
    client: Client,
    coin_symbol: String,
    request_id: std::sync::Arc<AtomicU64>,
}

impl RpcClient {
    /// Create a new RPC client for a coin daemon
    pub fn new(url: &str, user: &str, password: &str, coin_symbol: &str) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(4)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            url: url.to_string(),
            user: user.to_string(),
            password: password.to_string(),
            client,
            coin_symbol: coin_symbol.to_string(),
            request_id: std::sync::Arc::new(AtomicU64::new(1)),
        }
    }

    /// Make a generic JSON-RPC call
    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RpcClientError> {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);

        let request = RpcRequest {
            jsonrpc: "1.0",
            id,
            method: method.to_string(),
            params,
        };

        debug!(
            "[{}] RPC call: {} (id={})",
            self.coin_symbol, method, id
        );

        let response = self
            .client
            .post(&self.url)
            .basic_auth(&self.user, Some(&self.password))
            .json(&request)
            .send()
            .await
            .map_err(|e| RpcClientError::Network(format!("{}: {}", self.coin_symbol, e)))?;

        let status = response.status();
        if !status.is_success() && status.as_u16() != 500 {
            let body = response.text().await.unwrap_or_default();
            return Err(RpcClientError::Http(format!(
                "{}: HTTP {} - {}",
                self.coin_symbol, status, body
            )));
        }

        let rpc_response: RpcResponse = response
            .json()
            .await
            .map_err(|e| RpcClientError::Parse(format!("{}: {}", self.coin_symbol, e)))?;

        if let Some(err) = rpc_response.error {
            return Err(RpcClientError::Rpc {
                code: err.code,
                message: format!("{}: {}", self.coin_symbol, err.message),
            });
        }

        rpc_response
            .result
            .ok_or_else(|| RpcClientError::Parse(format!("{}: null result", self.coin_symbol)))
    }

    /// Get block template for mining (parent chain only)
    pub async fn get_block_template(&self) -> Result<BlockTemplate, RpcClientError> {
        let params = serde_json::json!([{
            "rules": ["segwit", "mweb"]
        }]);

        let result = self.call("getblocktemplate", params).await?;
        let template: BlockTemplate = serde_json::from_value(result)
            .map_err(|e| RpcClientError::Parse(format!("{}: {}", self.coin_symbol, e)))?;
        Ok(template)
    }

    /// Create aux block for merge mining (aux chains)
    pub async fn create_aux_block(&self, address: &str) -> Result<AuxBlock, RpcClientError> {
        let params = serde_json::json!([address]);
        let result = self.call("createauxblock", params).await?;
        let aux_block: AuxBlock = serde_json::from_value(result)
            .map_err(|e| RpcClientError::Parse(format!("{}: {}", self.coin_symbol, e)))?;
        Ok(aux_block)
    }

    /// Make a JSON-RPC call that may return null as a success indicator.
    pub async fn call_nullable(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<Option<serde_json::Value>, RpcClientError> {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);

        let request = RpcRequest {
            jsonrpc: "1.0",
            id,
            method: method.to_string(),
            params,
        };

        debug!(
            "[{}] RPC call: {} (id={})",
            self.coin_symbol, method, id
        );

        let response = self
            .client
            .post(&self.url)
            .basic_auth(&self.user, Some(&self.password))
            .json(&request)
            .send()
            .await
            .map_err(|e| RpcClientError::Network(format!("{}: {}", self.coin_symbol, e)))?;

        let status = response.status();
        if !status.is_success() && status.as_u16() != 500 {
            let body = response.text().await.unwrap_or_default();
            return Err(RpcClientError::Http(format!(
                "{}: HTTP {} - {}",
                self.coin_symbol, status, body
            )));
        }

        let rpc_response: RpcResponse = response
            .json()
            .await
            .map_err(|e| RpcClientError::Parse(format!("{}: {}", self.coin_symbol, e)))?;

        if let Some(err) = rpc_response.error {
            return Err(RpcClientError::Rpc {
                code: err.code,
                message: format!("{}: {}", self.coin_symbol, err.message),
            });
        }

        Ok(rpc_response.result)
    }

    /// Submit a solved parent block
    pub async fn submit_block(&self, block_hex: &str) -> Result<(), RpcClientError> {
        let params = serde_json::json!([block_hex]);
        let result = self.call_nullable("submitblock", params).await?;

        match result {
            None => Ok(()),
            Some(v) if v.is_null() => Ok(()),
            Some(v) => {
                if let Some(s) = v.as_str() {
                    if s.is_empty() || s == "null" {
                        Ok(())
                    } else if s == "inconclusive" || s == "duplicate" || s == "duplicate-inconclusive" {
                        warn!("[{}] submitblock returned: {} (may be duplicate)", self.coin_symbol, s);
                        Ok(())
                    } else {
                        warn!("[{}] submitblock returned: {}", self.coin_symbol, s);
                        Err(RpcClientError::Rpc {
                            code: -1,
                            message: format!("{}: submitblock: {}", self.coin_symbol, s),
                        })
                    }
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Submit an aux proof-of-work block.
    /// Tries submitauxblock first, falls back to getauxblock (2-param submit form).
    pub async fn submit_aux_block(
        &self,
        hash: &str,
        auxpow_hex: &str,
    ) -> Result<(), RpcClientError> {
        let params = serde_json::json!([hash, auxpow_hex]);

        // Try submitauxblock first
        match self.call_nullable("submitauxblock", params.clone()).await {
            Ok(result) => {
                match result {
                    None => return Ok(()),
                    Some(v) if v.is_null() => return Ok(()),
                    Some(v) if v.as_bool() == Some(true) => return Ok(()),
                    Some(v) => {
                        if v.as_bool() == Some(false) {
                            log::warn!(
                                "[{}] submitauxblock returned false, trying getauxblock...",
                                self.coin_symbol
                            );
                        } else {
                            let msg = format!("{}", v);
                            log::warn!(
                                "[{}] submitauxblock returned: {}, trying getauxblock...",
                                self.coin_symbol, msg
                            );
                        }
                    }
                }
            }
            Err(RpcClientError::Rpc { code, ref message }) if code == -32601 || code == -1 => {
                log::info!(
                    "[{}] submitauxblock not available (code={}, {}), trying getauxblock...",
                    self.coin_symbol, code, message
                );
            }
            Err(e) => {
                log::warn!(
                    "[{}] submitauxblock failed: {}, trying getauxblock...",
                    self.coin_symbol, e
                );
            }
        }

        // Fallback: try getauxblock with 2 params (Yiimp-style submit)
        let params2 = serde_json::json!([hash, auxpow_hex]);
        let result = self.call_nullable("getauxblock", params2).await?;

        match result {
            None => Ok(()),
            Some(v) if v.is_null() => Ok(()),
            Some(v) if v.as_bool() == Some(true) => Ok(()),
            Some(v) => {
                let msg = format!("{}", v);
                log::warn!(
                    "[{}] getauxblock (submit) returned: {}",
                    self.coin_symbol, msg
                );
                if v.as_bool() == Some(false) {
                    Err(RpcClientError::Rpc {
                        code: -1,
                        message: format!("{}: aux block rejected (returned false)", self.coin_symbol),
                    })
                } else {
                    Err(RpcClientError::Rpc {
                        code: -1,
                        message: format!("{}: getauxblock submit: {}", self.coin_symbol, msg),
                    })
                }
            }
        }
    }

    /// Get blockchain info (height, difficulty, etc.)
    pub async fn get_blockchain_info(&self) -> Result<BlockchainInfo, RpcClientError> {
        let result = self.call("getblockchaininfo", serde_json::json!([])).await?;
        let info: BlockchainInfo = serde_json::from_value(result)
            .map_err(|e| RpcClientError::Parse(format!("{}: {}", self.coin_symbol, e)))?;
        Ok(info)
    }

    /// Get mining info (difficulty, networkhashps)
    pub async fn get_mining_info(&self) -> Result<MiningInfo, RpcClientError> {
        let result = self.call("getmininginfo", serde_json::json!([])).await?;
        let info: MiningInfo = serde_json::from_value(result)
            .map_err(|e| RpcClientError::Parse(format!("{}: {}", self.coin_symbol, e)))?;
        Ok(info)
    }

    /// Validate an address and get its scriptPubKey
    pub async fn validate_address(
        &self,
        address: &str,
    ) -> Result<ValidateAddressResult, RpcClientError> {
        let params = serde_json::json!([address]);
        let result = self.call("validateaddress", params).await?;
        let info: ValidateAddressResult = serde_json::from_value(result)
            .map_err(|e| RpcClientError::Parse(format!("{}: {}", self.coin_symbol, e)))?;
        Ok(info)
    }

    /// Get a block by hash (for confirmation tracking)
    pub async fn get_block(&self, hash: &str) -> Result<BlockInfo, RpcClientError> {
        let params = serde_json::json!([hash]);
        let result = self.call("getblock", params).await?;
        let info: BlockInfo = serde_json::from_value(result)
            .map_err(|e| RpcClientError::Parse(format!("{}: {}", self.coin_symbol, e)))?;
        Ok(info)
    }

    /// Get a block hash by height
    pub async fn get_block_hash(&self, height: u64) -> Result<String, RpcClientError> {
        let params = serde_json::json!([height]);
        let result = self.call("getblockhash", params).await?;
        result
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                RpcClientError::Parse(format!(
                    "{}: getblockhash did not return string",
                    self.coin_symbol
                ))
            })
    }

    /// Send coins to an address (for withdrawals)
    pub async fn send_to_address(
        &self,
        address: &str,
        amount: f64,
    ) -> Result<String, RpcClientError> {
        let params = serde_json::json!([address, amount]);
        let result = self.call("sendtoaddress", params).await?;
        result
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                RpcClientError::Parse(format!("{}: sendtoaddress did not return txid", self.coin_symbol))
            })
    }

    /// Get wallet balance
    pub async fn get_balance(&self) -> Result<f64, RpcClientError> {
        let result = self.call("getbalance", serde_json::json!([])).await?;
        result
            .as_f64()
            .ok_or_else(|| {
                RpcClientError::Parse(format!("{}: getbalance did not return number", self.coin_symbol))
            })
    }

    /// Get current block count
    pub async fn get_block_count(&self) -> Result<u64, RpcClientError> {
        let result = self.call("getblockcount", serde_json::json!([])).await?;
        result
            .as_u64()
            .ok_or_else(|| {
                RpcClientError::Parse(format!(
                    "{}: getblockcount did not return number",
                    self.coin_symbol
                ))
            })
    }

    /// Get the best block hash
    pub async fn get_best_block_hash(&self) -> Result<String, RpcClientError> {
        let result = self
            .call("getbestblockhash", serde_json::json!([]))
            .await?;
        result
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                RpcClientError::Parse(format!(
                    "{}: getbestblockhash did not return string",
                    self.coin_symbol
                ))
            })
    }

    /// Get network hash rate
    pub async fn get_network_hashps(&self) -> Result<f64, RpcClientError> {
        let result = self
            .call("getnetworkhashps", serde_json::json!([]))
            .await?;
        result
            .as_f64()
            .ok_or_else(|| {
                RpcClientError::Parse(format!(
                    "{}: getnetworkhashps did not return number",
                    self.coin_symbol
                ))
            })
    }

    /// Get the coin symbol this client is configured for
    pub fn symbol(&self) -> &str {
        &self.coin_symbol
    }
}

/// RPC client errors
#[derive(Debug, thiserror::Error)]
pub enum RpcClientError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("RPC error (code {code}): {message}")]
    Rpc { code: i64, message: String },

    #[error("Parse error: {0}")]
    Parse(String),
}

/// Create RPC clients for all configured coins
pub fn create_rpc_clients(
    coins: &[crate::config::CoinConfig],
) -> std::collections::HashMap<String, RpcClient> {
    let mut clients = std::collections::HashMap::new();
    for coin in coins {
        let client = RpcClient::new(&coin.rpc_url, &coin.rpc_user, &coin.rpc_password, &coin.symbol);
        clients.insert(coin.symbol.clone(), client);
    }
    clients
}

/// Test connectivity to all configured coin daemons
pub async fn test_all_connections(
    clients: &std::collections::HashMap<String, RpcClient>,
) -> Vec<(String, Result<BlockchainInfo, RpcClientError>)> {
    let mut results = Vec::new();
    for (symbol, client) in clients {
        let result = client.get_blockchain_info().await;
        results.push((symbol.clone(), result));
    }
    results
}
