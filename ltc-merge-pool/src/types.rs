use serde::{Deserialize, Serialize};

/// Block template from getblocktemplate RPC
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BlockTemplate {
    pub version: i64,
    pub previousblockhash: String,
    pub transactions: Vec<TemplateTransaction>,
    pub coinbasevalue: u64,
    pub target: String,
    pub height: u64,
    pub bits: String,
    pub curtime: u64,
    #[serde(default)]
    pub mintime: u64,
    #[serde(default)]
    pub mutable: Vec<String>,
    #[serde(default)]
    pub noncerange: String,
    #[serde(default)]
    pub sigoplimit: i64,
    #[serde(default)]
    pub sizelimit: i64,
    #[serde(default)]
    pub weightlimit: i64,
    #[serde(default)]
    pub default_witness_commitment: Option<String>,
    #[serde(default)]
    pub mweb: Option<String>,
}

/// Transaction from block template
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TemplateTransaction {
    pub data: String,
    pub txid: String,
    pub hash: String,
    pub fee: i64,
    #[serde(default)]
    pub sigops: i64,
    #[serde(default)]
    pub weight: i64,
}

/// Aux block from createauxblock RPC
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuxBlock {
    pub hash: String,
    pub chainid: u32,
    #[serde(default)]
    pub previousblockhash: Option<String>,
    #[serde(default)]
    pub coinbasevalue: Option<u64>,
    pub bits: String,
    pub height: u64,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(rename = "_target", default)]
    pub underscore_target: Option<String>,
}

impl AuxBlock {
    /// Get the target hex string, checking both field names
    pub fn get_target(&self) -> Option<&str> {
        self.target
            .as_deref()
            .or(self.underscore_target.as_deref())
    }
}

/// Blockchain info from getblockchaininfo RPC
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BlockchainInfo {
    pub chain: String,
    pub blocks: u64,
    pub headers: u64,
    pub bestblockhash: String,
    pub difficulty: f64,
    #[serde(default)]
    pub mediantime: u64,
    #[serde(default)]
    pub pruned: bool,
}

/// Validate address response
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ValidateAddressResult {
    pub isvalid: bool,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(rename = "scriptPubKey", default)]
    pub script_pub_key: Option<String>,
    #[serde(default)]
    pub isscript: Option<bool>,
    #[serde(default)]
    pub iswitness: Option<bool>,
}

/// Get block response (for confirmation tracking)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BlockInfo {
    pub hash: String,
    pub confirmations: i64,
    pub height: u64,
    #[serde(default)]
    pub time: u64,
    #[serde(default)]
    pub nonce: u64,
    #[serde(default)]
    pub bits: String,
    #[serde(default)]
    pub difficulty: f64,
    #[serde(default)]
    pub previousblockhash: Option<String>,
    #[serde(default)]
    pub nextblockhash: Option<String>,
}

/// Mining info from getmininginfo RPC
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MiningInfo {
    pub blocks: u64,
    pub difficulty: f64,
    #[serde(default)]
    pub networkhashps: f64,
    #[serde(default)]
    pub pooledtx: u64,
    pub chain: String,
}

/// Wallet info
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WalletInfo {
    #[serde(default)]
    pub balance: f64,
    #[serde(default)]
    pub unconfirmed_balance: f64,
    #[serde(default)]
    pub immature_balance: f64,
}

/// JSON-RPC request structure
#[derive(Debug, Serialize)]
pub struct RpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    pub params: serde_json::Value,
}

/// JSON-RPC response structure
#[derive(Debug, Deserialize)]
pub struct RpcResponse {
    pub result: Option<serde_json::Value>,
    pub error: Option<RpcError>,
    pub id: Option<serde_json::Value>,
}

/// JSON-RPC error
#[derive(Debug, Deserialize, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RPC error {}: {}", self.code, self.message)
    }
}
