//! Unsigned MON -> USDC trades. The caller owns signing and broadcasting.
use super::*;
use alloy_primitives::{address, hex, Address, Bytes, U256};
use alloy_sol_types::{sol, SolCall, SolValue};
use anyhow::{bail, ensure};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

sol! {
    struct PoolKey { address currency0; address currency1; uint24 fee; int24 tickSpacing; address hooks; }
    struct QuoteParams { PoolKey poolKey; bool zeroForOne; uint128 exactAmount; bytes hookData; }
    struct SwapParams { PoolKey poolKey; bool zeroForOne; uint128 amountIn; uint128 amountOutMinimum; bytes hookData; }
    function quoteExactInputSingle(QuoteParams params) returns (uint256 amountOut, uint256 gasEstimate);
    function execute(bytes commands, bytes[] inputs, uint256 deadline) payable;
}
const USDC: Address = address!("754704bc059f8c67012fed69bc8a327a5aafb603");
const ROUTER: Address = address!("0d97dc33264bfc1c226207428a79b26757fb9dc3");
const QUOTER: Address = address!("a222dd357a9076d1091ed6aa2e16c9742dd26891");
const POOL_ID: &str = "0x18a9fc874581f3ba12b7898f80a683c66fd5877fd74b26a85ba9a3a79c549954";
type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    amount_mon: String,
    #[serde(default = "default_slippage")]
    slippage_bps: u16,
    wallet: Option<String>,
}
fn default_slippage() -> u16 {
    50
}
fn key() -> PoolKey {
    PoolKey {
        currency0: Address::ZERO,
        currency1: USDC,
        fee: 500u32.try_into().unwrap(),
        tickSpacing: 10i32.try_into().unwrap(),
        hooks: Address::ZERO,
    }
}
fn amount(value: &str) -> Result<u128> {
    ensure!(value.len() <= 60, "invalid amount_mon");
    let parts: Vec<_> = value.split('.').collect();
    ensure!(!parts.is_empty() && parts.len() <= 2, "invalid amount_mon");
    let whole = parts[0];
    ensure!(
        !whole.is_empty()
            && whole.bytes().all(|c| c.is_ascii_digit())
            && (whole.len() == 1 || !whole.starts_with('0')),
        "invalid amount_mon"
    );
    let fraction = parts.get(1).copied().unwrap_or("");
    ensure!(
        parts.len() == 1
            || (!fraction.is_empty()
                && fraction.len() <= 18
                && fraction.bytes().all(|c| c.is_ascii_digit())),
        "amount_mon supports at most 18 decimals"
    );
    let raw = format!("{whole}{fraction:0<18}")
        .parse::<u128>()
        .context("amount_mon too large")?;
    ensure!(raw > 0, "amount_mon must be positive");
    Ok(raw)
}
fn error(code: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (code, Json(json!({"error":message})))
}
async fn rpc(s: &AppState, method: &str, params: Value) -> Result<Value> {
    let response = s
        .rpc_http
        .post(format!(
            "{}/rpc/monad",
            s.rpc_price_url.trim_end_matches('/')
        ))
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .context("RPC transport failed")?;
    ensure!(response.status().is_success(), "RPC HTTP failure");
    let body: Value = response.json().await?;
    if body.get("error").is_some() {
        bail!("RPC rejected request");
    }
    body.get("result")
        .filter(|v| !v.is_null())
        .cloned()
        .context("missing RPC result")
}
fn uint(v: &Value) -> Result<U256> {
    U256::from_str_radix(
        v.as_str()
            .context("invalid RPC integer")?
            .strip_prefix("0x")
            .context("invalid RPC hex")?,
        16,
    )
    .context("invalid RPC integer")
}
fn calldata(amount_in: u128, minimum: u128, deadline: u64) -> Vec<u8> {
    let swap = SwapParams {
        poolKey: key(),
        zeroForOne: true,
        amountIn: amount_in,
        amountOutMinimum: minimum,
        hookData: Bytes::new(),
    };
    let actions = (
        Bytes::from_static(&[0x06, 0x0c, 0x0f]),
        vec![
            Bytes::from((swap,).abi_encode_params()),
            Bytes::from((Address::ZERO, U256::from(amount_in)).abi_encode_params()),
            Bytes::from((USDC, U256::from(minimum)).abi_encode_params()),
        ],
    )
        .abi_encode_params();
    let refund = (Address::ZERO, Address::with_last_byte(1), U256::ZERO).abi_encode_params();
    executeCall {
        commands: Bytes::from_static(&[0x10, 0x04]),
        inputs: vec![actions.into(), refund.into()],
        deadline: U256::from(deadline),
    }
    .abi_encode()
}
async fn run(s: &AppState, request: Request, build: bool) -> ApiResult {
    let input =
        amount(&request.amount_mon).map_err(|e| error(StatusCode::BAD_REQUEST, &e.to_string()))?;
    if !(1..=100).contains(&request.slippage_bps) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "slippage_bps must be 1..100",
        ));
    }
    let wallet = request
        .wallet
        .as_deref()
        .map(str::parse::<Address>)
        .transpose()
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid wallet address"))?;
    if build && wallet.is_none_or(|w| w == Address::ZERO) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "build requires a nonzero wallet address",
        ));
    }
    let work = async {
        ensure!(
            uint(&rpc(s, "eth_chainId", json!([])).await?)? == U256::from(143),
            "wrong chain"
        );
        let block = rpc(s, "eth_getBlockByNumber", json!(["latest", false])).await?;
        let number = block["number"].as_str().context("missing block")?;
        let timestamp: u64 = uint(&block["timestamp"])?.try_into()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        ensure!(
            timestamp <= now + 30 && timestamp + 60 >= now,
            "stale block"
        );
        let call = quoteExactInputSingleCall {
            params: QuoteParams {
                poolKey: key(),
                zeroForOne: true,
                exactAmount: input,
                hookData: Bytes::new(),
            },
        };
        let result = rpc(s,"eth_call",json!([{"to":QUOTER.to_string(),"data":hex::encode_prefixed(call.abi_encode())},number])).await?;
        let decoded = quoteExactInputSingleCall::abi_decode_returns(&hex::decode(
            result.as_str().context("invalid quote")?,
        )?)?;
        let output = decoded.amountOut;
        ensure!(
            output > U256::ZERO && output <= U256::from(u128::MAX),
            "invalid quote output"
        );
        let minimum = output * U256::from(10000 - request.slippage_bps) / U256::from(10000);
        ensure!(minimum > U256::ZERO, "quote output too small");
        let deadline = timestamp + 120;
        let mut value = json!({"chain_id":143,"direction":"MON_TO_USDC","pool_id":POOL_ID,"router":ROUTER.to_string(),"token_out":USDC.to_string(),"token_out_decimals":6,"amount_in_wei":input.to_string(),"amount_out_raw":output.to_string(),"minimum_out_raw":minimum.to_string(),"slippage_bps":request.slippage_bps,"block_number":number,"quoted_at":now,"expires_at":now+30,"deadline":deadline,"recipient":"transaction_sender"});
        if let Some(wallet) = wallet {
            value["wallet"] = json!(wallet.to_string());
        }
        if build {
            value["transaction"] = json!({"chainId":143,"from":wallet.unwrap().to_string(),"to":ROUTER.to_string(),"value":input.to_string(),"data":hex::encode_prefixed(calldata(input, minimum.try_into()?, deadline))});
        }
        Ok::<_, anyhow::Error>(value)
    };
    match tokio::time::timeout(Duration::from_secs(25), work).await {
        Ok(Ok(value)) => Ok(Json(value)),
        // Provider errors can contain credentials. Keep transport details private.
        Ok(Err(_)) => Err(error(
            StatusCode::BAD_GATEWAY,
            "Monad quote unavailable: check RPC chain, freshness and pool liquidity",
        )),
        Err(_) => Err(error(StatusCode::GATEWAY_TIMEOUT, "Monad quote timed out")),
    }
}
pub async fn quote(State(s): State<Arc<AppState>>, Json(request): Json<Request>) -> ApiResult {
    run(&s, request, false).await
}
pub async fn build(State(s): State<Arc<AppState>>, Json(request): Json<Request>) -> ApiResult {
    run(&s, request, true).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn amounts_are_exact_and_strict() {
        assert_eq!(amount("0.000000000000000001").unwrap(), 1);
        assert_eq!(amount("1.25").unwrap(), 1_250_000_000_000_000_000);
        for s in [
            "0",
            "-1",
            "1e3",
            "01",
            "1.",
            ".1",
            " 1",
            "1.0000000000000000001",
            "340282366920938463464",
        ] {
            assert!(amount(s).is_err(), "{s}");
        }
    }
    #[test]
    fn encoding_matches_existing_browser() {
        assert_eq!(
            alloy_primitives::keccak256(key().abi_encode()).to_string(),
            POOL_ID
        );
        let expected = include_str!("../tests/mon-buy-calldata.txt").trim();
        assert_eq!(
            hex::encode_prefixed(calldata(100000000000000000, 12345, 2000000000)),
            expected
        );
    }
}
