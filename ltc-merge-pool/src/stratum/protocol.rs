/// Stratum JSON-RPC protocol parsing.
/// Handles both array params (standard) and object params (MRR/NiceHash compat).

use serde_json::Value;

/// A parsed stratum request from a client.
#[derive(Debug, Clone)]
pub struct StratumRequest {
    /// JSON-RPC id (can be number, string, or null)
    pub id: Value,
    /// Method name (e.g., "mining.subscribe")
    pub method: String,
    /// Parameters (normalized to array form)
    pub params: Vec<Value>,
}

/// Parse a single JSON line into a StratumRequest.
/// Returns None if the line is not a valid stratum request.
pub fn parse_request(line: &str) -> Option<StratumRequest> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let obj = v.as_object()?;

    let id = obj.get("id").cloned().unwrap_or(Value::Null);
    let method = obj
        .get("method")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())?;

    let params = match obj.get("params") {
        Some(Value::Array(arr)) => arr.clone(),
        Some(Value::Object(map)) => {
            // MRR/NiceHash sends object params for "login" method
            // Convert to array form: extract known fields
            normalize_object_params(&method, map)
        }
        Some(Value::Null) | None => Vec::new(),
        Some(other) => vec![other.clone()],
    };

    Some(StratumRequest { id, method, params })
}

/// Convert object params to array params for known methods.
fn normalize_object_params(
    method: &str,
    map: &serde_json::Map<String, Value>,
) -> Vec<Value> {
    match method {
        "login" => {
            // MRR sends: {"login": "address.worker", "pass": "x"}
            let login = map
                .get("login")
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let pass = map
                .get("pass")
                .cloned()
                .unwrap_or(Value::String("x".to_string()));
            vec![login, pass]
        }
        "mining.authorize" => {
            let user = map
                .get("user")
                .or_else(|| map.get("login"))
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let pass = map
                .get("pass")
                .or_else(|| map.get("password"))
                .cloned()
                .unwrap_or(Value::String("x".to_string()));
            vec![user, pass]
        }
        _ => {
            // Generic: return values in insertion order
            map.values().cloned().collect()
        }
    }
}

/// Build a JSON-RPC response (success).
pub fn response_ok(id: &Value, result: Value) -> String {
    serde_json::json!({
        "id": id,
        "result": result,
        "error": null
    })
    .to_string()
}

/// Build a JSON-RPC error response.
pub fn response_error(id: &Value, code: i64, message: &str) -> String {
    serde_json::json!({
        "id": id,
        "result": null,
        "error": [code, message, null]
    })
    .to_string()
}

/// Build a JSON-RPC notification (no id).
pub fn notification(method: &str, params: Value) -> String {
    serde_json::json!({
        "id": null,
        "method": method,
        "params": params
    })
    .to_string()
}

/// Build a mining.set_difficulty notification.
pub fn set_difficulty_notification(difficulty: f64) -> String {
    notification("mining.set_difficulty", serde_json::json!([difficulty]))
}

/// Build a mining.notify notification.
///
/// Params:
/// [job_id, prevhash, coinbase1, coinbase2, merkle_branches, version, nbits, ntime, clean_jobs]
pub fn mining_notify(
    job_id: &str,
    prevhash: &str,
    coinbase1: &str,
    coinbase2: &str,
    merkle_branches: &[String],
    version: &str,
    nbits: &str,
    ntime: &str,
    clean_jobs: bool,
) -> String {
    notification(
        "mining.notify",
        serde_json::json!([
            job_id,
            prevhash,
            coinbase1,
            coinbase2,
            merkle_branches,
            version,
            nbits,
            ntime,
            clean_jobs,
        ]),
    )
}

/// Parse miner.worker from a "user" param string.
/// Format: "address" or "address.worker_name"
pub fn parse_miner_worker(user: &str) -> (String, String) {
    if let Some(dot_pos) = user.find('.') {
        let miner = user[..dot_pos].to_string();
        let worker = user[dot_pos + 1..].to_string();
        if worker.is_empty() {
            (miner, "default".to_string())
        } else {
            (miner, worker)
        }
    } else {
        (user.to_string(), "default".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_subscribe() {
        let line = r#"{"id":1,"method":"mining.subscribe","params":["cgminer/4.11.1"]}"#;
        let req = parse_request(line).unwrap();
        assert_eq!(req.method, "mining.subscribe");
        assert_eq!(req.params.len(), 1);
    }

    #[test]
    fn test_parse_object_params_login() {
        let line = r#"{"id":1,"method":"login","params":{"login":"LTC_ADDR.rig1","pass":"x"}}"#;
        let req = parse_request(line).unwrap();
        assert_eq!(req.method, "login");
        assert_eq!(req.params.len(), 2);
        assert_eq!(req.params[0].as_str().unwrap(), "LTC_ADDR.rig1");
    }

    #[test]
    fn test_parse_miner_worker() {
        let (m, w) = parse_miner_worker("LTC_ADDR.rig1");
        assert_eq!(m, "LTC_ADDR");
        assert_eq!(w, "rig1");

        let (m2, w2) = parse_miner_worker("LTC_ADDR");
        assert_eq!(m2, "LTC_ADDR");
        assert_eq!(w2, "default");
    }

    #[test]
    fn test_response_ok() {
        let resp = response_ok(&Value::Number(1.into()), Value::Bool(true));
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"], true);
        assert_eq!(v["error"], Value::Null);
    }

    #[test]
    fn test_set_difficulty() {
        let msg = set_difficulty_notification(1024.0);
        let v: Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["method"], "mining.set_difficulty");
        assert_eq!(v["params"][0], 1024.0);
    }
}
