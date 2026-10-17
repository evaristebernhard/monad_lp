//! Read-only wallet balances through the existing local RPC router.
use super::*;
use anyhow::{bail, ensure};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_ADDRESS: &str = "0x41d5D3B5f2e497d1F4bCF767ffac9DDAb85D2b79";
// Circle native USDC on Monad mainnet (chain ID 143).
const USDC: &str = "0x754704Bc059F8C67012fEd69BC8A327a5aafb603";
type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

#[derive(Deserialize)]
pub struct WalletQuery {
    address: Option<String>,
}

pub async fn query(State(s): State<Arc<AppState>>, Query(q): Query<WalletQuery>) -> ApiResult {
    respond(&s, q.address.as_deref().unwrap_or(DEFAULT_ADDRESS)).await
}

pub async fn by_address(State(s): State<Arc<AppState>>, Path(address): Path<String>) -> ApiResult {
    respond(&s, &address).await
}

async fn respond(s: &AppState, address: &str) -> ApiResult {
    if address.len() != 42
        || !address.starts_with("0x")
        || !address[2..].bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"address must be a 0x-prefixed 20-byte EVM address"})),
        ));
    }
    match tokio::time::timeout(Duration::from_secs(20), balances(s, address)).await {
        Ok(Ok(value)) => Ok(Json(value)),
        Ok(Err(error)) => Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({"error":error.to_string()})),
        )),
        Err(_) => Err((
            StatusCode::GATEWAY_TIMEOUT,
            Json(json!({"error":"Monad wallet RPC timed out"})),
        )),
    }
}

async fn rpc(s: &AppState, method: &str, params: Value) -> Result<String> {
    let response = s
        .rpc_http
        .post(format!(
            "{}/rpc/monad",
            s.rpc_price_url.trim_end_matches('/')
        ))
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("local Monad RPC connection failed"))?;
    ensure!(
        response.status().is_success(),
        "local Monad RPC returned HTTP {}",
        response.status()
    );
    let value: Value = response
        .json()
        .await
        .context("invalid local Monad RPC JSON")?;
    if value.get("error").is_some() {
        // Never expose provider URLs or credentials returned by the transport.
        bail!("Monad RPC rejected {method}");
    }
    Ok(value["result"]
        .as_str()
        .context("missing Monad RPC result")?
        .to_owned())
}

async fn balances(s: &AppState, address: &str) -> Result<Value> {
    let (chain, block) = tokio::try_join!(
        rpc(s, "eth_chainId", json!([])),
        rpc(s, "eth_blockNumber", json!([]))
    )?;
    ensure!(
        hex_u64(&chain)? == 143,
        "RPC is not Monad mainnet (chain ID 143)"
    );
    let block_number = hex_u64(&block)?;
    // Pin both assets to the same block, including when the router fails over.
    let data = format!("0x70a08231000000000000000000000000{}", &address[2..]);
    let (mon, usdc, decimals) = tokio::try_join!(
        rpc(s, "eth_getBalance", json!([address, block])),
        rpc(s, "eth_call", json!([{"to":USDC,"data":data}, block])),
        rpc(
            s,
            "eth_call",
            json!([{"to":USDC,"data":"0x313ce567"}, block])
        )
    )?;
    ensure!(hex_u64(&decimals)? == 6, "unexpected native USDC decimals");
    let mon_raw = decimal(&mon)?;
    let usdc_raw = decimal(&usdc)?;
    Ok(json!({
        "chain":"monad", "chain_id":143, "address":address,
        "block_number":block_number, "block_tag":block,
        "observed_at":SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        "source":"local_rpc", "status":"ok",
        "mon":{"symbol":"MON","decimals":18,"balance":units(&mon_raw,18),"raw_balance":mon_raw},
        "usdc":{"symbol":"USDC","contract_address":USDC,"decimals":6,"balance":units(&usdc_raw,6),"raw_balance":usdc_raw}
    }))
}

fn hex_u64(value: &str) -> Result<u64> {
    let hex = value.strip_prefix("0x").context("invalid RPC hex value")?;
    ensure!(
        !hex.is_empty() && hex.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid RPC hex value"
    );
    let hex = hex.trim_start_matches('0');
    Ok(if hex.is_empty() {
        0
    } else {
        u64::from_str_radix(hex, 16)?
    })
}

// Decimal strings preserve every uint256 digit without floating-point rounding.
fn decimal(value: &str) -> Result<String> {
    let hex = value.strip_prefix("0x").context("invalid RPC hex value")?;
    ensure!(
        !hex.is_empty() && hex.len() <= 64,
        "invalid RPC uint256 length"
    );
    let mut digits = vec![0u8];
    for ch in hex.chars() {
        let mut carry = ch.to_digit(16).context("invalid RPC hex digit")?;
        for digit in &mut digits {
            carry += u32::from(*digit) * 16;
            *digit = (carry % 10) as u8;
            carry /= 10;
        }
        while carry > 0 {
            digits.push((carry % 10) as u8);
            carry /= 10;
        }
    }
    Ok(digits.iter().rev().map(|d| char::from(b'0' + d)).collect())
}

fn units(raw: &str, decimals: usize) -> String {
    let padded = format!("{:0>width$}", raw, width = decimals + 1);
    let (whole, fraction) = padded.split_at(padded.len() - decimals);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        whole.into()
    } else {
        format!("{whole}.{fraction}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_uint256_and_token_units() {
        assert_eq!(
            units(&decimal("0x41d4eb0c5fcfdc41d").unwrap(), 18),
            "75.898795934521082909"
        );
        assert_eq!(units(&decimal("0x17dfc2").unwrap(), 6), "1.56461");
        assert_eq!(units(&decimal("0x0").unwrap(), 18), "0");
        assert_eq!(units(&decimal("0x1").unwrap(), 18), "0.000000000000000001");
        assert_eq!(
            decimal(&format!("0x{}", "f".repeat(64))).unwrap(),
            "115792089237316195423570985008687907853269984665640564039457584007913129639935"
        );
        for invalid in ["0x", "0xgg", "123", &format!("0x{}", "f".repeat(65))] {
            assert!(decimal(invalid).is_err());
        }
    }
}
