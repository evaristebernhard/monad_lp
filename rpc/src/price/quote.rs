//! Atomic current-price reads.
//!
//! A quote is one logical pool read.  A refresh is only a batch of these
//! requests; it is not a prerequisite for obtaining a current price.

use axum::{
    body::{to_bytes, Bytes},
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    forward::{forward_impl, rpc_error},
    state::{AppState, RpcQuery},
};

#[derive(Debug, Deserialize)]
pub(crate) struct PriceRequest {
    pub(crate) pool_type: String,
    pub(crate) token0_symbol: Option<String>,
    pub(crate) token1_symbol: Option<String>,
    pub(crate) token0_anchor_usd: Option<f64>,
    pub(crate) token1_anchor_usd: Option<f64>,
}

#[derive(Debug)]
struct Account {
    data: Vec<u8>,
}

fn stable(symbol: Option<&str>) -> bool {
    matches!(
        symbol
            .unwrap_or_default()
            .trim()
            .to_ascii_uppercase()
            .as_str(),
        "USDC"
            | "USDT"
            | "USDC.E"
            | "USDT0"
            | "DAI"
            | "USDE"
            | "USDS"
            | "USDB"
            | "USDBC"
            | "FDUSD"
            | "TUSD"
            | "FRAX"
            | "BUSD"
            | "USD0"
            | "USD1"
            | "AUSD"
            | "USDG"
            | "PYUSD"
            | "USD"
            | "LUSD"
    )
}

fn u16(data: &[u8], offset: usize) -> Option<u16> {
    data.get(offset..offset + 2)
        .and_then(|value| value.try_into().ok())
        .map(u16::from_le_bytes)
}

fn u64(data: &[u8], offset: usize) -> Option<u64> {
    data.get(offset..offset + 8)
        .and_then(|value| value.try_into().ok())
        .map(u64::from_le_bytes)
}

fn u128(data: &[u8], offset: usize) -> Option<u128> {
    data.get(offset..offset + 16)
        .and_then(|value| value.try_into().ok())
        .map(u128::from_le_bytes)
}

fn i32(data: &[u8], offset: usize) -> Option<i32> {
    data.get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(i32::from_le_bytes)
}

fn key(data: &[u8], offset: usize) -> Option<String> {
    data.get(offset..offset + 32)
        .map(bs58::encode)
        .map(|value| value.into_string())
}

fn account_from(value: &Value) -> Option<Account> {
    let data = value
        .get("result")?
        .get("value")?
        .get("data")?
        .as_array()?
        .first()?
        .as_str()?;
    Some(Account {
        data: base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data).ok()?,
    })
}

fn response_for_id(value: Value, id: u64) -> Option<Value> {
    match value {
        Value::Array(values) => values
            .into_iter()
            .find(|item| item.get("id").and_then(Value::as_u64) == Some(id)),
        value if value.get("id").and_then(Value::as_u64) == Some(id) => Some(value),
        _ => None,
    }
}

async fn rpc_json(state: AppState, chain: &str, request: Value) -> Result<Value, Response> {
    let body = Bytes::from(
        serde_json::to_vec(&request)
            .map_err(|error| rpc_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    let response = forward_impl(
        state,
        chain.to_string(),
        RpcQuery { title: None },
        body,
        false,
    )
    .await;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 10 * 1024 * 1024)
        .await
        .map_err(|error| rpc_error(StatusCode::BAD_GATEWAY, error.to_string()))?;
    if !status.is_success() {
        return Err((status, bytes).into_response());
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| rpc_error(StatusCode::BAD_GATEWAY, error.to_string()))
}

async fn accounts(
    state: AppState,
    chain: &str,
    requests: Vec<Value>,
) -> Result<Vec<Option<Account>>, Response> {
    let value = rpc_json(state, chain, Value::Array(requests)).await?;
    let mut result = Vec::new();
    for id in 1..=value.as_array().map_or(0, |items| items.len()) as u64 {
        result.push(response_for_id(value.clone(), id).and_then(|item| account_from(&item)));
    }
    Ok(result)
}

fn account_request(id: u64, address: &str) -> Value {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "getAccountInfo",
        "params": [address, {"encoding": "base64", "commitment": "confirmed"}]
    })
}

fn mint_offsets(pool_type: &str, length: usize) -> Option<(usize, usize)> {
    match pool_type {
        "solana_clmm" => Some((73, 105)),
        "solana_whirlpool" => Some((101, 181)),
        "solana_dlmm" => Some((88, 120)),
        "solana_standard" if length == 637 => Some((168, 200)),
        "solana_standard" if length == 752 => Some((400, 432)),
        "solana_standard" if length == 1232 => Some((464, 496)),
        _ => None,
    }
}

fn vault_offsets(length: usize) -> Option<(usize, usize)> {
    match length {
        637 => Some((72, 104)),
        752 => Some((336, 368)),
        1232 => Some((368, 400)),
        _ => None,
    }
}

fn mint_decimals(account: &Account) -> Option<u32> {
    let decimals = *account.data.get(44)? as u32;
    (decimals <= 36).then_some(decimals)
}

fn token_amount(account: &Account) -> Option<u64> {
    u64(&account.data, 64)
}

