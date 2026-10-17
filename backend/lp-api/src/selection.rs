//! Read-only streams. All upstream requests use the existing local Monad RPC.
use super::*;
use axum::response::{
    sse::{Event, KeepAlive},
    Response, Sse,
};
use std::{
    convert::Infallible,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
type ApiError = (StatusCode, Json<ErrorBody>);
fn bad(message: &str) -> ApiError {
    (
        StatusCode::BAD_GATEWAY,
        Json(ErrorBody {
            error: message.into(),
        }),
    )
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
async fn read(s: &AppState, method: &str, params: Value) -> Result<Value, ApiError> {
    let r = s
        .rpc_http
        .post(format!(
            "{}/rpc/monad",
            s.rpc_price_url.trim_end_matches('/')
        ))
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map_err(|_| bad("RPC unavailable"))?;
    if !r.status().is_success() {
        return Err(bad("RPC request failed"));
    }
    let body: Value = r.json().await.map_err(|_| bad("Invalid RPC response"))?;
    if !body["error"].is_null() || body["result"].is_null() {
        return Err(bad("RPC result unavailable"));
    }
    Ok(body["result"].clone())
}
pub fn response(rx: mpsc::Receiver<Value>) -> Response {
    let stream = futures_util::stream::unfold(rx, |mut receiver| async move {
        receiver.recv().await.map(|value| {
            (
                Ok::<_, Infallible>(Event::default().data(value.to_string())),
                receiver,
            )
        })
    });
    let mut r = Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response();
    r.headers_mut()
        .insert("cache-control", "no-store, no-transform".parse().unwrap());
    r.headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    r
}
async fn quote_round(s: &AppState, data: &str) -> Result<Value, ApiError> {
    let started = Instant::now();
    let started_ms = now_ms();
    let (chain, block) = tokio::try_join!(
        read(s, "eth_chainId", json!([])),
        read(s, "eth_getBlockByNumber", json!(["latest", false]))
    )?;
    if chain.as_str() != Some("0x8f") || !block["number"].is_string() || !block["hash"].is_string()
    {
        return Err(bad("Wrong Monad RPC chain or block"));
    }
    let result=read(s,"eth_call",json!([{"to":"0xcA11bde05977b3631167028862bE2a173976CA11","data":data,"gas":"0x1c9c380"},block["number"]])).await?;
    let check = read(s, "eth_getBlockByNumber", json!([block["number"], false])).await?;
    if check["hash"] != block["hash"] {
        return Err(bad("Quote block changed"));
    }
    Ok(
        json!({"chainId":143,"block":block,"result":result,"startedAtMs":started_ms,"capturedAtMs":now_ms(),"durationMs":started.elapsed().as_millis() as u64}),
    )
}
// Public quote API owns the catalog, ABI encoding, decoding and ranking.
use alloy_primitives::{address, hex, keccak256, Address, Bytes, B256, U256};
use alloy_sol_types::{sol, SolCall, SolValue};
use std::{str::FromStr, sync::OnceLock};
sol! {
 struct RouteKey { address currency0;address currency1;uint24 fee;int24 tickSpacing;address hooks; }
 struct V4Input { RouteKey poolKey;bool zeroForOne;uint128 exactAmount;bytes hookData; }
 struct V3Input { address tokenIn;address tokenOut;uint256 amountIn;uint24 fee;uint160 sqrtPriceLimitX96; }
 struct AggregateCall {address target;bool allowFailure;bytes callData;}
 struct AggregateResult {bool success;bytes returnData;}
 struct ReplayContext {uint256 sourceChainId;uint256 sourceBlock;bytes32 sourceBlockHash;address tokenIn;address tokenOut;uint256 amountIn;}
 struct ReplayQuote {bytes32 poolId;uint256 amountOut;}
 interface RouteV4 {function quoteExactInputSingle(V4Input params) returns(uint256 amountOut,uint256 gasEstimate);}
 interface RouteV3 {function quoteExactInputSingle(V3Input params) returns(uint256 amountOut,uint160 sqrtPriceX96After,uint32 initializedTicksCrossed,uint256 gasEstimate);}
 interface RouteV2 {function getReserves() view returns(uint112 reserve0,uint112 reserve1,uint32 blockTimestampLast);}
 interface Aggregate {function aggregate3(AggregateCall[] calls) payable returns(AggregateResult[] results);}
}
const USDC: Address = address!("754704bc059f8c67012fed69bc8a327a5aafb603");
const WMON: Address = address!("3bd359c1119da7da1d913d1c4d2b7c461115433a");
const V4_QUOTER: Address = address!("a222dd357a9076d1091ed6aa2e16c9742dd26891");
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuoteRequest {
    amount: String,
    #[serde(default = "default_token")]
    token_in: String,
    token_out: Option<String>,
}
fn default_token() -> String {
    "MON".into()
}
fn invalid(message: &str) -> ApiError {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorBody {
            error: message.into(),
        }),
    )
}
fn catalog() -> &'static Value {
    static CATALOG: OnceLock<Value> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../选池/results/route-catalog.json"
        )))
        .expect("verified route catalog")
    })
}
struct Prepared {
    request: QuoteRequest,
    zero_for_one: bool,
    amount: u128,
    output_decimals: usize,
    calls: Vec<AggregateCall>,
    data: String,
}
fn parse_amount(value: &str, decimals: usize) -> Result<u128, ApiError> {
    if value.len() > 60 {
        return Err(invalid("amount too large"));
    }
    let parts = value.split('.').collect::<Vec<_>>();
    let whole = parts[0];
    let fraction = parts.get(1).copied().unwrap_or("");
    if parts.len() > 2
        || whole.is_empty()
        || !whole.bytes().all(|c| c.is_ascii_digit())
        || (whole.len() > 1 && whole.starts_with('0'))
        || parts.len() == 2
            && (fraction.is_empty()
                || fraction.len() > decimals
                || !fraction.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err(invalid("invalid amount precision or format"));
    }
    let raw = format!("{whole}{fraction:0<width$}", width = decimals)
        .parse::<u128>()
        .map_err(|_| invalid("amount too large"))?;
    if raw == 0 || raw > ((1u128 << 112) - 1) {
        return Err(invalid("amount outside supported range"));
    }
    Ok(raw)
}
fn addr(value: &Value) -> Result<Address, ApiError> {
    value
        .as_str()
        .and_then(|s| Address::from_str(s).ok())
        .ok_or_else(|| bad("Invalid catalog address"))
}
fn prepare(mut request: QuoteRequest) -> Result<Prepared, ApiError> {
    request.token_in = request.token_in.to_ascii_uppercase();
    let zero_for_one = match request.token_in.as_str() {
        "MON" => true,
        "USDC" => false,
        _ => return Err(invalid("only MON and USDC are supported")),
    };
    let output = if zero_for_one { "USDC" } else { "MON" };
    if request
        .token_out
        .as_deref()
        .is_some_and(|t| !t.eq_ignore_ascii_case(output))
    {
        return Err(invalid("token pair must be MON/USDC"));
    }
    request.token_out = Some(output.into());
    let amount = parse_amount(&request.amount, if zero_for_one { 18 } else { 6 })?;
    let mut calls = Vec::new();
    for q in catalog()["candidates"]
        .as_array()
        .ok_or_else(|| bad("Missing catalog"))?
    {
        let fee = q["fee"].as_u64().ok_or_else(|| bad("Missing fee"))?;
        let (target, call_data) = match q["kind"].as_str() {
            Some("v3") => (
                addr(&q["quoter"])?,
                RouteV3::quoteExactInputSingleCall {
                    params: V3Input {
                        tokenIn: if zero_for_one { WMON } else { USDC },
                        tokenOut: if zero_for_one { USDC } else { WMON },
                        amountIn: U256::from(amount),
                        fee: fee.try_into().map_err(|_| bad("Fee range"))?,
                        sqrtPriceLimitX96: Default::default(),
                    },
                }
                .abi_encode(),
            ),
            Some("v2") => (
                addr(&q["address"])?,
                RouteV2::getReservesCall {}.abi_encode(),
            ),
            Some("v4") => {
                let k = &q["poolKey"];
                (
                    V4_QUOTER,
                    RouteV4::quoteExactInputSingleCall {
                        params: V4Input {
                            poolKey: RouteKey {
                                currency0: addr(&k["currency0"])?,
                                currency1: addr(&k["currency1"])?,
                                fee: k["fee"]
                                    .as_u64()
                                    .ok_or_else(|| bad("Fee missing"))?
                                    .try_into()
                                    .map_err(|_| bad("Fee range"))?,
                                tickSpacing: k["tickSpacing"]
                                    .as_i64()
                                    .ok_or_else(|| bad("Tick missing"))?
                                    .try_into()
                                    .map_err(|_| bad("Tick range"))?,
                                hooks: addr(&k["hooks"])?,
                            },
                            zeroForOne: zero_for_one,
                            exactAmount: amount,
                            hookData: Bytes::new(),
                        },
                    }
                    .abi_encode(),
                )
            }
            _ => return Err(bad("Unsupported pool protocol")),
        };
        calls.push(AggregateCall {
            target,
            allowFailure: true,
            callData: call_data.into(),
        });
    }
    let data = hex::encode_prefixed(
        Aggregate::aggregate3Call {
            calls: calls.clone(),
        }
        .abi_encode(),
    );
    Ok(Prepared {
        request,
        zero_for_one,
        amount,
        output_decimals: if zero_for_one { 6 } else { 18 },
        calls,
        data,
    })
}
fn units(value: U256, decimals: usize) -> String {
    let raw = value.to_string();
    let mut text = format!("{raw:0>width$}", width = decimals + 1);
    let split = text.len() - decimals;
    text.insert(split, '.');
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}
fn output(q: &Value, bytes: &[u8], p: &Prepared) -> Result<U256, ApiError> {
    match q["kind"].as_str() {
        Some("v4") => RouteV4::quoteExactInputSingleCall::abi_decode_returns(bytes)
            .map(|v| v.amountOut)
            .map_err(|_| bad("V4 decode failed")),
        Some("v3") => RouteV3::quoteExactInputSingleCall::abi_decode_returns(bytes)
            .map(|v| v.amountOut)
            .map_err(|_| bad("V3 decode failed")),
        Some("v2") => {
            let r = RouteV2::getReservesCall::abi_decode_returns(bytes)
                .map_err(|_| bad("V2 decode failed"))?;
            let input_token = if p.zero_for_one { WMON } else { USDC };
            let (reserve_in, reserve_out) = if addr(&q["token0"])? == input_token {
                (U256::from(r.reserve0), U256::from(r.reserve1))
            } else {
                (U256::from(r.reserve1), U256::from(r.reserve0))
            };
            let adjusted = U256::from(p.amount)
                * U256::from(1_000_000 - q["fee"].as_u64().ok_or_else(|| bad("Fee missing"))?);
            Ok(if reserve_in.is_zero() || reserve_out.is_zero() {
                U256::ZERO
            } else {
                adjusted * reserve_out / (reserve_in * U256::from(1_000_000) + adjusted)
            })
        }
        _ => Err(bad("Unsupported quote")),
    }
}
fn decoded_snapshot(
    p: &Prepared,
    frame: &Value,
    results: &[AggregateResult],
) -> Result<Value, ApiError> {
    let candidates = catalog()["candidates"]
        .as_array()
        .ok_or_else(|| bad("Catalog missing"))?;
    if candidates.len() != results.len() {
        return Err(bad("Quote count mismatch"));
    }
    let mut quotes = Vec::new();
    let mut replay = Vec::new();
    let mut best: Option<(B256, U256, Value)> = None;
    let mut available = 0;
    for (q, result) in candidates.iter().zip(results) {
        let pool = q["pool"]
            .as_str()
            .and_then(|v| B256::from_str(v).ok())
            .ok_or_else(|| bad("Invalid pool ID"))?;
        let value = if result.success {
            output(q, &result.returnData, p).ok()
        } else {
            None
        };
        let out = value.unwrap_or(U256::ZERO);
        let status = if value.is_none() {
            "unavailable"
        } else if out.is_zero() {
            "zero"
        } else {
            "quoted"
        };
        if !out.is_zero() {
            available += 1
        }
        let mut row = q.clone();
        let object = row
            .as_object_mut()
            .ok_or_else(|| bad("Invalid pool metadata"))?;
        object.remove("quoterGasEstimate");
        object.insert("status".into(), json!(status));
        object.insert("amountOutRaw".into(), json!(out.to_string()));
        object.insert("amountOut".into(), json!(units(out, p.output_decimals)));
        if !out.is_zero()
            && best
                .as_ref()
                .is_none_or(|(id, amount, _)| out > *amount || out == *amount && pool < *id)
        {
            best = Some((pool, out, row.clone()))
        }
        replay.push(ReplayQuote {
            poolId: pool,
            amountOut: out,
        });
        quotes.push(row);
    }
    let (pool, out, selected) = best.ok_or_else(|| bad("No valid pool quotes"))?;
    let block = &frame["block"];
    let number = U256::from_str_radix(
        block["number"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches("0x"),
        16,
    )
    .map_err(|_| bad("Invalid block number"))?;
    let timestamp = U256::from_str_radix(
        block["timestamp"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches("0x"),
        16,
    )
    .map_err(|_| bad("Invalid timestamp"))?;
    let hash = B256::from_str(block["hash"].as_str().unwrap_or(""))
        .map_err(|_| bad("Invalid block hash"))?;
    let context = ReplayContext {
        sourceChainId: U256::from(143),
        sourceBlock: number,
        sourceBlockHash: hash,
        tokenIn: if p.zero_for_one { Address::ZERO } else { USDC },
        tokenOut: if p.zero_for_one { USDC } else { Address::ZERO },
        amountIn: U256::from(p.amount),
    };
    let ids = replay.iter().map(|q| q.poolId).collect::<Vec<_>>();
    let universe_hash = keccak256((ids,).abi_encode_params());
    let replay_hash = keccak256((universe_hash, context, replay).abi_encode_params());
    let timestamp_ms: u64 = timestamp.try_into().unwrap_or(0);
    let age_ms = now_ms().saturating_sub(timestamp_ms.saturating_mul(1000));
    Ok(
        json!({"version":1,"fresh":age_ms<=5000,"ageMs":age_ms,"algorithm":"max_gross_output","gasIncluded":false,"sourceChainId":143,"sourceBlock":number.to_string(),"sourceBlockHash":hash.to_string(),"sourceTimestamp":timestamp.to_string(),"startedAtMs":frame["startedAtMs"],"capturedAtMs":frame["capturedAtMs"],"durationMs":frame["durationMs"],"available":available,"candidateCount":candidates.len(),"selected":{"pool":pool.to_string(),"amountOutRaw":out.to_string(),"amountOut":units(out,p.output_decimals),"protocol":selected["protocol"]},"replayHash":replay_hash.to_string(),"scenarios":[{"direction":if p.zero_for_one{"MON_TO_USDC"}else{"USDC_TO_MON"},"inputToken":if p.zero_for_one{Address::ZERO.to_string()}else{USDC.to_string()},"outputToken":if p.zero_for_one{USDC.to_string()}else{Address::ZERO.to_string()},"amountIn":p.request.amount,"amountInRaw":p.amount.to_string(),"outputDecimals":p.output_decimals,"quotes":quotes}]}),
    )
}
async fn fresh_quote(s: &AppState, p: &Prepared) -> Result<Value, ApiError> {
    let mut frame = quote_round(s, &p.data).await?;
    let bytes = hex::decode(frame["result"].as_str().unwrap_or(""))
        .map_err(|_| bad("Invalid aggregate result"))?;
    let mut decoded = Aggregate::aggregate3Call::abi_decode_returns(&bytes)
        .map_err(|_| bad("Cannot decode quotes"))?;
    if decoded.len() != p.calls.len() {
        return Err(bad("Quote count mismatch"));
    }
    let failed = decoded
        .iter()
        .enumerate()
        .filter_map(|(i, r)| (!r.success).then_some(i))
        .collect::<Vec<_>>();
    if !failed.is_empty() && failed.len() < decoded.len() {
        let reads=failed.iter().map(|i|{let c=&p.calls[*i];read(s,"eth_call",json!([{"to":c.target.to_string(),"data":hex::encode_prefixed(&c.callData)},frame["block"]["number"]]))});
        let values = futures_util::future::join_all(reads).await;
        for (i, result) in failed.into_iter().zip(values) {
            if let Ok(raw) = result {
                if let Some(data) = raw.as_str().and_then(|v| hex::decode(v).ok()) {
                    decoded[i] = AggregateResult {
                        success: true,
                        returnData: data.into(),
                    }
                }
            }
        }
        if read(
            s,
            "eth_getBlockByNumber",
            json!([frame["block"]["number"], false]),
        )
        .await?["hash"]
            != frame["block"]["hash"]
        {
            return Err(bad("Quote block changed"));
        }
    }
    frame["capturedAtMs"] = json!(now_ms());
    frame["durationMs"] = json!(now_ms() - frame["startedAtMs"].as_u64().unwrap_or(now_ms()));
    decoded_snapshot(p, &frame, &decoded)
}
fn json_response(value: Value) -> Response {
    let mut response = Json(value).into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}
pub async fn quote_get(
    State(s): State<Arc<AppState>>,
    Query(request): Query<QuoteRequest>,
) -> Result<Response, ApiError> {
    Ok(json_response(fresh_quote(&s, &prepare(request)?).await?))
}
pub async fn quote_post(
    State(s): State<Arc<AppState>>,
    Json(request): Json<QuoteRequest>,
) -> Result<Response, ApiError> {
    Ok(json_response(fresh_quote(&s, &prepare(request)?).await?))
}
fn prepared_stream(s: Arc<AppState>, p: Prepared) -> Response {
    let (tx, rx) = mpsc::channel(1);
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {_=tx.closed()=>break,_=timer.tick()=>{}}
            let value = tokio::select! {_=tx.closed()=>break,r=fresh_quote(&s,&p)=>r.unwrap_or_else(|_|json!({"error":"报价刷新失败"}))};
            if tx.send(value).await.is_err() {
                break;
            }
        }
    });
    response(rx)
}
pub async fn quotes_stream_get(
    State(s): State<Arc<AppState>>,
    Query(r): Query<QuoteRequest>,
) -> Result<Response, ApiError> {
    Ok(prepared_stream(s, prepare(r)?))
}
pub async fn quotes_stream_post(
    State(s): State<Arc<AppState>>,
    Json(r): Json<QuoteRequest>,
) -> Result<Response, ApiError> {
    Ok(prepared_stream(s, prepare(r)?))
}
pub async fn pools() -> Json<Value> {
    let mut value = catalog().clone();
    for q in value["candidates"].as_array_mut().into_iter().flatten() {
        if let Some(row) = q.as_object_mut() {
            for k in ["status", "amountOutRaw", "amountOut", "quoterGasEstimate"] {
                row.remove(k);
            }
        }
    }
    Json(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(amount: &str, token: &str) -> QuoteRequest {
        QuoteRequest {
            amount: amount.into(),
            token_in: token.into(),
            token_out: None,
        }
    }
    #[test]
    fn amounts_and_pair_are_strict() {
        assert_eq!(parse_amount("0.1", 18).unwrap(), 100_000_000_000_000_000);
        assert_eq!(parse_amount("0.1", 6).unwrap(), 100_000);
        for text in ["0", "-1", "NaN", "1e2", "01", "1.", ".1", "1.0000001"] {
            assert!(parse_amount(text, 6).is_err(), "{text}")
        }
        assert!(prepare(request("0.1", "WBTC")).is_err());
        let mut wrong = request("0.1", "MON");
        wrong.token_out = Some("MON".into());
        assert!(prepare(wrong).is_err());
        assert!(serde_json::from_str::<QuoteRequest>(r#"{"amount":0.1}"#).is_err());
        assert!(serde_json::from_str::<QuoteRequest>(r#"{"amount":"0.1","data":"0x"}"#).is_err());
    }
    #[test]
    fn builds_fixed_catalog_in_both_directions() {
        let forward = prepare(request("0.1", "MON")).unwrap();
        let reverse = prepare(request("0.1", "USDC")).unwrap();
        assert_eq!(forward.calls.len(), 14);
        assert_eq!(reverse.calls.len(), 14);
        assert!(forward.data.starts_with("0x82ad56cb"));
        assert_ne!(forward.data, reverse.data);
    }
    #[test]
    fn decimal_format_does_not_round() {
        assert_eq!(units(U256::from(2277), 6), "0.002277");
        assert_eq!(units(U256::from(1000000), 6), "1");
        assert_eq!(units(U256::ZERO, 18), "0");
    }
    #[test]
    fn mixed_quotes_rank_and_mark_failures() {
        let p = prepare(request("0.1", "MON")).unwrap();
        let pools = catalog()["candidates"].as_array().unwrap();
        let mut results = pools
            .iter()
            .enumerate()
            .map(|(i, q)| {
                let out = U256::from(1000 + i);
                let raw = match q["kind"].as_str().unwrap() {
                    "v4" => (out, U256::ZERO).abi_encode_params(),
                    "v3" => (out, alloy_primitives::aliases::U160::ZERO, 0u32, U256::ZERO)
                        .abi_encode_params(),
                    _ => (
                        alloy_primitives::aliases::U112::from(100_000_000_000_000_000_000u128),
                        alloy_primitives::aliases::U112::from(3_000_000u64),
                        0u32,
                    )
                        .abi_encode_params(),
                };
                AggregateResult {
                    success: true,
                    returnData: raw.into(),
                }
            })
            .collect::<Vec<_>>();
        let frame = json!({"block":{"number":"0x1234","hash":format!("0x{:064x}",1),"timestamp":"0x65000000"},"startedAtMs":1,"capturedAtMs":2,"durationMs":1});
        let result = decoded_snapshot(&p, &frame, &results).unwrap();
        assert_eq!(result["available"], 14);
        assert_eq!(result["selected"]["amountOutRaw"], "2988");
        assert_eq!(result["selected"]["protocol"], "Uniswap V2");
        assert_eq!(result["sourceBlock"], "4660");
        assert_eq!(result["fresh"], false);
        results[0].success = false;
        assert_eq!(
            decoded_snapshot(&p, &frame, &results).unwrap()["available"],
            13
        );
        for r in &mut results {
            r.success = false
        }
        assert!(decoded_snapshot(&p, &frame, &results).is_err());
    }
}
