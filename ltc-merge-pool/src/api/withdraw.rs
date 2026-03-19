/// Withdrawal API endpoint.

use actix_web::{web, HttpResponse};
use serde::{Deserialize, Serialize};
use log::info;

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

/// POST /api/withdraw
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
    match db.has_recent_withdrawal(miner, &coin, 4).await {
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

    // Debit balance first (atomic check)
    if let Err(_) = db.debit_balance(miner, &coin, amount).await {
        return HttpResponse::BadRequest().json(WithdrawResponse {
            success: false,
            message: "Insufficient balance (concurrent withdrawal?)".to_string(),
            withdrawal_id: None,
        });
    }

    // Create withdrawal record
    match db.create_withdrawal(miner, &coin, amount, fee).await {
        Ok(id) => {
            info!(
                "Withdrawal created: id={} miner={} coin={} amount={:.8} fee={:.8}",
                id, miner, coin, amount, fee
            );
            HttpResponse::Ok().json(WithdrawResponse {
                success: true,
                message: format!(
                    "Withdrawal of {:.8} {} queued (fee: {:.8} {}). It will be processed shortly.",
                    amount, coin, fee, coin
                ),
                withdrawal_id: Some(id),
            })
        }
        Err(e) => {
            // Refund the balance since withdrawal creation failed
            let _ = db.credit_balance(miner, &coin, amount).await;
            HttpResponse::InternalServerError().json(WithdrawResponse {
                success: false,
                message: format!("Failed to create withdrawal: {}", e),
                withdrawal_id: None,
            })
        }
    }
}
