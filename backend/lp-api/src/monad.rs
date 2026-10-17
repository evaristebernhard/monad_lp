use super::*;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
#[path = "monad_market.rs"]
mod market;
#[path = "../../../rpc/src/monad_store.rs"]
mod store;
pub use market::candles;
type ApiResult = Result<Json<Value>, (StatusCode, Json<ErrorBody>)>;
fn bad(code: StatusCode, message: &str) -> (StatusCode, Json<ErrorBody>) {
    (
        code,
        Json(ErrorBody {
            error: message.into(),
        }),
    )
}
fn address(s: &str) -> bool {
    s.len() == 42 && s.starts_with("0x") && s[2..].bytes().all(|c| c.is_ascii_hexdigit())
}

// Read-only transport for the browser's on-chain quotes and simulations.
// Signing and transaction submission belong exclusively to the user's wallet.
pub async fn trade_rpc(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult {
    let method = body["method"].as_str().unwrap_or("");
    // JSON-RPC permits omitting params for methods with no arguments. Viem
    // does this for eth_gasPrice and eth_blockNumber in the purchase flow.
    let params = match body.get("params") {
        Some(params) if params.is_array() => params.clone(),
        None if matches!(method, "eth_chainId" | "eth_blockNumber" | "eth_gasPrice") => json!([]),
        _ => return Err(bad(StatusCode::BAD_REQUEST, "invalid Monad RPC parameters")),
    };
    if !matches!(
        method,
        "eth_chainId"
            | "eth_call"
            | "eth_getBalance"
            | "eth_blockNumber"
            | "eth_getBlockByNumber"
            | "eth_getTransactionReceipt"
            | "eth_getTransactionByHash"
            | "eth_getCode"
            | "eth_estimateGas"
            | "eth_gasPrice"
    ) || body.to_string().len() > 65536
    {
        return Err(bad(
            StatusCode::BAD_REQUEST,
            "only supported read-only Monad RPC methods are allowed",
        ));
    }
    let response = s
        .rpc_http
        .post(format!(
            "{}/rpc/monad",
            s.rpc_price_url.trim_end_matches('/')
        ))
        .json(&json!({"jsonrpc":"2.0","id":body["id"],"method":method,"params":params}))
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|_| bad(StatusCode::BAD_GATEWAY, "Monad RPC unavailable"))?;
    if !response.status().is_success() {
        return Err(bad(StatusCode::BAD_GATEWAY, "Monad RPC request failed"));
    }
    let value: Value = response
        .json()
        .await
        .map_err(|_| bad(StatusCode::BAD_GATEWAY, "invalid Monad RPC response"))?;
    Ok(Json(value))
}
pub async fn selection_testnet_rpc(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult {
    let method = body["method"].as_str().unwrap_or("");
    // JSON-RPC permits omitting params for methods with no arguments. Viem
    // does this for eth_gasPrice and eth_blockNumber in the purchase flow.
    let params = match body.get("params") {
        Some(params) if params.is_array() => params.clone(),
        None if matches!(method, "eth_chainId" | "eth_blockNumber" | "eth_gasPrice") => json!([]),
        _ => return Err(bad(StatusCode::BAD_REQUEST, "invalid Monad RPC parameters")),
    };
    if !matches!(
        method,
        "eth_chainId"
            | "eth_call"
            | "eth_getBalance"
            | "eth_blockNumber"
            | "eth_getBlockByNumber"
            | "eth_getTransactionReceipt"
            | "eth_getTransactionByHash"
            | "eth_getCode"
            | "eth_estimateGas"
            | "eth_gasPrice"
    ) || body.to_string().len() > 65536
    {
        return Err(bad(
            StatusCode::BAD_REQUEST,
            "only supported read-only Monad RPC methods are allowed",
        ));
    }
    let response = s
        .rpc_http
        .post("https://testnet-rpc.monad.xyz")
        .json(&json!({"jsonrpc":"2.0","id":body["id"],"method":method,"params":params}))
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|_| bad(StatusCode::BAD_GATEWAY, "Monad RPC unavailable"))?;
    if !response.status().is_success() {
        return Err(bad(StatusCode::BAD_GATEWAY, "Monad RPC request failed"));
    }
    let value: Value = response
        .json()
        .await
        .map_err(|_| bad(StatusCode::BAD_GATEWAY, "invalid Monad RPC response"))?;
    Ok(Json(value))
}
// Read-only targeted queries: metric requests never create schema, invoke RPC,
// attach unrelated histories, or run the full pool metric pipeline.
fn read_db(path: &PathBuf, sql: &str) -> Result<Vec<Value>> {
    let c = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.busy_timeout(Duration::from_secs(5))?;
    if sql.contains("pool_price_history") {
        let price = env::var_os("LP_PRICE_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                path.parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join("lp_price_history.db")
            });
        if price != *path {
            if !price.exists() {
                return Ok(vec![]);
            }
            c.execute(
                "ATTACH DATABASE ?1 AS price",
                [price.to_string_lossy().as_ref()],
            )?;
        }
    }
    let mut st = c.prepare(sql)?;
    let names = st
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let rows = st
        .query_map([], |r| {
            let mut object = serde_json::Map::new();
            for (i, name) in names.iter().enumerate() {
                use rusqlite::types::ValueRef;
                let v = match r.get_ref(i)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => json!(n),
                    ValueRef::Real(n) => json!(n),
                    ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    ValueRef::Blob(_) => Value::Null,
                };
                object.insert(name.clone(), v);
            }
            Ok(Value::Object(object))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
fn canonical_labels(mut p: Value) -> Value {
    for side in ["a", "b"] {
        let address = p[format!("token_{side}_address")].as_str().unwrap_or("");
        let symbol = if address.eq_ignore_ascii_case("0x0000000000000000000000000000000000000000") {
            Some("MON")
        } else if address.eq_ignore_ascii_case("0x3bd359c1119da7da1d913d1c4d2b7c461115433a") {
            Some("WMON")
        } else {
            None
        };
        if let Some(symbol) = symbol {
            p[format!("token_{side}_symbol")] = json!(symbol);
        }
    }
    p["pair"] = json!(format!(
        "{}/{}",
        p["token_a_symbol"].as_str().unwrap_or("?"),
        p["token_b_symbol"].as_str().unwrap_or("?")
    ));
    p
}
fn catalog(s: &AppState, filter: &str) -> Result<Vec<Value>> {
    let mut rows = read_db(&s.sqlite_path,&format!("SELECT * FROM pools WHERE lower(chain)='monad' AND ({filter}) ORDER BY coalesce(liquidity_usd,0) DESC,pool_address"))?;
    if rows.is_empty() {
        return Ok(rows);
    }
    let addresses = rows
        .iter()
        .flat_map(|p| {
            ["token_a_address", "token_b_address"]
                .into_iter()
                .filter_map(|key| p[key].as_str())
        })
        .map(|a| format!("'{}'", escape(&a.to_ascii_lowercase())))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(",");
    // Verified token metadata wins over stale pool-side labels. A mislabeled
    // USDC side must never become a WETH search candidate.
    if !read_db(
        &s.sqlite_path,
        "SELECT name FROM sqlite_master WHERE type='table' AND name='token_metadata'",
    )?
    .is_empty()
    {
        let metadata=read_db(&s.sqlite_path,&format!("SELECT address,symbol,name FROM token_metadata WHERE lower(chain)='monad' AND status='resolved' AND lower(address) IN ({addresses})"))?;
        let metadata = metadata
            .iter()
            .filter_map(|m| m["address"].as_str().map(|a| (a.to_ascii_lowercase(), m)))
            .collect::<HashMap<_, _>>();
        for pool in &mut rows {
            for side in ["a", "b"] {
                if let Some(m) = pool[format!("token_{side}_address")]
                    .as_str()
                    .and_then(|a| metadata.get(&a.to_ascii_lowercase()))
                {
                    if m["symbol"].as_str().is_some_and(|s| !s.is_empty()) {
                        pool[format!("token_{side}_symbol")] = m["symbol"].clone();
                    }
                    pool[format!("token_{side}_name")] = m["name"].clone();
                }
            }
        }
    }
    Ok(rows.into_iter().map(canonical_labels).collect())
}

fn token_filter(token: &str) -> String {
    let token = escape(&token.to_ascii_lowercase());
    format!("lower(token_a_address)='{token}' OR lower(token_b_address)='{token}'")
}
fn eq(v: &Value, key: &str, text: &str) -> bool {
    v[key]
        .as_str()
        .is_some_and(|s| s.eq_ignore_ascii_case(text))
}
fn token_name_matches(p: &Value, side: &str, input: &str) -> bool {
    let symbol = p[format!("token_{side}_symbol")].as_str().unwrap_or("");
    symbol.eq_ignore_ascii_case(input)
        || eq(p, &format!("token_{side}_name"), input)
        || (matches!(input.to_ascii_uppercase().as_str(), "ETH" | "ETHEREUM")
            && symbol.eq_ignore_ascii_case("WETH"))
}
fn resolve_rows(rows: &[Value], input: &str) -> Value {
    let mut candidates = BTreeMap::new();
    let mut kind = "symbol";
    let matched = rows
        .iter()
        .filter(|p| eq(p, "pool_address", input))
        .collect::<Vec<_>>();
    if let Some(p) = matched.first() {
        kind = "pool";
        let a = p["token_a_symbol"].as_str().unwrap_or("");
        let key = if matches!(
            a.to_uppercase().as_str(),
            "MON" | "WMON" | "USDC" | "USDT" | "AUSD"
        ) {
            "token_b_address"
        } else {
            "token_a_address"
        };
        if let Some(a) = p[key].as_str() {
            candidates.insert(a.to_lowercase(), p[key].clone());
        }
    } else {
        for p in rows {
            for side in ["a", "b"] {
                let ak = format!("token_{side}_address");
                let sk = format!("token_{side}_symbol");
                if eq(p, &ak, input) || (!address(input) && token_name_matches(p, side, input)) {
                    if eq(p, &ak, input) {
                        kind = "address";
                    }
                    if let Some(a) = p[&ak].as_str() {
                        candidates.insert(a.to_lowercase(), json!({"address":a,"symbol":p[&sk]}));
                    }
                }
            }
        }
    }
    let choices = candidates
        .values()
        .map(|v| {
            if v.is_string() {
                json!({"address":v})
            } else {
                v.clone()
            }
        })
        .collect::<Vec<_>>();
    let token = (choices.len() == 1).then(|| choices[0]["address"].clone());
    json!({"success":token.is_some(),"input_address":input,"address_type":kind,"resolved_token":token,"candidates":choices,"requires_selection":choices.len()>1,"hint":if choices.len()>1 {"Choose an explicit token address"} else if choices.is_empty(){"No Monad catalog match"} else {"Resolved from Monad catalog"}})
}
fn select(rows: Vec<Value>, token: &str) -> Vec<Value> {
    let mut counts = HashMap::new();
    let mut ranked = Vec::new();
    for mut p in rows
        .into_iter()
        .filter(|p| eq(p, "token_a_address", token) || eq(p, "token_b_address", token))
    {
        if !matches!(
            p["pool_type"].as_str(),
            Some("constant_product_v2" | "solidly_v2" | "concentrated_v3" | "uniswap_v4")
        ) {
            continue;
        }
        let n = counts
            .entry(p["protocol"].as_str().unwrap_or("").to_owned())
            .or_insert(0);
        *n += 1;
        if *n <= 2 {
            p["protocol_rank"] = json!(*n);
            ranked.push(p)
        }
    }
    ranked.sort_by(|a, b| {
        a["protocol_rank"]
            .as_i64()
            .cmp(&b["protocol_rank"].as_i64())
            .then_with(|| {
                b["liquidity_usd"]
                    .as_f64()
                    .unwrap_or(0.)
                    .total_cmp(&a["liquidity_usd"].as_f64().unwrap_or(0.))
            })
            .then_with(|| a["pool_address"].as_str().cmp(&b["pool_address"].as_str()))
    });
    ranked.truncate(5);
    ranked
}
pub async fn resolve(
    State(s): State<Arc<AppState>>,
    Query(q): Query<SolanaResolveQuery>,
) -> ApiResult {
    let input = q.address.as_deref().unwrap_or("").trim();
    if input.is_empty() {
        return Err(bad(StatusCode::BAD_REQUEST, "address is required"));
    }
    let raw = escape(&input.to_ascii_lowercase());
    let symbols = if matches!(raw.as_str(), "eth" | "ethereum") {
        format!("'{raw}','weth'")
    } else {
        format!("'{raw}'")
    };
    let mut filter=format!("lower(pool_address)='{raw}' OR lower(token_a_address)='{raw}' OR lower(token_b_address)='{raw}' OR lower(token_a_symbol) IN ({symbols}) OR lower(token_b_symbol) IN ({symbols})");
    if !read_db(
        &s.sqlite_path,
        "SELECT name FROM sqlite_master WHERE type='table' AND name='token_metadata'",
    )
    .map_err(internal)?
    .is_empty()
    {
        let tokens=read_db(&s.sqlite_path,&format!("SELECT address FROM token_metadata WHERE lower(chain)='monad' AND status='resolved' AND (lower(symbol) IN ({symbols}) OR lower(name)='{raw}')")).map_err(internal)?;
        let addresses = tokens
            .iter()
            .filter_map(|m| m["address"].as_str())
            .map(|a| format!("'{}'", escape(&a.to_ascii_lowercase())))
            .collect::<Vec<_>>()
            .join(",");
        if !addresses.is_empty() {
            filter.push_str(&format!(" OR lower(token_a_address) IN ({addresses}) OR lower(token_b_address) IN ({addresses})"));
        }
    }
    Ok(Json(resolve_rows(
        &catalog(&s, &filter).map_err(internal)?,
        input,
    )))
}
pub async fn pools(State(s): State<Arc<AppState>>, Query(q): Query<SolanaPoolQuery>) -> ApiResult {
    let token = q.token.unwrap_or_default();
    if !address(&token) {
        return Err(bad(
            StatusCode::BAD_REQUEST,
            "explicit token address required",
        ));
    }
    let config = store::display(&store::open().map_err(internal)?).map_err(internal)?;
    let catalog_rows = catalog(&s, &token_filter(&token)).map_err(internal)?;
    let rows = if eq(&config, "token", &token) {
        config["pools"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|saved| {
                let mut p = catalog_rows
                    .iter()
                    .find(|p| {
                        eq(
                            p,
                            "pool_address",
                            saved["pool_address"].as_str().unwrap_or(""),
                        )
                    })
                    .cloned()
                    .unwrap_or(saved.clone());
                p["protocol_rank"] = saved["protocol_rank"].clone();
                p
            })
            .collect()
    } else {
        select(catalog_rows, &token)
    };
    let mut enriched = Vec::new();
    for mut p in rows {
        let pool = p["pool_address"].as_str().unwrap_or_default().to_owned();
        for key in ["vol", "fee_apr", "price", "volume", "tvl"] {
            p[format!("monad_{key}")] =
                metric_value(&s, &pool, key, Some(&token)).map_err(internal)?;
        }
        enriched.push(p);
    }
    Ok(Json(
        json!({"token":token,"pools":enriched,"pool_count":enriched.len(),"source":"catalog_snapshot"}),
    ))
}
pub async fn display() -> ApiResult {
    Ok(Json(
        store::display(&store::open().map_err(internal)?).map_err(internal)?,
    ))
}
pub async fn save(
    State(s): State<Arc<AppState>>,
    payload: std::result::Result<Json<SolanaStartRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    let Json(q) = payload.map_err(|_| {
        bad(
            StatusCode::BAD_REQUEST,
            "token must be a string in a JSON object",
        )
    })?;
    if !address(&q.token) {
        return Err(bad(
            StatusCode::BAD_REQUEST,
            "explicit token address required",
        ));
    }
    let rows = select(
        catalog(&s, &token_filter(&q.token)).map_err(internal)?,
        &q.token,
    );
    if rows.is_empty() {
        return Err(bad(
            StatusCode::NOT_FOUND,
            "no supported Monad pools for token",
        ));
    }
    let v =
        store::save(&mut store::open().map_err(internal)?, &q.token, &rows).map_err(internal)?;
    Ok(Json(
        json!({"listener":true,"token":q.token,"pools":v["pools"],"pool_ids":v["pool_ids"],"version":v["version"]}),
    ))
}
#[derive(Deserialize)]
pub struct MetricQuery {
    base: Option<String>,
}
type Cache = HashMap<String, (Instant, Value)>;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

// Keep market snapshots distinct from RPC spot observations and old catalogs.
// No metric GET performs provider I/O or refreshes collection timestamps.
fn live_market_metric(s: &AppState, pool: &str, metric: &str) -> Result<Option<Value>> {
    if !matches!(metric, "volume" | "tvl" | "fee_apr") {
        return Ok(None);
    }
    let Some(c) = store::read_connection()? else {
        return Ok(None);
    };
    if c.query_row(
        "SELECT 1 FROM sqlite_master WHERE name='live_market'",
        [],
        |r| r.get::<_, i32>(0),
    )
    .optional()?
    .is_none()
    {
        return Ok(None);
    }
    let raw: Option<String> = c
        .query_row(
            "SELECT body FROM live_market WHERE lower(pool)=lower(?1)",
            [pool],
            |r| r.get(0),
        )
        .optional()?;
    let Some(raw) = raw else { return Ok(None) };
    let market: Value = serde_json::from_str(&raw)?;
    let market_time = market["as_of"].as_i64().unwrap_or(0);
    let mut as_of = market_time;
    let mut rpc_fee_time = None;
    let mut extra = json!({"source":market["source"],"as_of_kind":market["as_of_kind"],"source_as_of":market["source_as_of"],"observed_at":market["observed_at"],"provider_cache_age_seconds":market["provider_cache_age_seconds"]});
    let value = if metric == "fee_apr" {
        let snapshot: Option<String> = c
            .query_row(
                "SELECT body FROM snapshots WHERE lower(pool)=lower(?1)",
                [pool],
                |r| r.get(0),
            )
            .optional()?;
        let snapshot: Value = snapshot
            .map(|v| serde_json::from_str(&v))
            .transpose()?
            .unwrap_or(json!({}));
        let q = &snapshot["snapshot"]["decoded_price"];
        let Some(fee) = q["fee_bps"].as_f64() else {
            return Ok(None);
        };
        let fee_time = q["as_of"].as_i64().unwrap_or(0);
        rpc_fee_time = Some(fee_time);
        as_of = as_of.min(fee_time);
        extra["fee_as_of"] = json!(fee_time);
        extra["fee_bps"] = json!(fee);
        extra["volume_as_of"] = json!(market_time);
        extra["tvl_as_of"] = json!(market_time);
        extra["fee_source"] = json!("monad_listener_rpc");
        let tvl = market["tvl_usd"].as_f64().unwrap_or(0.);
        let Some(volume) = market["volume_usd"].as_f64() else {
            return Ok(None);
        };
        let share=read_db(&s.sqlite_path,&format!("SELECT lp_fee_share_assumption FROM pools WHERE chain='monad' AND pool_address='{}'",escape(&pool.to_ascii_lowercase())))?.first().and_then(|r|r["lp_fee_share_assumption"].as_f64()).unwrap_or(1.);
        if tvl > 0. {
            Some(volume * fee / 10000. * 365. / tvl * share)
        } else {
            None
        }
    } else {
        market[if metric == "volume" {
            "volume_usd"
        } else {
            "tvl_usd"
        }]
        .as_f64()
    };
    let value = value.filter(|v| v.is_finite() && *v >= 0.);
    let stale =
        store::now() - market_time > 120 || rpc_fee_time.is_some_and(|t| store::now() - t > 30);
    let mut result = json!({"pool":pool,"value":value,"unit":if metric=="fee_apr" {"annual_fraction"}else{"USD"},"as_of":as_of,"status":if value.is_none(){"insufficient_data"}else if stale{"stale"}else{"ok"}});
    result
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    Ok(Some(result))
}

pub async fn demo(State(s): State<Arc<AppState>>) -> ApiResult {
    let mut pools = store::fixed_pools().map_err(internal)?;
    for pool in &mut pools {
        let id = pool["pool_address"].as_str().unwrap_or("").to_owned();
        let base = pool["base_token"].as_str().unwrap_or("").to_owned();
        for kind in ["vol", "fee_apr", "price", "volume"] {
            pool[kind] = metric_value(&s, &id, kind, Some(&base)).map_err(internal)?;
        }
    }
    Ok(Json(
        json!({"version":1,"pool_count":pools.len(),"poll_ms":store::poll_ms(),"pools":pools}),
    ))
}

#[derive(Deserialize)]
pub struct PricesQuery {
    pools: Option<String>,
    base: Option<String>,
}

pub async fn prices(State(s): State<Arc<AppState>>, Query(q): Query<PricesQuery>) -> ApiResult {
    if q.base.as_deref().is_some_and(|base| !address(base)) {
        return Err(bad(StatusCode::BAD_REQUEST, "base must be a token address"));
    }
    let fixed = store::fixed_pools().map_err(internal)?;
    let mut allowed = fixed.clone();
    if let Some(c) = store::read_connection().map_err(internal)? {
        let display = store::display(&c).map_err(internal)?;
        for pool in display["pools"].as_array().into_iter().flatten() {
            if !allowed
                .iter()
                .any(|p| p["pool_address"] == pool["pool_address"])
            {
                allowed.push(pool.clone())
            }
        }
    }
    let ids = q
        .pools
        .map(|s| {
            s.split(',')
                .map(|id| id.trim().to_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            fixed
                .iter()
                .filter_map(|p| p["pool_address"].as_str().map(str::to_owned))
                .collect()
        });
    if ids.is_empty() || ids.len() > 20 || ids.iter().any(|id| id.is_empty()) {
        return Err(bad(
            StatusCode::BAD_REQUEST,
            "request between 1 and 20 pools",
        ));
    }
    let mut values = Vec::new();
    for id in ids {
        let pool = allowed
            .iter()
            .find(|p| eq(p, "pool_address", &id))
            .ok_or_else(|| bad(StatusCode::NOT_FOUND, "pool is not monitored"))?;
        let base = q
            .base
            .as_deref()
            .or(pool["base_token"].as_str())
            .or(pool["token_a_address"].as_str())
            .unwrap_or("");
        if !eq(pool, "token_a_address", base) && !eq(pool, "token_b_address", base) {
            return Err(bad(StatusCode::BAD_REQUEST, "base is not in this pool"));
        }
        values.push(
            metric_value(
                &s,
                pool["pool_address"].as_str().unwrap(),
                "price",
                Some(base),
            )
            .map_err(internal)?,
        );
    }
    Ok(Json(
        json!({"price_poll_ms":store::price_poll_ms(),"prices":values}),
    ))
}
pub async fn prices_stream(State(s):State<Arc<AppState>>) -> axum::response::Response {
    let (tx,rx)=tokio::sync::mpsc::channel(1);
    tokio::spawn(async move {
        let mut timer=tokio::time::interval(Duration::from_secs(1));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select!{_=tx.closed()=>break,_=timer.tick()=>{}}
            let result=prices(State(s.clone()),Query(PricesQuery{pools:None,base:None})).await;
            let value=match result {Ok(Json(value))=>value,Err(_)=>json!({"error":"价格读取失败"})};
            if tx.send(value).await.is_err(){break}
        }
    });
    crate::selection::response(rx)
}
pub async fn metric(
    State(s): State<Arc<AppState>>,
    Path((pool, metric)): Path<(String, String)>,
    Query(q): Query<MetricQuery>,
) -> ApiResult {
    if !matches!(
        metric.as_str(),
        "vol" | "fee_apr" | "price" | "volume" | "tvl"
    ) {
        return Err(bad(StatusCode::NOT_FOUND, "unknown metric"));
    }
    if metric == "price" && !q.base.as_deref().is_some_and(address) {
        return Err(bad(StatusCode::BAD_REQUEST, "base must be a token address"));
    }
    let exists=read_db(&s.sqlite_path,&format!("SELECT token_a_address,token_b_address FROM pools WHERE chain='monad' AND pool_address='{}'",escape(&pool.to_ascii_lowercase()))).map_err(internal)?;
    if exists.is_empty() {
        return Err(bad(StatusCode::NOT_FOUND, "pool not found"));
    }
    if metric == "price"
        && !exists.iter().any(|p| {
            eq(p, "token_a_address", q.base.as_deref().unwrap())
                || eq(p, "token_b_address", q.base.as_deref().unwrap())
        })
    {
        return Err(bad(StatusCode::BAD_REQUEST, "base is not in this pool"));
    }
    Ok(Json(
        metric_value(&s, &pool, &metric, q.base.as_deref()).map_err(internal)?,
    ))
}
fn metric_value(s: &AppState, pool: &str, metric: &str, base: Option<&str>) -> Result<Value> {
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = format!(
        "{}:{pool}:{metric}:{}",
        s.sqlite_path.display(),
        base.unwrap_or("")
    )
    .to_lowercase();
    if let Some((t, v)) = cache.lock().unwrap().get(&key) {
        if t.elapsed()
            < if metric == "price" {
                Duration::from_millis(200)
            } else {
                Duration::from_secs(2)
            }
        {
            return Ok(v.clone());
        }
    }
    if let Some(value) = live_market_metric(s, pool, metric)? {
        cache
            .lock()
            .unwrap()
            .insert(key, (Instant::now(), value.clone()));
        return Ok(value);
    }
    let filter = format!(
        "p.chain='monad' AND p.pool_address='{}'",
        escape(&pool.to_ascii_lowercase())
    );
    let (mut value, mut timestamp, mut reason) =
        (Value::Null, Value::Null, "insufficient_data".to_string());
    let unit = match metric {
        "vol" => "daily_fraction",
        "fee_apr" => "annual_fraction",
        "price" => "quote_per_base",
        _ => "USD",
    };
    let mut extra = json!({});
    match metric {
        "tvl" | "volume" => {
            let col = if metric == "tvl" {
                "liquidity_usd"
            } else {
                "volume_24h_usd"
            };
            if let Some(r) = read_db(
                &s.sqlite_path,
                &format!("SELECT {col} AS value,updated_at AS as_of,coalesce(unixepoch(updated_at),CAST(updated_at AS INTEGER)) AS source_time FROM pools p WHERE {filter}"),
            )?
            .first()
            {
                value = r["value"].clone();
                timestamp = r["as_of"].clone();
                if r["source_time"].as_i64().is_some_and(|t|store::now()-t>120) {reason="stale".into();}
                extra["source"]=json!("catalog_snapshot");
            }
        }
        "fee_apr" => {
            // TVL is only a positive denominator here, never a minimum-size filter.
            let formula = expand_fee_sql(FEE_APR).replace(
                "coalesce(p.liquidity_usd, 0) >= 100",
                "coalesce(p.liquidity_usd, 0) > 0",
            );
            let sql=format!("SELECT ({formula}) AS value,p.updated_at AS as_of,s.checked_at AS fee_as_of FROM pools p LEFT JOIN pool_rpc_state s ON s.pool_address=p.pool_address AND s.chain=p.chain WHERE {filter}");
            if let Some(r) = read_db(&s.sqlite_path, &sql)?.first() {
                value = r["value"].clone();
                timestamp = r["as_of"].clone();
                extra["fee_as_of"] = r["fee_as_of"].clone();
                extra["source"] = json!("catalog_snapshot");
                // Missing current volume/TVL cannot be made live by a fresh fee read.
                reason = "stale".into();
            }
        }
        "price" => {
            let raw: Option<String> = if let Some(c) = store::read_connection()? {
                c.query_row(
                    "SELECT body FROM snapshots WHERE lower(pool)=lower(?1)",
                    [pool],
                    |r| r.get(0),
                )
                .optional()?
            } else {
                None
            };
            let r = raw
                .map(|x| serde_json::from_str::<Value>(&x))
                .transpose()?
                .unwrap_or(json!({}));
            let mut q = r["snapshot"]["decoded_price"].clone();
            let mut source = "monad_listener_rpc".to_string();
            // The listener watches five display pools. Other known pools can
            // still expose their last real pool ratio, with its original time.
            // A stale snapshot must not hide newer externally observed history.
            if q["as_of"].as_i64().is_none_or(|t| store::now() - t > 30) {
                let history = read_db(&s.sqlite_path,&format!("SELECT token0_address,token1_address,token1_per_token0,observed_at,source FROM pool_price_history WHERE chain='monad' AND pool_address='{}' AND price_status='priced' AND token1_per_token0>0 ORDER BY timestamp DESC LIMIT 1",escape(&pool.to_ascii_lowercase())))?;
                if let Some(h) = history.first().filter(|h| {
                    h["observed_at"].as_i64().unwrap_or(0) > q["as_of"].as_i64().unwrap_or(0)
                }) {
                    q = json!({"token0_address":h["token0_address"],"token1_address":h["token1_address"],"token1_per_token0":h["token1_per_token0"],"as_of":h["observed_at"]});
                    source = h["source"].as_str().unwrap_or("pool_history").to_string();
                }
            }
            let fallback = read_db(
                &s.sqlite_path,
                &format!("SELECT token_a_address,token_b_address FROM pools p WHERE {filter}"),
            )?
            .into_iter()
            .next()
            .unwrap_or(json!({}));
            extra = json!({"base_token":base,"quote_token":if base.is_some_and(|b|eq(&fallback,"token_a_address",b)){fallback["token_b_address"].clone()}else{fallback["token_a_address"].clone()}});
            let a = q["token0_address"].as_str().unwrap_or("");
            let b = q["token1_address"].as_str().unwrap_or("");
            let ratio = q["token1_per_token0"].as_f64();
            if let Some(base) = base {
                if base.eq_ignore_ascii_case(a) {
                    value = json!(ratio);
                    extra = json!({"base_token":a,"quote_token":b});
                } else if base.eq_ignore_ascii_case(b) {
                    value = json!(ratio.filter(|x| *x > 0.).map(|x| 1. / x));
                    extra = json!({"base_token":b,"quote_token":a});
                }
            }
            timestamp = q["as_of"].clone();
            extra["state_block"] = q["state_block"].clone();
            extra["source"] = json!(source);
            extra["price_semantics"] = json!("pool_token_ratio");
            if timestamp.as_i64().is_some_and(|t| store::now() - t > 30) {
                reason = "stale".into();
            }
        }
        "vol" => {
            let hour = store::now() / 3600 * 3600;
            let rows=read_db(&s.sqlite_path,&format!("SELECT timestamp,token1_per_token0 AS price,observed_at,source FROM pool_price_history WHERE chain='monad' AND pool_address='{}' AND price_status='priced' AND timestamp<{hour} ORDER BY timestamp DESC LIMIT 24",escape(&pool.to_ascii_lowercase())))?;
            let sources = rows
                .iter()
                .filter_map(|r| r["source"].as_str())
                .collect::<std::collections::BTreeSet<_>>();
            extra = json!({"samples":rows.len(),"source":if sources.len()==1 {sources.iter().next().copied().unwrap_or("pool_history")}else{"mixed_pool_history"},"sources":sources,"price_semantics":"pool_token_ratio","window_start":rows.last().map(|r|r["timestamp"].clone()),"window_end":rows.first().and_then(|r|r["timestamp"].as_i64()).map(|t|t+3600)});
            if rows.len() == 24
                && rows.windows(2).all(|w| {
                    w[0]["timestamp"].as_i64().unwrap_or(0)
                        - w[1]["timestamp"].as_i64().unwrap_or(0)
                        == 3600
                })
                && rows
                    .iter()
                    .all(|r| r["price"].as_f64().is_some_and(|x| x.is_finite() && x > 0.))
            {
                let points = rows
                    .iter()
                    .rev()
                    .map(|r| {
                        (
                            r["timestamp"].as_i64().unwrap(),
                            r["price"].as_f64().unwrap(),
                        )
                    })
                    .collect::<Vec<_>>();
                value = json!(lp_metrics::consecutive_hourly_daily_volatility(&points));
                timestamp = rows[0]["observed_at"].clone();
            } else {
                reason = "requires_24_consecutive_hourly_pool_prices".into();
                timestamp = rows
                    .first()
                    .map(|r| r["observed_at"].clone())
                    .unwrap_or(Value::Null);
            }
        }
        _ => {}
    }
    if metric == "vol"
        && !value.is_null()
        && timestamp.as_i64().is_some_and(|t| store::now() - t > 7200)
    {
        reason = "stale".into();
    }
    if !value.is_null() && reason != "stale" {
        reason = "ok".into()
    }
    let mut v = json!({"pool":pool,"value":value,"unit":unit,"as_of":timestamp,"status":reason});
    if let Some(obj) = extra.as_object() {
        v.as_object_mut().unwrap().extend(obj.clone());
    }
    if metric == "price" {
        v["base_token"] = v.get("base_token").cloned().unwrap_or(json!(base));
        v["quote_token"] = v.get("quote_token").cloned().unwrap_or(Value::Null);
    }
    cache
        .lock()
        .unwrap()
        .insert(key, (Instant::now(), v.clone()));
    Ok(v)
}
pub async fn proxy(
    State(s): State<Arc<AppState>>,
    Path(endpoint): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    if !matches!(
        endpoint.as_str(),
        "events" | "whale-alerts" | "pool-history" | "pool-snapshots" | "protocols"
    ) {
        return Err(bad(StatusCode::NOT_FOUND, "unknown endpoint"));
    }
    let r = s
        .rpc_http
        .get(format!(
            "{}/monad/{endpoint}",
            s.rpc_price_url.trim_end_matches('/')
        ))
        .query(&q)
        .send()
        .await
        .map_err(|e| internal(e.into()))?;
    let status = r.status();
    let v = r.json::<Value>().await.map_err(|e| internal(e.into()))?;
    if !status.is_success() {
        return Err(bad(
            status,
            v["error"].as_str().unwrap_or("RPC listener unavailable"),
        ));
    }
    Ok(Json(v))
}
pub async fn status(State(s): State<Arc<AppState>>) -> ApiResult {
    let r = s
        .rpc_http
        .get(format!(
            "{}/monad/listener/status",
            s.rpc_price_url.trim_end_matches('/')
        ))
        .send()
        .await
        .map_err(|e| internal(e.into()))?;
    Ok(Json(r.json().await.map_err(|e| internal(e.into()))?))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotas_and_ambiguous_symbols() {
        let mut rows = vec![];
        for i in 0..12 {
            rows.push(json!({"pool_address":format!("pool{i}"),"protocol":format!("p{}",i/4),"pool_type":"concentrated_v3","token_a_address":format!("0x{:040x}",i%2),"token_a_symbol":"SAME","token_b_address":"other","liquidity_usd":100-i}));
        }
        let r = resolve_rows(&rows, "SAME");
        assert_eq!(r["requires_selection"], true);
        assert!(r["resolved_token"].is_null());
        let selected = select(rows, "other");
        assert_eq!(selected.len(), 5);
        assert!(selected[..3].iter().all(|p| p["protocol_rank"] == 1));
    }
    #[test]
    fn eth_resolves_wrapped_eth_without_matching_derivatives() {
        let rows = vec![
            json!({"token_a_symbol":"WETH","token_a_address":"0x1111111111111111111111111111111111111111","token_b_symbol":"wstETH","token_b_address":"0x2222222222222222222222222222222222222222"}),
        ];
        assert_eq!(
            resolve_rows(&rows, "eth")["resolved_token"],
            rows[0]["token_a_address"]
        );
        assert_eq!(
            resolve_rows(&rows, "Ethereum")["resolved_token"],
            rows[0]["token_a_address"]
        );
    }
    #[test]
    fn mon_is_not_wmon() {
        let rows = vec![
            json!({"token_a_symbol":"MON","token_a_address":"0x0000000000000000000000000000000000000000","token_b_symbol":"WMON","token_b_address":"0x0000000000000000000000000000000000000001"}),
        ];
        assert_ne!(
            resolve_rows(&rows, "MON")["resolved_token"],
            resolve_rows(&rows, "WMON")["resolved_token"]
        );
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    #[tokio::test]
    async fn trade_rpc_normalizes_omitted_no_argument_params() {
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = tcp.local_addr().unwrap();
        let upstream = Router::new().route(
            "/rpc/monad",
            axum::routing::post(|Json(body): Json<Value>| async move {
                assert_eq!(body["params"], json!([]));
                assert_eq!(body["jsonrpc"], "2.0");
                Json(json!({"jsonrpc":"2.0","id":body["id"],"result":"0x123"}))
            }),
        );
        let server = tokio::spawn(async move { axum::serve(tcp, upstream).await.unwrap() });
        let state = Arc::new(AppState {
            sqlite_path: PathBuf::new(),
            market_db_path: PathBuf::new(),
            rpc_refresh_path: PathBuf::new(),
            watchlist_user_id: "test".into(),
            rpc_price_url: format!("http://{address}"),
            rpc_http: reqwest::Client::new(),
            vybe_api_key: None,
        });
        for method in ["eth_gasPrice", "eth_blockNumber", "eth_chainId"] {
            for request in [
                json!({"id":7,"method":method}),
                json!({"id":7,"method":method,"params":[]}),
            ] {
                let response = trade_rpc(State(state.clone()), Json(request))
                    .await
                    .unwrap()
                    .0;
                assert_eq!(response["result"], "0x123");
                assert_eq!(response["id"], 7);
            }
        }
        for request in [
            json!({"method":"eth_call"}),
            json!({"method":"eth_gasPrice","params":{}}),
            json!({"method":"eth_sendTransaction","params":[]}),
        ] {
            assert_eq!(
                trade_rpc(State(state.clone()), Json(request))
                    .await
                    .unwrap_err()
                    .0,
                StatusCode::BAD_REQUEST
            );
        }
        server.abort();
    }
    #[tokio::test]
    async fn metric_contract_isolation_times_nulls_direction_and_config() {
        let dir = std::env::temp_dir().join(format!(
            "monad-api-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let catalog = dir.join("catalog.db");
        let price = dir.join("price.db");
        let events = dir.join("events.db");
        std::env::set_var("LP_PRICE_DB", &price);
        std::env::set_var("MONAD_LISTENER_DB", &events);
        let a = "0x0000000000000000000000000000000000000000";
        let b = "0x1111111111111111111111111111111111111111";
        let pool = "0x2222222222222222222222222222222222222222";
        let c = Connection::open(&catalog).unwrap();
        // No history or RPC-state tables exist. These metrics must still work.
        c.execute_batch("CREATE TABLE pools(chain TEXT,pool_address TEXT,token_a_address TEXT,token_b_address TEXT,liquidity_usd REAL,volume_24h_usd REAL,updated_at INTEGER);").unwrap();
        c.execute(
            "INSERT INTO pools VALUES('monad',?1,?2,?3,1000,200,12345)",
            rusqlite::params![pool, a, b],
        )
        .unwrap();
        let s = Arc::new(AppState {
            sqlite_path: catalog.clone(),
            market_db_path: dir.join("unused"),
            rpc_refresh_path: dir.join("unused2"),
            watchlist_user_id: "test".into(),
            rpc_price_url: "http://127.0.0.1:1".into(),
            rpc_http: reqwest::Client::new(),
            vybe_api_key: None,
        });
        for method in [
            "eth_sendTransaction",
            "eth_sendRawTransaction",
            "personal_sign",
            "wallet_sendCalls",
        ] {
            let error = trade_rpc(
                State(s.clone()),
                Json(json!({"jsonrpc":"2.0","id":1,"method":method,"params":[]})),
            )
            .await
            .unwrap_err();
            assert_eq!(error.0, StatusCode::BAD_REQUEST);
        }
        let call = |kind: &str, base: Option<&str>| {
            let s = s.clone();
            let pool = pool.to_owned();
            let kind = kind.to_owned();
            let base = base.map(str::to_owned);
            async move { metric(State(s), Path((pool, kind)), Query(MetricQuery { base })).await }
        };
        let v = call("tvl", None).await.unwrap().0;
        assert_eq!(v["value"], 1000.0);
        assert_eq!(v["unit"], "USD");
        assert_eq!(v["as_of"], 12345);
        assert_eq!(call("volume", None).await.unwrap().0["value"], 200.0);
        c.execute("UPDATE pools SET liquidity_usd=9999", [])
            .unwrap();
        assert_eq!(call("tvl", None).await.unwrap().0["value"], 1000.0); // independent 10s cache
        assert_eq!(
            call("price", None).await.unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call("price", Some(pool)).await.unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
        let unknown = metric(
            State(s.clone()),
            Path(("unknown".into(), "tvl".into())),
            Query(MetricQuery { base: None }),
        )
        .await
        .unwrap_err();
        assert_eq!(unknown.0, StatusCode::NOT_FOUND);
        // Price needs only the standalone snapshot, not catalog USD ratios/history.
        let now = store::now();
        let snap = json!({"snapshot":{"decoded_price":{"token0_address":a,"token1_address":b,"token1_per_token0":2.5,"as_of":now}}});
        let mut e = store::open().unwrap();
        e.execute(
            "INSERT INTO snapshots VALUES(?1,?2)",
            rusqlite::params![pool, snap.to_string()],
        )
        .unwrap();
        let v = call("price", Some(a)).await.unwrap().0;
        assert_eq!(v["value"], 2.5);
        assert_eq!(v["quote_token"], b);
        assert_eq!(v["as_of"], now);
        assert_eq!(call("price", Some(b)).await.unwrap().0["value"], 0.4);
        let changed = json!({"snapshot":{"decoded_price":{"token0_address":a,"token1_address":b,"token1_per_token0":3.,"as_of":now}}});
        e.execute(
            "UPDATE snapshots SET body=?2 WHERE pool=?1",
            rusqlite::params![pool, changed.to_string()],
        )
        .unwrap();
        assert_eq!(call("price", Some(a)).await.unwrap().0["value"], 2.5);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(call("price", Some(a)).await.unwrap().0["value"], 3.);
        let missing = call("vol", None).await.unwrap().0;
        assert!(missing["value"].is_null());
        assert!(missing["as_of"].is_null());
        // Fee APR does not need any price/history table.
        c.execute_batch("ALTER TABLE pools ADD COLUMN pool_type TEXT;ALTER TABLE pools ADD COLUMN fee_bps REAL;ALTER TABLE pools ADD COLUMN lp_fee_share_assumption REAL;CREATE TABLE pool_rpc_state(pool_address TEXT,chain TEXT,fee_bps REAL,checked_at INTEGER);UPDATE pools SET pool_type='concentrated_v3',fee_bps=3000,lp_fee_share_assumption=1,liquidity_usd=1000;").unwrap();
        let apr = call("fee_apr", None).await.unwrap().0;
        assert!((apr["value"].as_f64().unwrap() - 0.219).abs() < 1e-12);
        assert_eq!(apr["as_of"], 12345);
        let h = Connection::open(&price).unwrap();
        h.execute_batch("CREATE TABLE pool_price_history(chain TEXT,pool_address TEXT,timestamp INTEGER,observed_at INTEGER,token1_per_token0 REAL,price_status TEXT,source TEXT,token0_address TEXT,token1_address TEXT);").unwrap();
        let hour = now / 3600 * 3600 - 3600;
        for i in 0..24 {
            h.execute(
                "INSERT INTO pool_price_history VALUES('monad',?1,?2,?2,?3,'priced','geckoterminal_pool_ratio_1h',?4,?5)",
                rusqlite::params![
                    pool,
                    hour - (23 - i) * 3600,
                    if i % 2 == 0 { 1.0 } else { 1.1 }, a, b
                ],
            )
            .unwrap();
        }
        CACHE
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .retain(|k, _| !k.contains(":vol:"));
        let v = call("vol", None).await.unwrap().0;
        assert!(v["value"].as_f64().unwrap() > 0.);
        assert_eq!(v["unit"], "daily_fraction");
        assert_eq!(v["as_of"], hour);
        assert_eq!(v["source"], "geckoterminal_pool_ratio_1h");
        assert_eq!(v["samples"], 24);
        // An unfinished hour must not displace a completed observation.
        h.execute("INSERT INTO pool_price_history VALUES('monad',?1,?2,?2,999,'priced','monad_listener_rpc',?3,?4)",rusqlite::params![pool,hour+3600,a,b]).unwrap();
        CACHE.get().unwrap().lock().unwrap().clear();
        assert_eq!(call("vol", None).await.unwrap().0["value"], v["value"]);
        h.execute(
            "DELETE FROM pool_price_history WHERE timestamp=?1",
            [hour + 3600],
        )
        .unwrap();
        // Other pools need no active listener snapshot to expose a real ratio.
        e.execute("DELETE FROM snapshots", []).unwrap();
        CACHE.get().unwrap().lock().unwrap().clear();
        let historical = call("price", Some(b)).await.unwrap().0;
        assert_eq!(historical["value"], 1.0 / 1.1);
        assert_eq!(historical["as_of"], hour);
        assert_eq!(historical["status"], "stale");
        assert_eq!(historical["source"], "geckoterminal_pool_ratio_1h");
        // Small pools remain eligible; TVL is only the APR denominator.
        c.execute("UPDATE pools SET liquidity_usd=10", []).unwrap();
        CACHE.get().unwrap().lock().unwrap().clear();
        assert!(
            (call("fee_apr", None).await.unwrap().0["value"]
                .as_f64()
                .unwrap()
                - 21.9)
                .abs()
                < 1e-9
        );
        h.execute(
            "DELETE FROM pool_price_history WHERE timestamp=?1",
            [hour - 10 * 3600],
        )
        .unwrap();
        CACHE
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .retain(|k, _| !k.contains(":vol:"));
        assert!(call("vol", None).await.unwrap().0["value"].is_null());
        // Current market observations override old catalog volume/TVL, while
        // APR combines actual RPC fee time with market observation time.
        let live_snapshot = json!({"snapshot":{"decoded_price":{"fee_bps":5.,"as_of":now}}});
        e.execute(
            "INSERT INTO snapshots VALUES(?1,?2)",
            rusqlite::params![pool, live_snapshot.to_string()],
        )
        .unwrap();
        let mut market = json!({"volume_usd":1000.,"tvl_usd":100.,"as_of":now-10,"observed_at":now,"source_as_of":null,"source":"geckoterminal_pool_snapshot","as_of_kind":"provider_snapshot_observation"});
        e.execute(
            "INSERT INTO live_market VALUES(?1,?2)",
            rusqlite::params![pool, market.to_string()],
        )
        .unwrap();
        CACHE.get().unwrap().lock().unwrap().clear();
        let live_volume = call("volume", None).await.unwrap().0;
        assert_eq!(live_volume["value"], 1000.);
        assert_eq!(live_volume["as_of"], now - 10);
        assert_eq!(live_volume["status"], "ok");
        assert!(live_volume["source_as_of"].is_null());
        let live_apr = call("fee_apr", None).await.unwrap().0;
        assert!((live_apr["value"].as_f64().unwrap() - 1.825).abs() < 1e-9);
        assert_eq!(live_apr["fee_as_of"], now);
        assert_eq!(live_apr["as_of"], now - 10);
        market["as_of"] = json!(now - 50);
        e.execute(
            "UPDATE live_market SET body=?2 WHERE pool=?1",
            rusqlite::params![pool, market.to_string()],
        )
        .unwrap();
        CACHE.get().unwrap().lock().unwrap().clear();
        assert_eq!(call("fee_apr", None).await.unwrap().0["status"], "ok");
        market["as_of"] = json!(now - 300);
        e.execute(
            "UPDATE live_market SET body=?2 WHERE pool=?1",
            rusqlite::params![pool, market.to_string()],
        )
        .unwrap();
        CACHE.get().unwrap().lock().unwrap().clear();
        assert_eq!(call("volume", None).await.unwrap().0["status"], "stale");
        assert_eq!(call("fee_apr", None).await.unwrap().0["status"], "stale");
        let pools = vec![json!({"pool_address":pool})];
        let v = store::save(&mut e, a, &pools).unwrap();
        let same = store::save(&mut e, a, &pools).unwrap();
        assert_eq!(same["version"], v["version"]);
        let switched = store::save(&mut e, b, &pools).unwrap();
        assert_eq!(switched["version"], 2);
        assert_eq!(store::display(&store::open().unwrap()).unwrap()["token"], b);
        c.execute_batch("ALTER TABLE pools ADD COLUMN token_a_symbol TEXT;ALTER TABLE pools ADD COLUMN token_b_symbol TEXT;CREATE TABLE token_metadata(chain TEXT,address TEXT,symbol TEXT,name TEXT,status TEXT);UPDATE pools SET token_a_symbol='WETH',token_b_symbol='WETH';").unwrap();
        let mislabeled = "0x9999999999999999999999999999999999999999";
        c.execute("UPDATE pools SET token_a_address=?1", [mislabeled])
            .unwrap();
        c.execute("INSERT INTO token_metadata VALUES('monad',?1,'USDC',NULL,'resolved'),('monad',?2,'WETH',NULL,'resolved')",rusqlite::params![mislabeled,b]).unwrap();
        let labels = super::catalog(&s, "1=1").unwrap();
        assert_eq!(resolve_rows(&labels, "eth")["resolved_token"], b);
        assert_eq!(resolve_rows(&labels, "eth")["requires_selection"], false);
        drop((e, c, h));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
