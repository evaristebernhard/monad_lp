//! Incremental Monad polling for the fixed demo registry plus the active display.
use crate::{
    forward::forward_impl,
    monad_store as store,
    state::{AppState, RpcQuery},
};
use anyhow::{bail, Context, Result};
use axum::{
    body::{to_bytes, Bytes},
    extract::Query,
    http::StatusCode,
    Json,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use tiny_keccak::{Hasher, Keccak};
const V2_SWAP: &str = "Swap(address,uint256,uint256,uint256,uint256,address)";
const V2_MINT: &str = "Mint(address,uint256,uint256)";
const V2_BURN: &str = "Burn(address,uint256,uint256,address)";
const V3_SWAP: &str = "Swap(address,address,int256,int256,uint160,uint128,int24)";
const PANCAKE_V3_SWAP: &str =
    "Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)";
const V3_MINT: &str = "Mint(address,address,int24,int24,uint128,uint256,uint256)";
const V3_BURN: &str = "Burn(address,int24,int24,uint128,uint256,uint256)";
const V4_SWAP: &str = "Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24)";
const V4_LIQ: &str = "ModifyLiquidity(bytes32,address,int24,int24,int256,bytes32)";
#[derive(Clone)]
struct PoolMetadata {
    a: String,
    b: String,
    d0: i32,
    d1: i32,
    fee: Option<f64>,
}
static METADATA: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, PoolMetadata>>,
> = std::sync::OnceLock::new();
static HEAD: std::sync::OnceLock<tokio::sync::Mutex<Option<(std::time::Instant, Value)>>> =
    std::sync::OnceLock::new();
const ZERO: &str = "0x0000000000000000000000000000000000000000";
fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn topic(s: &str) -> String {
    let mut h = Keccak::v256();
    let mut b = [0u8; 32];
    h.update(s.as_bytes());
    h.finalize(&mut b);
    format!(
        "0x{}",
        b.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}
fn selector(s: &str) -> String {
    topic(s)[..10].to_owned()
}
fn hex_i(v: &Value) -> Result<i64> {
    Ok(i64::from_str_radix(
        v.as_str()
            .context("missing hex integer")?
            .trim_start_matches("0x"),
        16,
    )?)
}
// Accumulate the magnitude directly, including two's complement, to avoid
// losing small negative int256 values through f64 cancellation.
fn number(word: &str, signed: bool) -> Result<f64> {
    if word.len() != 64 || !word.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("invalid ABI word")
    }
    let negative = signed && word.as_bytes()[0].to_ascii_lowercase() >= b'8';
    let mut bytes = Vec::with_capacity(32);
    for i in (0..64).step_by(2) {
        bytes.push(u8::from_str_radix(&word[i..i + 2], 16)?)
    }
    if negative {
        let mut carry = 1u16;
        for b in bytes.iter_mut().rev() {
            let n = (!*b) as u16 + carry;
            *b = n as u8;
            carry = n >> 8;
        }
    }
    let value = bytes.iter().fold(0., |v, b| v * 256. + *b as f64);
    Ok(if negative { -value } else { value })
}
fn words(data: &str) -> Result<Vec<&str>> {
    let data = data.strip_prefix("0x").context("missing ABI data")?;
    if data.len() % 64 != 0 || !data.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("malformed ABI data")
    };
    Ok((0..data.len())
        .step_by(64)
        .map(|i| &data[i..i + 64])
        .collect())
}
fn topic_address(v: &Value) -> Option<String> {
    let t = v.as_str()?;
    (t.len() == 66).then(|| format!("0x{}", &t[26..]))
}
fn signatures(kind: &str) -> Vec<String> {
    match kind {
        "uniswap_v4" => vec![topic(V4_SWAP), topic(V4_LIQ)],
        "concentrated_v3" => vec![
            topic(V3_SWAP),
            topic(PANCAKE_V3_SWAP),
            topic(V3_MINT),
            topic(V3_BURN),
        ],
        _ => vec![topic(V2_SWAP), topic(V2_MINT), topic(V2_BURN)],
    }
}
fn log_filter(pool: &Value, from: i64, to: i64) -> Value {
    let kind = text(pool, "pool_type");
    let mut topics = json!([signatures(kind)]);
    let address = if kind == "uniswap_v4" {
        topics
            .as_array_mut()
            .unwrap()
            .push(pool["pool_address"].clone());
        manager()
    } else {
        text(pool, "pool_address").to_owned()
    };
    json!({"address":address,"fromBlock":format!("0x{from:x}"),"toBlock":format!("0x{to:x}"),"topics":topics})
}
fn manager() -> String {
    std::env::var("MONAD_POOL_MANAGER")
        .unwrap_or_else(|_| "0x188d586ddcf52439676ca21a244753fa19f9ea8e".into())
}
async fn rpc(s: &AppState, method: &str, params: Value) -> Result<Value> {
    if method == "eth_blockNumber" {
        let mut cached = HEAD
            .get_or_init(|| tokio::sync::Mutex::new(None))
            .lock()
            .await;
        if let Some((at, value)) = &*cached {
            if at.elapsed() < std::time::Duration::from_millis(500) {
                return Ok(value.clone());
            }
        }
        let value = rpc_mode(s, method, params, false).await?;
        *cached = Some((std::time::Instant::now(), value.clone()));
        return Ok(value);
    }
    rpc_mode(s, method, params, false).await
}
async fn rpc_mode(s: &AppState, method: &str, params: Value, historical: bool) -> Result<Value> {
    let result = rpc_at(s, method, params.clone(), historical, None).await;
    // A known block/state returning JSON null is a provider data gap, not a
    // successful price observation. Try other routed providers without changing
    // the requested block or substituting the current state.
    if result
        .as_ref()
        .is_err_and(|e| e.to_string().contains("null/missing result"))
    {
        if let Some(route) = s.routes.get("monad") {
            for endpoint in &route.endpoints {
                if historical && endpoint.supports_historical == Some(false) {
                    continue;
                }
                if let Ok(value) = rpc_at(
                    s,
                    method,
                    params.clone(),
                    historical,
                    Some(endpoint.title.clone()),
                )
                .await
                {
                    return Ok(value);
                }
            }
        }
    }
    result
}
async fn rpc_at(
    s: &AppState,
    method: &str,
    params: Value,
    historical: bool,
    title: Option<String>,
) -> Result<Value> {
    let body = Bytes::from(serde_json::to_vec(
        &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
    )?);
    let response = forward_impl(
        s.clone(),
        "monad".into(),
        RpcQuery { title },
        body,
        historical,
    )
    .await;
    let status = response.status();
    let b = to_bytes(response.into_body(), 16 * 1024 * 1024).await?;
    if !status.is_success() {
        bail!(
            "RPC {method}: {status}: {}",
            String::from_utf8_lossy(&b)
                .chars()
                .take(1024)
                .collect::<String>()
        )
    }
    let v: Value = serde_json::from_slice(&b)?;
    if !v["error"].is_null() {
        bail!("RPC {method}: {}", v["error"])
    }
    v.get("result")
        .filter(|r| !r.is_null())
        .cloned()
        .with_context(|| format!("RPC {method}: null/missing result for {params}"))
}
// Monad providers commonly cap batches at ten. Fall back to routed single
// reads if a provider does not support batching, while preserving range atomicity.
async fn batch(s: &AppState, requests: &[(String, Value)]) -> Result<Vec<Value>> {
    let result:Result<Vec<Value>>=async {
        let calls=requests.iter().enumerate().map(|(i,(method,params))|json!({"jsonrpc":"2.0","id":i+1,"method":method,"params":params})).collect::<Vec<_>>();
        let response=forward_impl(s.clone(),"monad".into(),RpcQuery{title:None},Bytes::from(serde_json::to_vec(&calls)?),false).await;
        if !response.status().is_success(){bail!("batch transport failed")}
        let bytes=to_bytes(response.into_body(),16*1024*1024).await?;
        let rows:Value=serde_json::from_slice(&bytes)?;
        let rows=rows.as_array().context("provider does not support batches")?;
        let mut results=Vec::new();
        for i in 0..requests.len() {
            let row=rows.iter().find(|r|r["id"].as_u64()==Some((i+1)as u64)).context("batch item missing")?;
            if !row["error"].is_null()||row["result"].is_null(){bail!("batch item unavailable")}
            results.push(row["result"].clone());
        }
        Ok(results)
    }.await;
    match result {
        Ok(v) => Ok(v),
        Err(_) => futures_util::future::try_join_all(
            requests.iter().map(|(method,params)|rpc(s,method,params.clone()))
        ).await,
    }
}
async fn block(s: &AppState, n: i64) -> Result<Value> {
    block_mode(s, n, false).await
}
async fn block_mode(s: &AppState, n: i64, historical: bool) -> Result<Value> {
    rpc_mode(
        s,
        "eth_getBlockByNumber",
        json!([format!("0x{n:x}"), false]),
        historical,
    )
    .await
}
async fn call(
    s: &AppState,
    to: &str,
    data: String,
    block: i64,
    historical: bool,
) -> Result<String> {
    Ok(rpc_mode(
        s,
        "eth_call",
        json!([{"to":to,"data":data},format!("0x{block:x}")]),
        historical,
    )
    .await?
    .as_str()
    .context("call result")?
    .to_owned())
}
async fn decimals(s: &AppState, token: &str, n: i64, historical: bool) -> Result<i32> {
    if token.eq_ignore_ascii_case(ZERO) {
        return Ok(18);
    }
    let raw = call(s, token, selector("decimals()"), n, historical).await?;
    let d = number(words(&raw)?.first().context("decimals missing")?, false)?;
    if d > 36. {
        bail!("invalid decimals")
    };
    Ok(d as i32)
}
// Trusted token addresses, not symbol strings, anchor USD estimates.
fn usd_anchor(token: &str) -> bool {
    let anchors = std::env::var("MONAD_USD_ANCHORS")
        .unwrap_or_else(|_| "0x754704bc059f8c67012fed69bc8a327a5aafb603".into());
    anchors
        .split(',')
        .any(|a| a.trim().eq_ignore_ascii_case(token))
}
async fn refresh(s: &AppState, pool: &Value, n: i64) -> Result<()> {
    refresh_at(s, pool, n, false).await
}
async fn refresh_at(s: &AppState, pool: &Value, n: i64, historical: bool) -> Result<()> {
    let kind = text(pool, "pool_type");
    let id = text(pool, "pool_address");
    let head = block_mode(s, n, historical).await?;
    let as_of = hex_i(&head["timestamp"])?;
    let cached = if historical {
        None
    } else {
        METADATA
            .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
            .lock()
            .unwrap()
            .get(id)
            .cloned()
    };
    let metadata = if let Some(metadata) = cached {
        metadata
    } else {
        let (a, b) = if kind == "uniswap_v4" {
            let a = text(pool, "token_a_address").to_owned();
            let b = text(pool, "token_b_address").to_owned();
            // V4 PoolKey currencies are sorted by address, regardless of catalog display order.
            if a.to_ascii_lowercase() < b.to_ascii_lowercase() {
                (a, b)
            } else {
                (b, a)
            }
        } else {
            let x = call(s, id, selector("token0()"), n, historical).await?;
            let y = call(s, id, selector("token1()"), n, historical).await?;
            (
                topic_address(&json!(x)).context("token0")?,
                topic_address(&json!(y)).context("token1")?,
            )
        };
        let d0 = decimals(s, &a, n, historical).await?;
        let d1 = decimals(s, &b, n, historical).await?;
        let fee = if kind == "concentrated_v3" {
            let value = call(s, id, selector("fee()"), n, historical).await?;
            Some(number(words(&value)?.first().context("fee missing")?, false)? / 100.)
        } else if kind == "uniswap_v4" {
            None
        } else {
            pool["fee_bps"].as_f64()
        };
        let metadata = PoolMetadata { a, b, d0, d1, fee };
        if !historical {
            METADATA
                .get()
                .unwrap()
                .lock()
                .unwrap()
                .insert(id.to_owned(), metadata.clone());
        }
        metadata
    };
    let PoolMetadata {
        a,
        b,
        d0,
        d1,
        mut fee,
    } = metadata;
    let ratio = if kind == "constant_product_v2" || kind == "solidly_v2" {
        let raw = call(s, id, selector("getReserves()"), n, historical).await?;
        let w = words(&raw)?;
        if w.len() < 2 {
            bail!("reserves missing")
        };
        let r0 = number(w[0], false)?;
        let r1 = number(w[1], false)?;
        if r0 <= 0. || r1 <= 0. {
            bail!("empty reserves")
        };
        r1 / r0 * 10f64.powi(d0 - d1)
    } else {
        let raw = if kind == "uniswap_v4" {
            call(
                s,
                "0x77395f3b2e73ae90843717371294fa97cc419d64",
                format!(
                    "{}{}",
                    selector("getSlot0(bytes32)"),
                    id.trim_start_matches("0x")
                ),
                n,
                historical,
            )
            .await?
        } else {
            call(s, id, selector("slot0()"), n, historical).await?
        };
        let w = words(&raw)?;
        let sqrt = number(w.first().context("slot0 missing")?, false)?;
        if kind == "uniswap_v4" && w.len() >= 4 {
            let raw_fee = number(w[3], false)?;
            if raw_fee < 8388608. {
                fee = Some(raw_fee / 100.)
            }
        }
        (sqrt / 2f64.powi(96)).powi(2) * 10f64.powi(d0 - d1)
    };
    if !ratio.is_finite() || ratio <= 0. {
        bail!("invalid price")
    }
    let (p0, p1) = if usd_anchor(&a) {
        (Some(1.), Some(1. / ratio))
    } else if usd_anchor(&b) {
        (Some(ratio), Some(1.))
    } else {
        (None, None)
    };
    let q = json!({"token0_address":a,"token1_address":b,"decimals0":d0,"decimals1":d1,"token1_per_token0":ratio,"token0_price_usd":p0,"token1_price_usd":p1,"as_of":as_of,"state_block":n,"fee_bps":fee,"usd_anchor_status":if p0.is_some(){"trusted_stable_assumption"}else{"unavailable"}});
    if block_mode(s, n, historical).await?["hash"] != head["hash"] {
        bail!("state block replaced during decoding")
    }
    let snapshot =
        json!({"pool_address":id,"refreshed_at_ms":as_of*1000,"snapshot":{"decoded_price":q}});
    if !historical {
        store::open()?.execute(
        "INSERT INTO snapshots VALUES(?1,?2) ON CONFLICT(pool) DO UPDATE SET body=excluded.body WHERE coalesce(json_extract(excluded.body,'$.snapshot.decoded_price.state_block'),0)>=coalesce(json_extract(snapshots.body,'$.snapshot.decoded_price.state_block'),0)",
        params![id, snapshot.to_string()],
    )?;
    }
    // Existing hourly pool history remains the common source for backend volatility.
    let catalog = catalog_path();
    let price = std::env::var_os("LP_PRICE_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| catalog.parent().unwrap().join("lp_price_history.db"));
    let c = rusqlite::Connection::open(price)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    c.execute_batch("CREATE TABLE IF NOT EXISTS pool_price_history(chain TEXT NOT NULL,pool_address TEXT NOT NULL,timestamp INTEGER NOT NULL,token0_address TEXT NOT NULL,token1_address TEXT NOT NULL,token1_per_token0 REAL,token0_price_usd REAL,token1_price_usd REAL,source TEXT NOT NULL,price_status TEXT NOT NULL DEFAULT 'priced',status_reason TEXT,observed_at INTEGER NOT NULL,PRIMARY KEY(chain,pool_address,timestamp));")?;
    c.execute("INSERT INTO pool_price_history(chain,pool_address,timestamp,token0_address,token1_address,token1_per_token0,token0_price_usd,token1_price_usd,source,observed_at) VALUES('monad',?1,?2,?3,?4,?5,?6,?7,'monad_listener_rpc',?8) ON CONFLICT(chain,pool_address,timestamp) DO UPDATE SET token1_per_token0=excluded.token1_per_token0,token0_price_usd=excluded.token0_price_usd,token1_price_usd=excluded.token1_price_usd,source=excluded.source,price_status='priced',status_reason=NULL,observed_at=excluded.observed_at WHERE excluded.observed_at>=pool_price_history.observed_at",params![id,as_of/3600*3600,a,b,ratio,p0,p1,as_of])?;
    // Live V4 fees are authoritative for the existing APR SQL.
    if let Some(fee) = fee.filter(|_| !historical) {
        let c = rusqlite::Connection::open(catalog)?;
        c.busy_timeout(std::time::Duration::from_secs(5))?;
        c.execute("UPDATE pool_rpc_state SET fee_bps=?2,checked_at=?3 WHERE pool_address=?1 AND chain='monad'",params![id,fee,as_of])?;
    }
    Ok(())
}
fn catalog_path() -> std::path::PathBuf {
    std::env::var_os("MONAD_POOL_DB")
        .or_else(|| std::env::var_os("LP_SQLITE_PATH"))
        .map(Into::into)
        .unwrap_or_else(|| {
            if std::path::Path::new("../data/lp_screener.db").exists() {
                "../data/lp_screener.db".into()
            } else {
                "data/lp_screener.db".into()
            }
        })
}
#[derive(Debug)]
struct Decoded {
    kind: &'static str,
    amount0: Option<f64>,
    amount1: Option<f64>,
    liquidity: Option<f64>,
    sender: Option<String>,
}
fn decode(log: &Value, kind: &str, d0: i32, d1: i32) -> Result<Decoded> {
    let signature = log["topics"][0].as_str().context("event signature")?;
    let w = words(text(log, "data"))?;
    let (data_words, topic_count) = if signature == topic(V2_SWAP) {
        (4, 3)
    } else if signature == topic(V2_MINT) {
        (2, 2)
    } else if signature == topic(V2_BURN) {
        (2, 3)
    } else if signature == topic(V3_SWAP) {
        (5, 3)
    } else if signature == topic(PANCAKE_V3_SWAP) {
        (7, 3)
    } else if signature == topic(V3_MINT) {
        (4, 4)
    } else if signature == topic(V3_BURN) {
        (3, 4)
    } else if signature == topic(V4_SWAP) {
        (6, 3)
    } else if signature == topic(V4_LIQ) {
        (4, 3)
    } else {
        bail!("unrecognized event")
    };
    if w.len() != data_words || log["topics"].as_array().map_or(0, |t| t.len()) != topic_count {
        bail!("event ABI shape mismatch")
    }
    let num = |i: usize, signed: bool| -> Result<f64> {
        number(w.get(i).context("short ABI event")?, signed)
    };
    let mut d = Decoded {
        kind: "unknown",
        amount0: None,
        amount1: None,
        liquidity: None,
        sender: topic_address(&log["topics"][if kind == "uniswap_v4" { 2 } else { 1 }]),
    };
    if signature == topic(V2_SWAP) {
        d.kind = "swap";
        d.amount0 = Some(num(2, false)? - num(0, false)?);
        d.amount1 = Some(num(3, false)? - num(1, false)?);
    } else if signature == topic(V3_SWAP)
        || signature == topic(PANCAKE_V3_SWAP)
        || signature == topic(V4_SWAP)
    {
        d.kind = "swap";
        let sign = if kind == "uniswap_v4" { 1. } else { -1. };
        d.amount0 = Some(sign * num(0, true)?);
        d.amount1 = Some(sign * num(1, true)?);
    } else if signature == topic(V4_LIQ) {
        let delta = num(2, true)?;
        d.kind = if delta > 0. {
            "add_liquidity"
        } else if delta < 0. {
            "remove_liquidity"
        } else {
            "liquidity_update"
        };
        d.liquidity = Some(delta);
    } else if signature == topic(V2_MINT) || signature == topic(V2_BURN) {
        d.kind = if signature == topic(V2_MINT) {
            "add_liquidity"
        } else {
            "remove_liquidity"
        };
        d.amount0 = Some(num(0, false)?);
        d.amount1 = Some(num(1, false)?);
    } else if signature == topic(V3_MINT) || signature == topic(V3_BURN) {
        let mint = signature == topic(V3_MINT);
        d.kind = if mint {
            "add_liquidity"
        } else {
            "remove_liquidity"
        };
        let offset = if mint { 1 } else { 0 };
        d.liquidity = Some(num(offset, false)?);
        d.amount0 = Some(num(offset + 1, false)?);
        d.amount1 = Some(num(offset + 2, false)?);
        if mint {
            d.sender = Some(format!("0x{}", &w[0][24..]));
        }
    } else {
        bail!("unrecognized event")
    }
    d.amount0 = d.amount0.map(|a| a / 10f64.powi(d0));
    d.amount1 = d.amount1.map(|a| a / 10f64.powi(d1));
    Ok(d)
}
fn enrich(
    pool: &Value,
    token: &str,
    log: &Value,
    q: &Value,
    d: Decoded,
    timestamp: i64,
    from: &Value,
    tx_to: &Value,
) -> Value {
    let target0 = text(q, "token0_address").eq_ignore_ascii_case(token);
    let (ta, qa) = if target0 {
        (d.amount0, d.amount1)
    } else {
        (d.amount1, d.amount0)
    };
    let tp = if target0 {
        q["token0_price_usd"].as_f64()
    } else {
        q["token1_price_usd"].as_f64()
    };
    let qp = if target0 {
        q["token1_price_usd"].as_f64()
    } else {
        q["token0_price_usd"].as_f64()
    };
    // Only a recent, on-chain stable anchor is accepted as a dollar valuation.
    let reliable = text(q, "usd_anchor_status") == "trusted_stable_assumption"
        && q["as_of"]
            .as_i64()
            .is_some_and(|t| (timestamp - t).abs() <= 30);
    let notional = if reliable {
        if d.kind == "swap" {
            ta.zip(tp)
                .map(|(a, p)| a.abs() * p)
                .or_else(|| qa.zip(qp).map(|(a, p)| a.abs() * p))
        } else {
            ta.zip(tp)
                .zip(qa.zip(qp))
                .map(|((a, p), (b, r))| a.abs() * p + b.abs() * r)
        }
    } else {
        None
    };
    let threshold = std::env::var("MONAD_LISTENER_WHALE_USD")
        .ok()
        .and_then(|x| x.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.)
        .unwrap_or(10_000.);
    let side = if d.kind == "swap" && ta.zip(qa).is_some_and(|(a, b)| a * b < 0.) {
        if ta.unwrap() > 0. {
            "buy"
        } else {
            "sell"
        }
    } else {
        "unknown"
    };
    let quote = if target0 {
        &q["token1_address"]
    } else {
        &q["token0_address"]
    };
    let ratio = ta
        .zip(qa)
        .filter(|(a, b)| *a != 0. && *b != 0.)
        .map(|(a, b)| (b / a).abs());
    let trade = json!({"side":side,"action":side,"target_mint":token,"quote_mint":quote,"target_amount_ui":ta.map(f64::abs),"quote_amount_ui":qa.map(f64::abs),"swap_ratio_quote_per_target":if d.kind=="swap"{ratio}else{None},"swap_price_usd":ratio.zip(qp).filter(|_|reliable).map(|(r,p)|r*p),"notional_usd":notional,"trader":from,"router":tx_to,"event_sender":d.sender});
    json!({"watched_pool":pool["pool_address"],"watched_protocol":pool["protocol"],"signature":log["transactionHash"],"transaction_hash":log["transactionHash"],"block_number":log["blockNumber"],"block_hash":log["blockHash"],"log_index":log["logIndex"],"observed_at_ms":timestamp*1000,"token":token,"router":tx_to,"event_sender":d.sender,"transaction_from":from,"transaction_to":tx_to,"whale":notional.map(|v|v>=threshold),"raw_log":log,"whale_status":if notional.is_some(){"valued"}else{"unknown_usd_value"},"enrichment":{"transaction":{"event_kind":d.kind,"trade":trade,"liquidity":{"detected":d.kind!="swap","delta_raw":d.liquidity,"direction":if d.kind=="remove_liquidity"{"withdraw"}else{"deposit"},"target_mint":token,"quote_mint":quote,"target_amount_ui":ta.map(f64::abs),"quote_amount_ui":qa.map(f64::abs)}}}})
}
fn log_range_rejected(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    // Some providers return only HTTP 400/413 for a range they refuse.
    message.contains("400 bad request")
        || message.contains("413 payload too large")
        || message.contains("block range")
        || message.contains("range too large")
        || message.contains("too many results")
        || message.contains("response size")
        || message.contains("query returned more")
}
async fn poll(s: &AppState, pool: &Value, token: &str, head: i64, span: &mut i64) -> Result<()> {
    let id = text(pool, "pool_address");
    let cursor: Option<(i64, String)> = store::open()?
        .query_row("SELECT block,hash FROM cursors WHERE pool=?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let mut from = cursor.as_ref().map(|v| v.0 + 1).unwrap_or(head);
    // Walk persisted checkpoints back to the common ancestor, even after a long outage.
    if let Some((n, hash)) = &cursor {
        if text(&block(s, *n).await?, "hash") != hash {
            let points = {
                let checkpoint_db = store::open()?;
                let mut stmt = checkpoint_db.prepare(
                    "SELECT block,hash FROM checkpoints WHERE pool=?1 ORDER BY block DESC",
                )?;
                let values = stmt
                    .query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                values
            };
            from = 0;
            for (n, h) in points {
                if text(&block(s, n).await?, "hash") == h {
                    from = n + 1;
                    break;
                }
            }
            let previous = if from > 0 {
                block(s, from - 1).await?
            } else {
                json!({"hash":""})
            };
            let c = store::open()?;
            let tx = c.unchecked_transaction()?;
            tx.execute(
                "DELETE FROM events WHERE pool=?1 AND block>=?2",
                params![id, from],
            )?;
            tx.execute(
                "DELETE FROM checkpoints WHERE pool=?1 AND block>=?2",
                params![id, from],
            )?;
            tx.execute(
                "UPDATE cursors SET block=?2,hash=?3 WHERE pool=?1",
                params![id, from - 1, text(&previous, "hash")],
            )?;
            tx.commit()?;
        }
    }
    if cursor.is_none() {
        let initial = block(s, (head - 1).max(0)).await?;
        let c = store::open()?;
        let tx = c.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO cursors(pool,block,hash,started_at) VALUES(?1,?2,?3,?4)",
            params![
                id,
                (head - 1).max(0),
                text(&initial, "hash"),
                hex_i(&initial["timestamp"])?
            ],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO checkpoints VALUES(?1,?2,?3)",
            params![id, (head - 1).max(0), text(&initial, "hash")],
        )?;
        tx.commit()?;
    }
    if from > head {
        return Ok(());
    }
    let (to, end, logs) = loop {
        let to = (from + *span - 1).min(head);
        let end = block(s, to).await?;
        match rpc(s, "eth_getLogs", json!([log_filter(pool, from, to)])).await {
            Ok(logs) => break (to, end, logs),
            Err(error) if to > from && log_range_rejected(&error) => {
                // Retry from the same cursor. Remember the smaller span for this
                // pool so subsequent polls do not repeat rejected large requests.
                *span = ((to - from + 1) / 2).max(1);
            }
            Err(error) => bail!("pool {id}, blocks {from}..={to}: {error}"),
        }
    };
    let logs = logs.as_array().context("logs result is not an array")?;
    let raw: Option<String> = store::open()?
        .query_row("SELECT body FROM snapshots WHERE pool=?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    let snapshot: Value =
        serde_json::from_str(raw.as_deref().context("waiting for pool metadata")?)?;
    let q = &snapshot["snapshot"]["decoded_price"];
    let d0 = q["decimals0"].as_i64().context("decimals0 missing")? as i32;
    let d1 = q["decimals1"].as_i64().context("decimals1 missing")? as i32;
    let numbers = logs
        .iter()
        .map(|l| hex_i(&l["blockNumber"]))
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    let hashes = logs
        .iter()
        .map(|l| text(l, "transactionHash").to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    let mut keys = Vec::new();
    let mut requests = Vec::new();
    for n in numbers {
        if n < from || n > to {
            bail!("log out of range")
        };
        keys.push(format!("block:{n}"));
        requests.push((
            "eth_getBlockByNumber".to_owned(),
            json!([format!("0x{n:x}"), false]),
        ));
    }
    for hash in hashes {
        if hash.is_empty() {
            bail!("transaction hash missing")
        };
        keys.push(format!("tx:{hash}"));
        requests.push(("eth_getTransactionByHash".to_owned(), json!([hash])));
    }
    let mut details = std::collections::HashMap::new();
    for (i, chunk) in requests.chunks(10).enumerate() {
        for (j, v) in batch(s, chunk).await?.into_iter().enumerate() {
            details.insert(keys[i * 10 + j].clone(), v);
        }
    }
    let mut values = Vec::new();
    for log in logs {
        let expected = if text(pool, "pool_type") == "uniswap_v4" {
            manager()
        } else {
            id.to_owned()
        };
        if !text(log, "address").eq_ignore_ascii_case(&expected) {
            bail!("log address outside pool filter")
        }
        if text(pool, "pool_type") == "uniswap_v4"
            && !log["topics"][1]
                .as_str()
                .is_some_and(|t| t.eq_ignore_ascii_case(id))
        {
            bail!("log pool ID outside filter")
        }
        if log["removed"] == true {
            bail!("removed log; retry range")
        }
        let n = hex_i(&log["blockNumber"])?;
        if n < from || n > to {
            bail!("log out of range")
        }
        let header = details
            .get(&format!("block:{n}"))
            .context("block detail missing")?;
        if header["hash"] != log["blockHash"] {
            bail!("replaced block while collecting")
        }
        let timestamp = hex_i(&header["timestamp"])?;
        let d = decode(log, text(pool, "pool_type"), d0, d1)?;
        let transaction = details
            .get(&format!("tx:{}", text(log, "transactionHash")))
            .context("transaction detail missing")?;
        let event = enrich(
            pool,
            token,
            log,
            q,
            d,
            timestamp,
            &transaction["from"],
            &transaction["to"],
        );
        values.push((n, hex_i(&log["logIndex"])?, timestamp, event));
    }
    if block(s, to).await?["hash"] != end["hash"] {
        bail!("range replaced before commit")
    }
    let c = store::open()?;
    let tx = c.unchecked_transaction()?;
    for (n, index, timestamp, v) in values {
        tx.execute("INSERT INTO events(pool,block,block_hash,tx,log_index,timestamp,whale,body) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(pool,tx,log_index) DO UPDATE SET block=excluded.block,block_hash=excluded.block_hash,timestamp=excluded.timestamp,whale=excluded.whale,body=excluded.body",params![id,n,text(&v,"block_hash"),text(&v,"signature"),index,timestamp,v["whale"].as_bool(),v.to_string()])?;
    }
    tx.execute("INSERT INTO checkpoints VALUES(?1,?2,?3) ON CONFLICT(pool,block) DO UPDATE SET hash=excluded.hash",params![id,to,text(&end,"hash")])?;
    tx.execute("INSERT INTO cursors(pool,block,hash,started_at,error) VALUES(?1,?2,?3,?4,NULL) ON CONFLICT(pool) DO UPDATE SET block=excluded.block,hash=excluded.hash,error=NULL",params![id,to,text(&end,"hash"),hex_i(&end["timestamp"]) ?])?;
    tx.commit()?;
    Ok(())
}
async fn block_at_or_after(s: &AppState, head: i64, timestamp: i64) -> Result<i64> {
    // Same recent-block estimate + verified bracket used by the existing
    // price-history probe. Only the requested 24-hour window is supported.
    let latest = block_mode(s, head, true).await?;
    let latest_time = hex_i(&latest["timestamp"])?;
    if timestamp > latest_time || latest_time - timestamp > 86400 {
        bail!("hourly backfill target outside recent 24-hour window")
    }
    let sample_n = (head - 1000).max(0);
    if sample_n == head {
        return Ok(head);
    }
    let sample = block_mode(s, sample_n, true).await?;
    let speed =
        ((latest_time - hex_i(&sample["timestamp"])?) as f64 / (head - sample_n) as f64).max(0.01);
    let distance =
        (((latest_time - timestamp) as f64 / speed * 1.15).ceil() as i64 + 1000).min(head);
    let (mut low, mut high) = ((head - distance).max(0), head);
    let mut bracketed = false;
    for _ in 0..4 {
        if hex_i(&block_mode(s, low, true).await?["timestamp"])? <= timestamp {
            bracketed = true;
            break;
        }
        low = (head - (head - low) * 2).max(0);
    }
    if !bracketed {
        bail!("cannot bracket requested hour from recent blocks")
    }
    while low < high {
        let mid = low + (high - low) / 2;
        if hex_i(&block_mode(s, mid, true).await?["timestamp"])? < timestamp {
            low = mid + 1
        } else {
            high = mid
        }
    }
    Ok(low)
}
async fn backfill_one(s: &AppState) -> Result<()> {
    let config = store::display(&store::open()?)?;
    let mut pools = store::fixed_pools()?;
    for pool in config["pools"].as_array().into_iter().flatten() {
        if !pools.iter().any(|p| p["pool_address"] == pool["pool_address"]) {
            pools.push(pool.clone());
        }
    }
    if pools.is_empty() {
        return Ok(());
    }
    let hour = store::now() / 3600 * 3600;
    let catalog = catalog_path();
    let price = std::env::var_os("LP_PRICE_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| catalog.parent().unwrap().join("lp_price_history.db"));
    if !price.exists() {
        return Ok(());
    }
    // Finish one hour for all selected pools, before moving to another hour.
    let mut missing = None;
    {
        let c = rusqlite::Connection::open_with_flags(
            price,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        for step in 1..=24 {
            let timestamp = hour - step * 3600;
            let gaps=pools.iter().filter_map(|p| {
                let row=c.query_row("SELECT 1 FROM pool_price_history WHERE chain='monad' AND pool_address=?1 AND timestamp=?2 AND price_status='priced' AND token1_per_token0>0",params![text(p,"pool_address"),timestamp],|r|r.get::<_,i32>(0)).optional();
                match row {Ok(Some(_))=>None,_=>Some(p.clone())}
            }).collect::<Vec<_>>();
            if !gaps.is_empty() {
                missing = Some((timestamp, gaps));
                break;
            }
        }
    }
    let Some((timestamp, gaps)) = missing else {
        return Ok(());
    };
    let head = (hex_i(&rpc(s, "eth_blockNumber", json!([])).await?)? - 2).max(0);
    // Sample near the end of the completed hour, keeping the oldest
    // required hour inside the recent 24-hour archive window.
    let n = block_at_or_after(s, head, timestamp + 3590).await?;
    if hex_i(&block(s, n).await?["timestamp"])? / 3600 * 3600 != timestamp {
        bail!("no block in requested historical hour")
    }
    for p in gaps {
        if store::display(&store::open()?)?["version"] != config["version"] {
            break;
        }
        // One unavailable pool must not prevent other selected pools filling.
        if let Err(error) = refresh_at(s, &p, n, true).await {
            eprintln!(
                "Monad hourly backfill pool {} at block {n}: {error}",
                text(&p, "pool_address")
            );
        }
    }
    Ok(())
}
/// One-shot repair for all 24 completed hours without starting another listener.
pub async fn backfill_recent_hours(s: &AppState) -> Result<()> {
    for hour in 1..=24 {
        backfill_one(s).await?;
        println!("Monad hourly backfill: completed pass {hour}/24");
    }
    Ok(())
}

async fn pool_worker(s: AppState, pool: Value, token: String) {
    let states = async {
        let mut timer = tokio::time::interval(std::time::Duration::from_millis(store::poll_ms()));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            let result: Result<()> = async {
                let head = (hex_i(&rpc(&s, "eth_blockNumber", json!([])).await?)? - 2).max(0);
                refresh(&s, &pool, head).await
            }
            .await;
            if let Err(e) = result {
                eprintln!("Monad state {}: {e}", text(&pool, "pool_address"))
            }
        }
    };
    let logs = async {
        let mut span = 128;
        let mut timer = tokio::time::interval(std::time::Duration::from_millis(store::poll_ms()));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            let result: Result<()> = async {
                let head = (hex_i(&rpc(&s, "eth_blockNumber", json!([])).await?)? - 2).max(0);
                poll(&s, &pool, &token, head, &mut span).await
            }
            .await;
            if let Err(e) = result {
                eprintln!("Monad logs {}: {e}", text(&pool, "pool_address"));
                if let Ok(c) = store::open() {
                    let _ = c.execute(
                        "UPDATE cursors SET error=?2 WHERE pool=?1",
                        params![text(&pool, "pool_address"), e.to_string()],
                    );
                }
            }
        }
    };
    // Cancellation of this task drops both futures, so a removed pool cannot
    // leave an independently spawned state refresher running.
    tokio::join!(states, logs);
}

fn fast_decode(
    pool: &Value,
    meta: &PoolMetadata,
    raw: &str,
    block: i64,
    as_of: i64,
) -> Result<Value> {
    let w = words(raw)?;
    let kind = text(pool, "pool_type");
    let mut fee = meta.fee;
    let ratio = if matches!(kind, "constant_product_v2" | "solidly_v2") {
        if w.len() < 2 {
            bail!("reserves missing")
        }
        let a = number(w[0], false)?;
        let b = number(w[1], false)?;
        if a <= 0. || b <= 0. {
            bail!("empty reserves")
        }
        b / a * 10f64.powi(meta.d0 - meta.d1)
    } else {
        let sqrt = number(w.first().context("slot0 missing")?, false)?;
        if kind == "uniswap_v4" {
            let raw_fee = number(w.get(3).context("V4 fee missing")?, false)?;
            fee = if raw_fee < 8388608. {
                Some(raw_fee / 100.)
            } else {
                None
            };
        }
        (sqrt / 2f64.powi(96)).powi(2) * 10f64.powi(meta.d0 - meta.d1)
    };
    if !ratio.is_finite() || ratio <= 0. {
        bail!("invalid fast price")
    }
    let (p0, p1) = if usd_anchor(&meta.a) {
        (Some(1.), Some(1. / ratio))
    } else if usd_anchor(&meta.b) {
        (Some(ratio), Some(1.))
    } else {
        (None, None)
    };
    Ok(
        json!({"pool_address":pool["pool_address"],"refreshed_at_ms":as_of*1000,"snapshot":{"decoded_price":{"token0_address":meta.a,"token1_address":meta.b,"decimals0":meta.d0,"decimals1":meta.d1,"token1_per_token0":ratio,"token0_price_usd":p0,"token1_price_usd":p1,"as_of":as_of,"state_block":block,"fee_bps":fee,"usd_anchor_status":if p0.is_some(){"trusted_stable_assumption"}else{"unavailable"}}}}),
    )
}

async fn fast_prices(s: &AppState) -> Result<()> {
    let mut pools = store::fixed_pools()?;
    for pool in store::display(&store::open()?)?["pools"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if !pools
            .iter()
            .any(|p| p["pool_address"] == pool["pool_address"])
        {
            pools.push(pool.clone())
        }
    }
    // The slower state worker validates immutable token identities/decimals.
    // Never repeat token0/token1/decimals RPC calls on the one-second path.
    let ready = {
        let cache = METADATA
            .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
            .lock()
            .unwrap();
        pools
            .into_iter()
            .filter_map(|p| cache.get(text(&p, "pool_address")).cloned().map(|m| (p, m)))
            .collect::<Vec<_>>()
    };
    if ready.is_empty() {
        return Ok(());
    }
    let n = (hex_i(&rpc(s, "eth_blockNumber", json!([])).await?)? - 2).max(0);
    let head = block(s, n).await?;
    let as_of = hex_i(&head["timestamp"])?;
    let mut decoded = Vec::new();
    let batches=ready.chunks(10).map(|chunk| async move {
        let requests = chunk
            .iter()
            .map(|(pool, _)| {
                let kind = text(pool, "pool_type");
                let id = text(pool, "pool_address");
                let (to, data) = if kind == "uniswap_v4" {
                    (
                        "0x77395f3b2e73ae90843717371294fa97cc419d64",
                        format!(
                            "{}{}",
                            selector("getSlot0(bytes32)"),
                            id.trim_start_matches("0x")
                        ),
                    )
                } else {
                    (
                        id,
                        selector(if matches!(kind, "constant_product_v2" | "solidly_v2") {
                            "getReserves()"
                        } else {
                            "slot0()"
                        }),
                    )
                };
                (
                    "eth_call".to_owned(),
                    json!([{"to":to,"data":data},format!("0x{n:x}")]),
                )
            })
            .collect::<Vec<_>>();
        // Each response ID is matched by batch(). A bad pool cannot discard
        // other prices: on failure retry items independently.
        let results = match batch(s, &requests).await {
            Ok(values) => values.into_iter().map(Ok).collect::<Vec<Result<Value>>>(),
            Err(_) => {
                futures_util::future::join_all(
                    requests
                        .iter()
                        .map(|(method, params)| rpc(s, method, params.clone())),
                )
                .await
            }
        };
        (chunk,results)
    });
    for (chunk,results) in futures_util::future::join_all(batches).await {
        for ((pool, meta), raw) in chunk.iter().zip(results) {
            match raw.and_then(|v| {
                fast_decode(
                    pool,
                    meta,
                    v.as_str().context("price result missing")?,
                    n,
                    as_of,
                )
            }) {
                Ok(value) => decoded.push(value),
                Err(e) => eprintln!("Monad fast price {}: {e}", text(pool, "pool_address")),
            }
        }
    }
    if block(s, n).await?["hash"] != head["hash"] {
        bail!("fast price block replaced before commit")
    }
    let mut c = store::open()?;
    let tx = c.transaction()?;
    for body in decoded {
        tx.execute("INSERT INTO snapshots VALUES(?1,?2) ON CONFLICT(pool) DO UPDATE SET body=excluded.body WHERE coalesce(json_extract(excluded.body,'$.snapshot.decoded_price.state_block'),0)>=coalesce(json_extract(snapshots.body,'$.snapshot.decoded_price.state_block'),0)",params![body["pool_address"].as_str(),body.to_string()])?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod fast_tests {
    use super::*;
    #[test]
    fn decode_fast_preserves_pool_units_and_fees() {
        let meta = PoolMetadata {
            a: ZERO.into(),
            b: "0x754704bc059f8c67012fed69bc8a327a5aafb603".into(),
            d0: 6,
            d1: 6,
            fee: Some(30.),
        };
        let v2 = json!({"pool_address":"v2","pool_type":"constant_product_v2"});
        let raw = format!("0x{:064x}{:064x}", 1_000_000u128, 2_000_000u128);
        let value = fast_decode(&v2, &meta, &raw, 100, 1000).unwrap();
        assert_eq!(value["snapshot"]["decoded_price"]["token1_per_token0"], 2.);
        let v4 = json!({"pool_address":"v4","pool_type":"uniswap_v4"});
        let raw = format!("0x{:064x}{:064x}{:064x}{:064x}", 1u128 << 96, 0, 0, 3000);
        let value = fast_decode(&v4, &meta, &raw, 101, 1001).unwrap();
        assert_eq!(value["pool_address"], "v4");
        assert_eq!(value["snapshot"]["decoded_price"]["token1_per_token0"], 1.);
        assert_eq!(value["snapshot"]["decoded_price"]["fee_bps"], 30.);
        assert!(fast_decode(&v4, &meta, "0x", 102, 1002).is_err());
    }
}

pub fn spawn(s: AppState) -> Result<()> {
    store::open()?.execute_batch("CREATE TABLE IF NOT EXISTS checkpoints(pool TEXT NOT NULL,block INTEGER NOT NULL,hash TEXT NOT NULL,PRIMARY KEY(pool,block));")?;
    let prices = s.clone();
    tokio::spawn(async move {
        let mut timer =
            tokio::time::interval(std::time::Duration::from_millis(store::price_poll_ms()));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            if let Err(e) = fast_prices(&prices).await {
                eprintln!("Monad fast prices: {e}")
            }
        }
    });
    if std::env::var("MONAD_LISTENER_BACKFILL").as_deref() != Ok("0") {
        let archive = s.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = backfill_one(&archive).await {
                    eprintln!("Monad hourly backfill: {e}")
                }
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });
    }
    tokio::spawn(async move {
        let mut tasks: std::collections::HashMap<String, (String, tokio::task::JoinHandle<()>)> =
            std::collections::HashMap::new();
        loop {
            let desired: Result<std::collections::HashMap<String, (Value, String)>> = (|| {
                let mut desired = std::collections::HashMap::new();
                for pool in store::fixed_pools()? {
                    desired.insert(
                        text(&pool, "pool_address").to_lowercase(),
                        (pool.clone(), text(&pool, "token_a_address").to_owned()),
                    );
                }
                let config = store::display(&store::open()?)?;
                for pool in config["pools"].as_array().into_iter().flatten() {
                    desired.insert(
                        text(pool, "pool_address").to_lowercase(),
                        (pool.clone(), text(&config, "token").to_owned()),
                    );
                }
                Ok(desired)
            })(
            );
            match desired {
                Ok(desired) => {
                    tasks.retain(|id, (token, handle)| {
                        let keep = desired.get(id).is_some_and(|(_, wanted)| wanted == token)
                            && !handle.is_finished();
                        if !keep {
                            handle.abort();
                        }
                        keep
                    });
                    for (id, (pool, token)) in desired {
                        tasks.entry(id).or_insert_with(|| {
                            (
                                token.clone(),
                                tokio::spawn(pool_worker(s.clone(), pool, token)),
                            )
                        });
                    }
                }
                Err(e) => eprintln!("Monad monitor configuration: {e}"),
            }
            tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        }
    });
    Ok(())
}
#[derive(Deserialize)]
pub struct ReadQuery {
    token: Option<String>,
    pool: Option<String>,
    limit: Option<usize>,
    resolution: Option<String>,
    target_mint: Option<String>,
}
fn event_read_db() -> Result<rusqlite::Connection> {
    store::read_connection()?.context("Monad database is not initialized")
}
fn read(q: &ReadQuery, whales: bool) -> Result<Vec<Value>> {
    let c = event_read_db()?;
    let config = store::display(&c)?;
    let ids = config["pools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| text(p, "pool_address").to_lowercase())
        .collect::<Vec<_>>();
    let mut stmt=c.prepare("SELECT body FROM events WHERE (?1 IS NULL OR lower(pool)=lower(?1)) AND (?2=0 OR whale=1) ORDER BY block DESC,log_index DESC LIMIT 10000")?;
    let rows = stmt
        .query_map(params![q.pool, whales], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut values = rows
        .into_iter()
        .map(|s| serde_json::from_str::<Value>(&s))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    values.retain(|v| {
        ids.contains(&text(v, "watched_pool").to_lowercase())
            && q.token
                .as_deref()
                .unwrap_or(text(&config, "token"))
                .eq_ignore_ascii_case(text(v, "token"))
    });
    values.truncate(q.limit.unwrap_or(100).clamp(1, 5000));
    values.reverse();
    Ok(values)
}
type Reply = std::result::Result<Json<Value>, (StatusCode, Json<Value>)>;
fn error(e: anyhow::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error":e.to_string()})),
    )
}
pub async fn events(Query(q): Query<ReadQuery>) -> Reply {
    Ok(Json(json!({"events":read(&q,false).map_err(error)?})))
}
pub async fn alerts(Query(q): Query<ReadQuery>) -> Reply {
    Ok(Json(json!({"alerts":read(&q,true).map_err(error)?})))
}
pub async fn snapshots() -> Reply {
    let c = event_read_db().map_err(error)?;
    let config = store::display(&c).map_err(error)?;
    let mut values = Vec::new();
    for p in config["pools"].as_array().into_iter().flatten() {
        let row: Option<String> = c
            .query_row(
                "SELECT body FROM snapshots WHERE pool=?1",
                [text(p, "pool_address")],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| error(e.into()))?;
        if let Some(s) = row {
            values.push(serde_json::from_str::<Value>(&s).map_err(|e| error(e.into()))?)
        }
    }
    Ok(Json(json!({"snapshots":values})))
}
pub async fn status() -> Reply {
    let c = event_read_db().map_err(error)?;
    let config = store::display(&c).map_err(error)?;
    let mut cursors = Vec::new();
    for p in config["pools"].as_array().into_iter().flatten() {
        let row:Option<Value>=c.query_row("SELECT block,error,started_at FROM cursors WHERE pool=?1",[text(p,"pool_address")],|r|Ok(json!({"pool":p["pool_address"],"block":r.get::<_,i64>(0)?,"error":r.get::<_,Option<String>>(1)?,"started_at":r.get::<_,i64>(2)?}))).optional().map_err(|e|error(e.into()))?;
        cursors.extend(row);
    }
    Ok(Json(
        json!({"listener":config["token"].is_string(),"config":config,"cursors":cursors,"poll_ms":store::poll_ms(),"state_refresh_ms":store::poll_ms(),"price_refresh_ms":store::price_poll_ms(),"fixed_pool_count":store::fixed_pools().map(|p|p.len()).unwrap_or(0),"market_refresh_ms":30000}),
    ))
}
pub async fn protocols() -> Json<Value> {
    Json(
        json!({"protocols":[{"decoder":"evm-abi","pool_types":["constant_product_v2","solidly_v2","concentrated_v3","uniswap_v4"]}]}),
    )
}
pub async fn history(Query(mut q): Query<ReadQuery>) -> Reply {
    let pool = q.pool.clone().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"pool is required"})),
        )
    })?;
    if q.token.is_none() {
        q.token = q.target_mint.clone();
    }
    let resolution = q.resolution.as_deref().unwrap_or("1m");
    let interval = match resolution {
        "1m" => 60,
        "5m" => 300,
        "15m" => 900,
        "1h" => 3600,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid resolution"})),
            ))
        }
    };
    q.limit = Some(5000);
    let events = read(&q, false).map_err(error)?;
    let mut candles = std::collections::BTreeMap::<i64, Value>::new();
    let mut trades = Vec::new();
    let mut unknown = 0;
    let mut volume = 0.;
    for e in &events {
        let t = &e["enrichment"]["transaction"]["trade"];
        let ts = e["observed_at_ms"].as_i64().unwrap_or(0) / 1000;
        if ts < store::now() - 86400 {
            continue;
        }
        if !matches!(text(t, "side"), "buy" | "sell") {
            continue;
        }
        let mut trade = t.clone();
        trade["signature"] = e["signature"].clone();
        trade["log_index"] = e["log_index"].clone();
        trade["observed_at_ms"] = e["observed_at_ms"].clone();
        trades.push(trade);
        let v = t["notional_usd"].as_f64();
        volume += v.unwrap_or(0.);
        if v.is_none() {
            unknown += 1;
        }
        // Ratio candles need no USD oracle and always represent the selected base.
        let Some(price) = t["swap_ratio_quote_per_target"]
            .as_f64()
            .filter(|p| *p > 0.)
        else {
            continue;
        };
        let bucket = ts / interval * interval;
        let c=candles.entry(bucket).or_insert(json!({"timestamp":bucket*1000,"open":price,"high":price,"low":price,"close":price,"volume_usd":0.,"source":"monad_local_trades"}));
        c["high"] = json!(c["high"].as_f64().unwrap().max(price));
        c["low"] = json!(c["low"].as_f64().unwrap().min(price));
        c["close"] = json!(price);
        c["volume_usd"] = json!(c["volume_usd"].as_f64().unwrap() + v.unwrap_or(0.));
    }
    let c = event_read_db().map_err(error)?;
    let started: Option<i64> = c
        .query_row(
            "SELECT started_at FROM cursors WHERE pool=?1",
            [&pool],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| error(e.into()))?;
    let data = candles.into_values().collect::<Vec<_>>();
    let cat = catalog_path();
    let price_path = std::env::var_os("LP_PRICE_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| cat.parent().unwrap().join("lp_price_history.db"));
    let hourly=(||->Result<Vec<Value>>{if !price_path.exists(){return Ok(vec![])}let c=rusqlite::Connection::open_with_flags(price_path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;if c.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name='pool_price_history'",[],|r|r.get::<_,i32>(0)).optional()?.is_none(){return Ok(vec![])}let mut st=c.prepare("SELECT timestamp,token1_per_token0,token0_address,token1_address FROM pool_price_history WHERE chain='monad' AND lower(pool_address)=lower(?1) AND timestamp>=?2 ORDER BY timestamp")?;let vals=st.query_map(params![pool,store::now()-86400],|r|Ok(json!({"timestamp":r.get::<_,i64>(0)?,"token1_per_token0":r.get::<_,Option<f64>>(1)?,"token0_address":r.get::<_,String>(2)?,"token1_address":r.get::<_,String>(3)?,"source":"hourly_pool_history"})))?.collect::<rusqlite::Result<Vec<_>>>()?;Ok(vals)})().map_err(error)?;
    Ok(Json(
        json!({"data":data,"candles":data,"trades":trades,"hourly_prices":hourly,"hourly_source":"hourly_pool_history","summary":{"volume_usd":if !trades.is_empty() && unknown==trades.len(){None}else{Some(volume)},"volume_label":"observed_volume","coverage_started_at":started,"full_24h":started.is_some_and(|t|t<=store::now()-86400),"unknown_usd_trades":unknown,"trades":trades.len()},"source":"monad_local_trades","price_unit":"quote_per_target"}),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn word(n: i128) -> String {
        if n < 0 {
            format!("{:032x}{:032x}", u128::MAX, n as u128)
        } else {
            format!("{n:064x}")
        }
    }
    fn fixture(signature: &str, nums: &[i128]) -> Value {
        let mut topics = vec![json!(topic(signature)), json!(format!("0x{}", word(1)))];
        if signature == V3_MINT || signature == V3_BURN {
            topics.push(json!(format!("0x{}", word(-10))));
            topics.push(json!(format!("0x{}", word(10))));
        } else if signature != V2_MINT {
            topics.push(json!(format!("0x{}", word(2))));
        }
        json!({"topics":topics,"data":format!("0x{}",nums.iter().map(|n|word(*n)).collect::<String>())})
    }
    #[test]
    fn abi_directions_and_liquidity() {
        let v2 = decode(
            &fixture(V2_SWAP, &[0, 200, 100, 0]),
            "constant_product_v2",
            0,
            0,
        )
        .unwrap();
        assert_eq!(v2.amount0, Some(100.));
        assert_eq!(v2.amount1, Some(-200.));
        let v3 = decode(
            &fixture(V3_SWAP, &[-100, 200, 0, 0, 0]),
            "concentrated_v3",
            0,
            0,
        )
        .unwrap();
        assert_eq!(v3.amount0, Some(100.));
        let v4 = decode(
            &fixture(V4_SWAP, &[100, -200, 0, 0, 0, 3000]),
            "uniswap_v4",
            0,
            0,
        )
        .unwrap();
        assert_eq!(v4.amount1, Some(-200.));
        for (sig, kind, nums, expected) in [
            (V2_MINT, "constant_product_v2", vec![1, 2], "add_liquidity"),
            (
                V2_BURN,
                "constant_product_v2",
                vec![1, 2],
                "remove_liquidity",
            ),
            (
                V3_MINT,
                "concentrated_v3",
                vec![1, 2, 3, 4],
                "add_liquidity",
            ),
            (
                V3_BURN,
                "concentrated_v3",
                vec![2, 3, 4],
                "remove_liquidity",
            ),
            (
                V4_LIQ,
                "uniswap_v4",
                vec![-10, 10, -100, 0],
                "remove_liquidity",
            ),
        ] {
            assert_eq!(
                decode(&fixture(sig, &nums), kind, 0, 0).unwrap().kind,
                expected
            );
        }
    }
    #[test]
    fn sell_directions_and_pancake_abi() {
        for (sig, kind, nums) in [
            (V2_SWAP, "constant_product_v2", vec![100, 0, 0, 200]),
            (V3_SWAP, "concentrated_v3", vec![100, -200, 1, 1, 0]),
            (
                PANCAKE_V3_SWAP,
                "concentrated_v3",
                vec![100, -200, 1, 1, 0, 0, 0],
            ),
            (V4_SWAP, "uniswap_v4", vec![-100, 200, 1, 1, 0, 3000]),
        ] {
            let d = decode(&fixture(sig, &nums), kind, 2, 2).unwrap();
            assert_eq!(d.amount0, Some(-1.));
            assert_eq!(d.amount1, Some(2.));
        }
        assert!(signatures("concentrated_v3").contains(&topic(PANCAKE_V3_SWAP)));
    }
    #[test]
    fn v4_filter_is_scoped() {
        let f = log_filter(
            &json!({"pool_type":"uniswap_v4","pool_address":format!("0x{}",word(9))}),
            10,
            20,
        );
        assert_eq!(f["topics"][1], format!("0x{}", word(9)));
        assert_eq!(f["address"], manager());
    }
    #[test]
    fn whale_and_unknown() {
        let log = fixture(V4_SWAP, &[100, -200, 1, 1, 0, 3000]);
        let p = json!({"pool_address":"p"});
        let q = json!({"token0_address":"a","token1_address":"b","as_of":100,"token0_price_usd":100.,"token1_price_usd":50.,"usd_anchor_status":"trusted_stable_assumption"});
        let e = enrich(
            &p,
            "a",
            &log,
            &q,
            decode(&log, "uniswap_v4", 0, 0).unwrap(),
            100,
            &json!("user"),
            &json!("router"),
        );
        assert_eq!(e["whale"], true);
        assert_eq!(e["enrichment"]["transaction"]["trade"]["side"], "buy");
        let e = enrich(
            &p,
            "a",
            &log,
            &json!({}),
            decode(&log, "uniswap_v4", 0, 0).unwrap(),
            100,
            &json!("user"),
            &json!("router"),
        );
        assert!(e["whale"].is_null());
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::state::{EndpointRuntime, RouteRuntime, Tier};
    use axum::{extract::State, response::IntoResponse};
    use std::sync::{
        atomic::{AtomicU64, AtomicUsize},
        Arc, Mutex,
    };
    #[derive(Default)]
    struct Mock {
        failed: bool,
        replaced: bool,
        ranges: Vec<(i64, i64)>,
        range_limit: Option<i64>,
        http_400: bool,
        min_block: Option<i64>,
        null_block_once: Option<i64>,
        headers: Vec<i64>,
    }
    fn hash(n: i64, replaced: bool) -> String {
        format!(
            "0x{:064x}",
            n * 10 + if replaced && n >= 101 { 1 } else { 0 }
        )
    }
    fn word(n: i128) -> String {
        if n < 0 {
            format!("{:032x}{:032x}", u128::MAX, n as u128)
        } else {
            format!("{n:064x}")
        }
    }
    async fn upstream(
        State(m): State<Arc<Mutex<Mock>>>,
        Json(req): Json<Value>,
    ) -> axum::response::Response {
        if let Some(items) = req.as_array() {
            let mut rows = Vec::new();
            for item in items {
                rows.push(upstream_one(&m, item.clone()));
            }
            return Json(json!(rows)).into_response();
        }
        let result = upstream_one(&m, req);
        let status = if m.lock().unwrap().http_400
            && result["error"]["message"] == "block range too large"
        {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::OK
        };
        (status, Json(result)).into_response()
    }
    fn upstream_one(m: &Arc<Mutex<Mock>>, req: Value) -> Value {
        let mut m = m.lock().unwrap();
        let method = text(&req, "method");
        let result = match method {
            "eth_getBlockByNumber" => {
                let n = hex_i(&req["params"][0]).unwrap();
                m.headers.push(n);
                if m.min_block.is_some_and(|min| n < min) {
                    return json!({"jsonrpc":"2.0","id":req["id"],"result":null});
                }
                if m.null_block_once == Some(n) {
                    m.null_block_once = None;
                    return json!({"jsonrpc":"2.0","id":req["id"],"result":null});
                }
                json!({"hash":hash(n,m.replaced),"timestamp":format!("0x{:x}",1000+n),"number":format!("0x{n:x}")})
            }
            "eth_getLogs" => {
                let f = &req["params"][0];
                let from = hex_i(&f["fromBlock"]).unwrap();
                let to = hex_i(&f["toBlock"]).unwrap();
                m.ranges.push((from, to));
                if m.failed {
                    return json!({"jsonrpc":"2.0","id":req["id"],"error":{"code":-32000,"message":"fixture outage"}});
                }
                if m.range_limit.is_some_and(|limit| to - from + 1 > limit) {
                    return json!({"jsonrpc":"2.0","id":req["id"],"error":{"code":-32602,"message":"block range too large"}});
                }
                json!((from..=to).map(|n|json!({"address":f["address"],"blockNumber":format!("0x{n:x}"),"blockHash":hash(n,m.replaced),"transactionHash":format!("0x{:064x}",n*10+if m.replaced&&n>=101{1}else{0}),"logIndex":"0x0","topics":[topic(V2_SWAP),format!("0x{}",word(1)),format!("0x{}",word(2))],"data":format!("0x{}{}{}{}",word(0),word(200),word(100),word(0))})).collect::<Vec<_>>())
            }
            "eth_getTransactionByHash" => {
                json!({"from":"0x3333333333333333333333333333333333333333","to":"0x4444444444444444444444444444444444444444"})
            }
            "eth_call" => {
                let data = text(&req["params"][0], "data");
                if data == selector("token0()") {
                    json!(format!("0x{}", word(0)))
                } else if data == selector("token1()") {
                    json!(format!(
                        "0x{}",
                        "1".repeat(40).chars().fold("0".repeat(24), |mut s, c| {
                            s.push(c);
                            s
                        })
                    ))
                } else if data == selector("decimals()") {
                    json!(format!("0x{}", word(6)))
                } else if data == selector("getReserves()") {
                    json!(format!("0x{}{}{}", word(100), word(250), word(0)))
                } else {
                    panic!("unexpected call selector")
                }
            }
            _ => panic!("unexpected mock method {method}"),
        };
        json!({"jsonrpc":"2.0","id":req["id"],"result":result})
    }
    #[tokio::test]
    async fn dedup_resume_reorg_and_token_isolation() {
        let dir = std::env::temp_dir().join(format!(
            "monad-events-{}-{}",
            std::process::id(),
            store::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("MONAD_LISTENER_DB", dir.join("events.db"));
        let a = "0x0000000000000000000000000000000000000000";
        let b = "0x1111111111111111111111111111111111111111";
        let id = "0x2222222222222222222222222222222222222222";
        let pool =
            json!({"pool_address":id,"pool_type":"constant_product_v2","protocol":"fixture"});
        let mut c = store::open().unwrap();
        c.execute_batch(
            "CREATE TABLE checkpoints(pool TEXT,block INTEGER,hash TEXT,PRIMARY KEY(pool,block));",
        )
        .unwrap();
        store::save(&mut c, a, &[pool.clone()]).unwrap();
        let snap = json!({"snapshot":{"decoded_price":{"token0_address":a,"token1_address":b,"decimals0":0,"decimals1":0,"as_of":1100,"token0_price_usd":100.,"token1_price_usd":50.,"usd_anchor_status":"trusted_stable_assumption"}}});
        c.execute(
            "INSERT INTO snapshots VALUES(?1,?2)",
            params![id, snap.to_string()],
        )
        .unwrap();
        let mock = Arc::new(Mutex::new(Mock {
            failed: true,
            ..Default::default()
        }));
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", tcp.local_addr().unwrap());
        let router = axum::Router::new()
            .route("/", axum::routing::post(upstream))
            .with_state(mock.clone());
        let server = tokio::spawn(async move { axum::serve(tcp, router).await.unwrap() });
        let endpoint = Arc::new(EndpointRuntime {
            title: "fixture".into(),
            provider: "fixture".into(),
            tier: Tier::Free,
            priority: 0,
            max_concurrency: 4,
            url,
            supports_historical: Some(true),
            benchmark: None,
            in_flight: AtomicUsize::new(0),
            requests: AtomicU64::new(0),
            successes: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
        });
        let route = Arc::new(RouteRuntime {
            name: "monad".into(),
            protocol: "evm".into(),
            chain_id: "143".into(),
            aliases: vec![],
            endpoints: vec![endpoint],
        });
        let state = AppState {
            routes: Arc::new(std::collections::HashMap::from([("monad".into(), route)])),
            http: reqwest::Client::new(),
        };
        let mut span = 128;
        assert!(poll(&state, &pool, a, 100, &mut span).await.is_err());
        assert_eq!(
            c.query_row("SELECT block FROM cursors", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            99
        );
        mock.lock().unwrap().failed = false;
        poll(&state, &pool, a, 100, &mut span).await.unwrap();
        assert_eq!(mock.lock().unwrap().ranges, vec![(100, 100), (100, 100)]);
        // Replay a committed range (e.g. after operator recovery); unique log keys deduplicate it.
        c.execute("UPDATE cursors SET block=99,hash=?1", [hash(99, false)])
            .unwrap();
        poll(&state, &pool, a, 100, &mut span).await.unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        poll(&state, &pool, a, 101, &mut span).await.unwrap();
        mock.lock().unwrap().replaced = true;
        poll(&state, &pool, a, 102, &mut span).await.unwrap();
        assert_eq!(mock.lock().unwrap().ranges.last(), Some(&(101, 102)));
        assert_eq!(
            c.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM events WHERE block=101 AND block_hash=?1",
                [hash(101, false)],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        // Restart opens the same cursor and only requests the missing block.
        drop(c);
        poll(&state, &pool, a, 103, &mut span).await.unwrap();
        assert_eq!(mock.lock().unwrap().ranges.last(), Some(&(103, 103)));
        let mut c = store::open().unwrap();
        store::save(&mut c, b, &[pool.clone()]).unwrap();
        let query = ReadQuery {
            token: None,
            pool: None,
            limit: None,
            resolution: None,
            target_mint: None,
        };
        assert!(read(&query, false).unwrap().is_empty());
        poll(&state, &pool, b, 104, &mut span).await.unwrap();
        let values = read(&query, false).unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["token"], b);
        assert_eq!(
            values[0]["enrichment"]["transaction"]["trade"]["side"],
            "sell"
        );
        assert_eq!(block_at_or_after(&state, 104, 1101).await.unwrap(), 101);
        std::env::set_var("LP_PRICE_DB", dir.join("price.db"));
        refresh(&state, &pool, 104).await.unwrap();
        let before: String = c
            .query_row("SELECT body FROM snapshots WHERE pool=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        refresh_at(&state, &pool, 100, true).await.unwrap();
        let after: String = c
            .query_row("SELECT body FROM snapshots WHERE pool=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(before, after); // Historical fills never replace current display state.
        let prices = rusqlite::Connection::open(dir.join("price.db")).unwrap();
        assert_eq!(
            prices
                .query_row("SELECT observed_at FROM pool_price_history", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1104
        ); // A newer observation in the same hour wins.
        drop(prices);
        // JSON-RPC range errors shrink immediately without moving the start.
        mock.lock().unwrap().range_limit = Some(3);
        mock.lock().unwrap().ranges.clear();
        span = 128;
        poll(&state, &pool, b, 116, &mut span).await.unwrap();
        assert_eq!(
            mock.lock().unwrap().ranges,
            vec![(105, 116), (105, 110), (105, 107)]
        );
        assert_eq!(span, 3);
        assert_eq!(
            c.query_row("SELECT block FROM cursors", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            107
        );
        // Later polls retain the learned limit and fill every intervening block.
        poll(&state, &pool, b, 116, &mut span).await.unwrap();
        assert_eq!(mock.lock().unwrap().ranges.last(), Some(&(108, 110)));
        mock.lock().unwrap().http_400 = true;
        mock.lock().unwrap().range_limit = Some(1);
        poll(&state, &pool, b, 116, &mut span).await.unwrap();
        assert_eq!(span, 1);
        assert_eq!(
            mock.lock()
                .unwrap()
                .ranges
                .iter()
                .rev()
                .take(2)
                .cloned()
                .collect::<Vec<_>>(),
            vec![(111, 111), (111, 113)]
        );
        for _ in 0..5 {
            poll(&state, &pool, b, 116, &mut span).await.unwrap();
        }
        assert_eq!(
            c.query_row(
                "SELECT count(DISTINCT block) FROM events WHERE block BETWEEN 105 AND 116",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            12
        );
        // Even a rejected single block must not advance its persisted cursor.
        mock.lock().unwrap().range_limit = Some(0);
        let error = poll(&state, &pool, b, 117, &mut span)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("400 Bad Request") && error.contains("block range too large"));
        assert_eq!(
            c.query_row("SELECT block FROM cursors", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            116
        );
        // Recent-hour location must work even when old chain history is pruned.
        {
            let mut mock = mock.lock().unwrap();
            mock.min_block = Some(180000);
            mock.null_block_once = Some(199000);
            mock.headers.clear();
        }
        assert_eq!(
            block_at_or_after(&state, 200000, 200900).await.unwrap(),
            199900
        );
        assert!(mock.lock().unwrap().headers.iter().all(|n| *n >= 180000));
        assert_eq!(
            mock.lock()
                .unwrap()
                .headers
                .iter()
                .filter(|n| **n == 199000)
                .count(),
            2
        );
        assert!(block_at_or_after(&state, 200000, 100)
            .await
            .unwrap_err()
            .to_string()
            .contains("24-hour window"));
        server.abort();
        drop(c);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