fn ratio(
    request: &PriceRequest,
    pool: &Account,
    extras: &[Option<Account>],
) -> Option<(f64, String, String)> {
    let data = &pool.data;
    let (mint0_offset, mint1_offset) = mint_offsets(&request.pool_type, data.len())?;
    let mint0 = key(data, mint0_offset)?;
    let mint1 = key(data, mint1_offset)?;
    let decimals0 = extras
        .first()
        .and_then(|item| item.as_ref())
        .and_then(mint_decimals)?;
    let decimals1 = extras
        .get(1)
        .and_then(|item| item.as_ref())
        .and_then(mint_decimals)?;
    let scale = 10f64.powi(decimals0 as i32 - decimals1 as i32);
    let raw_ratio = match request.pool_type.as_str() {
        "solana_clmm" => (u128(data, 253)? as f64).powi(2) / 2f64.powi(128) * scale,
        "solana_whirlpool" => (u128(data, 65)? as f64).powi(2) / 2f64.powi(128) * scale,
        "solana_dlmm" => {
            let active_id = i32(data, 76)?;
            let bin_step = u16(data, 80)? as f64;
            (1.0 + bin_step / 10_000.0).powi(active_id) * scale
        }
        "solana_standard" => {
            let (vault0_offset, vault1_offset) = vault_offsets(data.len())?;
            let _vault0 = key(data, vault0_offset)?;
            let _vault1 = key(data, vault1_offset)?;
            let amount0 = extras
                .get(2)
                .and_then(|item| item.as_ref())
                .and_then(token_amount)? as f64;
            let amount1 = extras
                .get(3)
                .and_then(|item| item.as_ref())
                .and_then(token_amount)? as f64;
            (amount1 / amount0) * scale
        }
        _ => return None,
    };
    // Pool reserves/sqrt-price encode the quote-side amount per base-side
    // amount. For USD prices, however, p1/p0 is the inverse: p0*x = p1*y.
    let price_ratio = 1.0 / raw_ratio;
    price_ratio
        .is_finite()
        .then_some((price_ratio, mint0, mint1))
}

fn usd_prices(request: &PriceRequest, ratio: f64) -> (Option<f64>, Option<f64>) {
    let p0 = request
        .token0_anchor_usd
        .or_else(|| stable(request.token0_symbol.as_deref()).then_some(1.0));
    let p1 = request
        .token1_anchor_usd
        .or_else(|| stable(request.token1_symbol.as_deref()).then_some(1.0));
    match (p0, p1) {
        (Some(p0), _) => (Some(p0), Some(p0 * ratio)),
        (_, Some(p1)) => (Some(p1 / ratio), Some(p1)),
        _ => (None, None),
    }
}

pub(crate) async fn decode_pool(
    state: AppState,
    chain: &str,
    pool_address: &str,
    request: PriceRequest,
) -> Result<Value, Response> {
    if !chain.to_ascii_lowercase().contains("solana") {
        return Err(rpc_error(
            StatusCode::NOT_IMPLEMENTED,
            "atomic quote currently supports Solana",
        ));
    }
    let pool_value = match rpc_json(state.clone(), &chain, account_request(1, &pool_address)).await
    {
        Ok(value) => value,
        Err(response) => return Err(response),
    };
    let Some(pool) = account_from(&pool_value) else {
        return Err(rpc_error(
            StatusCode::NOT_FOUND,
            "pool account is missing or unreadable",
        ));
    };
    let Some((mint0_offset, mint1_offset)) = mint_offsets(&request.pool_type, pool.data.len())
    else {
        return Err(rpc_error(
            StatusCode::NOT_IMPLEMENTED,
            "unsupported Solana pool account layout",
        ));
    };
    let Some(mint0) = key(&pool.data, mint0_offset) else {
        return Err(rpc_error(
            StatusCode::BAD_GATEWAY,
            "pool mint0 could not be decoded",
        ));
    };
    let Some(mint1) = key(&pool.data, mint1_offset) else {
        return Err(rpc_error(
            StatusCode::BAD_GATEWAY,
            "pool mint1 could not be decoded",
        ));
    };
    let mut requests = vec![account_request(1, &mint0), account_request(2, &mint1)];
    if let Some((vault0_offset, vault1_offset)) = vault_offsets(pool.data.len()) {
        if let (Some(vault0), Some(vault1)) = (
            key(&pool.data, vault0_offset),
            key(&pool.data, vault1_offset),
        ) {
            requests.push(account_request(3, &vault0));
            requests.push(account_request(4, &vault1));
        }
    }
    let extras = match accounts(state, &chain, requests).await {
        Ok(accounts) => accounts,
        Err(response) => return Err(response),
    };
    let Some((ratio, decoded_mint0, decoded_mint1)) = ratio(&request, &pool, &extras) else {
        return Err(rpc_error(
            StatusCode::BAD_GATEWAY,
            "pool price fields could not be decoded",
        ));
    };
    let (price0, price1) = usd_prices(&request, ratio);
    Ok(json!({
        "status": if price0.is_some() { "rpc_anchored" } else { "relative_only" },
        "chain": chain,
        "pool_address": pool_address,
        "pool_type": request.pool_type,
        "token0_address": decoded_mint0,
        "token1_address": decoded_mint1,
        "token1_per_token0": ratio,
        "token0_price_usd": price0,
        "token1_price_usd": price1,
    }))
}

pub(crate) async fn quote(
    State(state): State<AppState>,
    AxumPath((chain, pool_address)): AxumPath<(String, String)>,
    Json(request): Json<PriceRequest>,
) -> Response {
    match decode_pool(state, &chain, &pool_address, request).await {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}
