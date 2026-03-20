/// Withdrawal API endpoint and payout address management.

use actix_web::{web, HttpResponse};
use serde::{Deserialize, Serialize};
use log::{info, warn, error};

use crate::api::pool::AppState;

#[derive(Deserialize)]
pub struct WithdrawRequest {
    pub miner: String,
    pub coin: String,
    pub amount: f64,
}

#[derive(Serialize)]
struct WithdrawResponse {
    success: bool,
    message: String,
    withdrawal_id: Option<i32>,
}

#[derive(Deserialize)]
pub struct SetPayoutAddressRequest {
    pub miner: String,
    pub coin: String,
    pub address: String,
}

#[derive(Serialize)]
struct SetPayoutAddressResponse {
    success: bool,
    message: String,
}

#[derive(Serialize)]
struct PayoutAddressEntry {
    coin: String,
    address: String,
}

#[derive(Serialize)]
struct GetPayoutAddressesResponse {
    success: bool,
    addresses: Vec<PayoutAddressEntry>,
}

/// Minimum withdrawal amounts per coin (in coin units).
fn min_withdrawal(coin: &str) -> f64 {
    match coin {
        "LTC" => 0.01,
        "DOGE" => 100.0,
        "PEPE" => 100.0,
        "BELLS" => 0.1,
        "LKY" => 0.1,
        "JKC" => 10.0,
        "DINGO" => 100.0,
        "SHIC" => 100.0,
        "TRMP" => 1000.0,
        _ => 0.01,
    }
}

/// Withdrawal fee per coin.
fn withdrawal_fee(coin: &str) -> f64 {
    match coin {
        "LTC" => 0.001,
        "DOGE" => 2.0,
        "PEPE" => 2.0,
        "BELLS" => 0.01,
        "LKY" => 0.01,
        "JKC" => 1.0,
        "DINGO" => 2.0,
        "SHIC" => 2.0,
        "TRMP" => 10.0,
        _ => 0.001,
    }
}

/// POST /api/payout-address — Set a payout address for a coin
pub async fn set_payout_address(
    data: web::Data<AppState>,
    body: web::Json<SetPayoutAddressRequest>,
) -> HttpResponse {
    let db = &data.db;
    let config = &data.config;

    let coin = body.coin.to_uppercase();
    let miner = &body.miner;
    let address = body.address.trim();

    // Validate coin exists
    if config.coin_by_symbol(&coin).is_none() {
        return HttpResponse::BadRequest().json(SetPayoutAddressResponse {
            success: false,
            message: format!("Unknown coin: {}", coin),
        });
    }

    // Validate miner address length
    if miner.len() < 20 || miner.len() > 128 {
        return HttpResponse::BadRequest().json(SetPayoutAddressResponse {
            success: false,
            message: "Invalid miner address".to_string(),
        });
    }

    // Validate payout address length
    if address.len() < 20 || address.len() > 128 {
        return HttpResponse::BadRequest().json(SetPayoutAddressResponse {
            success: false,
            message: "Invalid payout address".to_string(),
        });
    }

    // Validate the payout address with the coin's daemon
    if let Some(rpc) = data.rpc_clients.get(&coin) {
        match rpc.validate_address(address).await {
            Ok(result) => {
                if !result.isvalid {
                    return HttpResponse::BadRequest().json(SetPayoutAddressResponse {
                        success: false,
                        message: format!("Address is not valid for {}", coin),
                    });
                }
            }
            Err(e) => {
                warn!("Failed to validate address {} for {}: {}", address, coin, e);
                return HttpResponse::InternalServerError().json(SetPayoutAddressResponse {
                    success: false,
                    message: format!("Failed to validate address with {} daemon", coin),
                });
            }
        }
    } else {
        return HttpResponse::InternalServerError().json(SetPayoutAddressResponse {
            success: false,
            message: format!("No RPC client available for {}", coin),
        });
    }

    // Check that miner exists in the pool
    match db.get_miner(miner).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return HttpResponse::BadRequest().json(SetPayoutAddressResponse {
                success: false,
                message: "Miner address not found in pool".to_string(),
            });
        }
        Err(e) => {
            return HttpResponse::InternalServerError().json(SetPayoutAddressResponse {
                success: false,
                message: format!("Database error: {}", e),
            });
        }
    }

    // Store the payout address
    match db.set_payout_address(miner, &coin, address).await {
        Ok(()) => {
            info!("Payout address set: miner={} coin={} address={}", miner, coin, address);
            HttpResponse::Ok().json(SetPayoutAddressResponse {
                success: true,
                message: format!("Payout address for {} set to {}", coin, address),
            })
        }
        Err(e) => {
            error!("Failed to set payout address: {}", e);
            HttpResponse::InternalServerError().json(SetPayoutAddressResponse {
                success: false,
                message: format!("Failed to save payout address: {}", e),
            })
        }
    }
}

/// GET /api/payout-addresses/{miner} — Get all payout addresses for a miner
pub async fn get_payout_addresses(
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> HttpResponse {
    let db = &data.db;
    let miner = path.into_inner();

    match db.get_payout_addresses(&miner).await {
        Ok(rows) => {
            let addresses: Vec<PayoutAddressEntry> = rows
                .into_iter()
                .map(|r| PayoutAddressEntry {
                    coin: r.coin,
                    address: r.address,
                })
                .collect();
            HttpResponse::Ok().json(GetPayoutAddressesResponse {
                success: true,
                addresses,
            })
        }
        Err(e) => {
            error!("Failed to get payout addresses: {}", e);
            HttpResponse::InternalServerError().json(GetPayoutAddressesResponse {
                success: true,
                addresses: vec![],
            })
        }
    }
}

/// POST /api/withdraw
///
/// Fixed flow:
/// 1. Validate inputs and check balance
/// 2. Look up payout address for (miner, coin)
/// 3. Validate payout address with daemon
/// 4. Create withdrawal record with status='pending'
/// 5. Send via RPC
/// 6. If SUCCESS -> deduct balance, update withdrawal status='completed', store tx_hash
/// 7. If FAIL -> update withdrawal status='failed', DON'T deduct balance
pub async fn create_withdrawal(
    data: web::Data<AppState>,
    body: web::Json<WithdrawRequest>,
) -> HttpResponse {
    let db = &data.db;
    let config = &data.config;

    let coin = body.coin.to_uppercase();
    let miner = &body.miner;
    let amount = body.amount;

    // Validate coin exists
    if config.coin_by_symbol(&coin).is_none() {
        return HttpResponse::BadRequest().json(WithdrawResponse {
            success: false,
            message: format!("Unknown coin: {}", coin),
            withdrawal_id: None,
        });
    }

    // Validate miner address length
    if miner.len() < 20 || miner.len() > 128 {
        return HttpResponse::BadRequest().json(WithdrawResponse {
            success: false,
            message: "Invalid miner address".to_string(),
            withdrawal_id: None,
        });
    }

    // Check minimum withdrawal
    let min = min_withdrawal(&coin);
    if amount < min {
        return HttpResponse::BadRequest().json(WithdrawResponse {
            success: false,
            message: format!("Minimum withdrawal is {} {}", min, coin),
            withdrawal_id: None,
        });
    }

    // Check rate limiting (1 withdrawal per coin per 4 hours)
    match db.has_recent_withdrawal(miner, &coin, 0).await {
        Ok(true) => {
            return HttpResponse::TooManyRequests().json(WithdrawResponse {
                success: false,
                message: "Please wait 4 hours between withdrawals for the same coin".to_string(),
                withdrawal_id: None,
            });
        }
        Ok(false) => {}
        Err(e) => {
            return HttpResponse::InternalServerError().json(WithdrawResponse {
                success: false,
                message: format!("Database error: {}", e),
                withdrawal_id: None,
            });
        }
    }

    // Check balance
    let balance = match db.get_balance(miner, &coin).await {
        Ok(b) => b,
        Err(e) => {
            return HttpResponse::InternalServerError().json(WithdrawResponse {
                success: false,
                message: format!("Database error: {}", e),
                withdrawal_id: None,
            });
        }
    };

    if balance < amount {
        return HttpResponse::BadRequest().json(WithdrawResponse {
            success: false,
            message: format!("Insufficient balance. Available: {:.8} {}", balance, coin),
            withdrawal_id: None,
        });
    }

    let fee = withdrawal_fee(&coin);
    let send_amount = amount - fee;

    if send_amount <= 0.0 {
        return HttpResponse::BadRequest().json(WithdrawResponse {
            success: false,
            message: "Amount after fee is zero or negative".to_string(),
            withdrawal_id: None,
        });
    }

    // Look up the payout address for (miner, coin)
    let payout_address = match db.get_payout_address(miner, &coin).await {
        Ok(Some(addr)) => addr,
        Ok(None) => {
            // No payout address set — for LTC, fall back to the miner's own address.
            // For other coins, the miner MUST set a payout address.
            if coin == "LTC" {
                miner.clone()
            } else {
                return HttpResponse::BadRequest().json(WithdrawResponse {
                    success: false,
                    message: format!(
                        "No payout address set for {}. Please set a {} withdrawal address first.",
                        coin, coin
                    ),
                    withdrawal_id: None,
                });
            }
        }
        Err(e) => {
            return HttpResponse::InternalServerError().json(WithdrawResponse {
                success: false,
                message: format!("Database error looking up payout address: {}", e),
                withdrawal_id: None,
            });
        }
    };

    // Validate the payout address with the coin's daemon
    let rpc = match data.rpc_clients.get(&coin) {
        Some(r) => r,
        None => {
            return HttpResponse::InternalServerError().json(WithdrawResponse {
                success: false,
                message: format!("No RPC client for {}", coin),
                withdrawal_id: None,
            });
        }
    };

    match rpc.validate_address(&payout_address).await {
        Ok(result) => {
            if !result.isvalid {
                return HttpResponse::BadRequest().json(WithdrawResponse {
                    success: false,
                    message: format!(
                        "Payout address {} is not valid for {}. Please update your {} withdrawal address.",
                        payout_address, coin, coin
                    ),
                    withdrawal_id: None,
                });
            }
        }
        Err(e) => {
            warn!("Failed to validate payout address: {}", e);
            return HttpResponse::InternalServerError().json(WithdrawResponse {
                success: false,
                message: format!("Failed to validate payout address with {} daemon", coin),
                withdrawal_id: None,
            });
        }
    }

    // Step 1: Create withdrawal record with status='pending' (NO balance deduction yet)
    let withdrawal_id = match db.create_withdrawal_with_address(miner, &coin, amount, fee, &payout_address).await {
        Ok(id) => id,
        Err(e) => {
            return HttpResponse::InternalServerError().json(WithdrawResponse {
                success: false,
                message: format!("Failed to create withdrawal record: {}", e),
                withdrawal_id: None,
            });
        }
    };

    info!(
        "Withdrawal created: id={} miner={} coin={} amount={:.8} fee={:.8} payout_address={}",
        withdrawal_id, miner, coin, amount, fee, payout_address
    );

    // Step 2: Send via RPC
    match rpc.send_to_address(&payout_address, send_amount).await {
        Ok(txid) => {
            info!(
                "Withdrawal {} sent successfully: txid={} to={}",
                withdrawal_id, txid, payout_address
            );

            // Step 3a: SUCCESS - Deduct balance NOW
            if let Err(e) = db.debit_balance(miner, &coin, amount).await {
                // This is bad - we sent the coins but can't deduct the balance.
                // Log it prominently so it can be fixed manually.
                error!(
                    "CRITICAL: Withdrawal {} sent (txid={}) but balance deduction failed: {}. \
                     Miner={} Coin={} Amount={:.8}. Manual intervention required!",
                    withdrawal_id, txid, e, miner, coin, amount
                );
            }

            // Mark withdrawal as completed
            if let Err(e) = db.complete_withdrawal(withdrawal_id, &txid).await {
                error!(
                    "Failed to mark withdrawal {} as completed (txid={}): {}",
                    withdrawal_id, txid, e
                );
            }

            HttpResponse::Ok().json(WithdrawResponse {
                success: true,
                message: format!(
                    "Withdrawal of {:.8} {} sent to {}. TX: {}",
                    send_amount, coin, payout_address, txid
                ),
                withdrawal_id: Some(withdrawal_id),
            })
        }
        Err(e) => {
            // Step 3b: FAIL - Do NOT deduct balance. Mark withdrawal as failed.
            let msg = format!("sendtoaddress failed: {}", e);
            error!("Withdrawal {} failed: {}", withdrawal_id, msg);

            if let Err(e2) = db.fail_withdrawal(withdrawal_id, &msg).await {
                error!("Failed to mark withdrawal {} as failed: {}", withdrawal_id, e2);
            }

            HttpResponse::InternalServerError().json(WithdrawResponse {
                success: false,
                message: format!("Withdrawal failed: {}. Your balance has NOT been deducted.", e),
                withdrawal_id: Some(withdrawal_id),
            })
        }
    }
}
