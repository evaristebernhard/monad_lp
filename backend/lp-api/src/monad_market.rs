//! Web chart OHLCV, independent of listener logs and ESP metric calculations.
use super::*;
use anyhow::bail;
static FETCH: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
fn cache_db(path: &PathBuf) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let c = Connection::open(path)?;
    c.busy_timeout(Duration::from_secs(5))?;
    c.execute_batch("PRAGMA journal_mode=WAL;CREATE TABLE IF NOT EXISTS monad_ohlcv_cache(pool TEXT NOT NULL,token TEXT NOT NULL,resolution TEXT NOT NULL,limit_count INTEGER NOT NULL,body TEXT NOT NULL,fetched_at INTEGER NOT NULL,PRIMARY KEY(pool,token,resolution,limit_count));")?;
    Ok(c)
}
fn cached(
    path: &PathBuf,
    pool: &str,
    token: &str,
    resolution: &str,
    limit: i64,
) -> Result<Option<(Value, i64)>> {
    cache_db(path)?.query_row("SELECT body,fetched_at FROM monad_ohlcv_cache WHERE pool=?1 AND token=?2 AND resolution=?3 AND limit_count=?4",rusqlite::params![pool,token,resolution,limit],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?))).optional()?.map(|(body,time)|Ok((serde_json::from_str(&body)?,time))).transpose()
}
fn interval(resolution: &str) -> Option<(&'static str, &'static str)> {
    match resolution {
        "1m" => Some(("minute", "1")),
        "5m" => Some(("minute", "5")),
        "15m" => Some(("minute", "15")),
        "1h" => Some(("hour", "1")),
        _ => None,
    }
}
fn decode(payload: &Value) -> Result<Vec<Value>> {
    let rows = payload
        .pointer("/data/attributes/ohlcv_list")
        .and_then(Value::as_array)
        .context("OHLCV rows missing")?;
    let mut data = Vec::new();
    for row in rows {
        let Some(r) = row.as_array().filter(|r| r.len() >= 6) else {
            continue;
        };
        let Some(t) = market_timestamp(&r[0]).filter(|t| *t > 0) else {
            continue;
        };
        let values = r[1..6]
            .iter()
            .map(market_number)
            .collect::<Option<Vec<_>>>();
        let Some(v) = values.filter(|v| {
            v.iter().all(|x| x.is_finite())
                && v[..4].iter().all(|x| *x > 0.)
                && v[4] >= 0.
                && v[1] >= v[0].max(v[3])
                && v[2] <= v[0].min(v[3])
        }) else {
            continue;
        };
        data.push(json!({"timestamp":if t>100_000_000_000{t}else{t*1000},"open":v[0],"high":v[1],"low":v[2],"close":v[3],"volume_usd":v[4]}));
    }
    data.sort_by_key(|r| r["timestamp"].as_i64());
    data.dedup_by_key(|r| r["timestamp"].as_i64());
    Ok(data)
}
async fn fetch(
    client: &reqwest::Client,
    base: &str,
    pool: &str,
    token: &str,
    resolution: &str,
    limit: i64,
) -> Result<Value> {
    let (timeframe, aggregate) = interval(resolution).context("unsupported resolution")?;
    let response = client
        .get(format!(
            "{}/pools/{pool}/ohlcv/{timeframe}",
            base.trim_end_matches('/')
        ))
        .header("Accept", "application/json;version=20230203")
        .query(&[
            ("aggregate", aggregate),
            ("limit", &limit.to_string()),
            ("currency", "usd"),
            ("token", token),
        ])
        .timeout(Duration::from_secs(10))
        .send()
        .await?;
    let status = response.status();
    let payload = response
        .json::<Value>()
        .await
        .context("invalid OHLCV response")?;
    if !status.is_success() {
        bail!("OHLCV provider HTTP {status}")
    }
    // Verify the requested address when provider metadata is supplied.
    if let Some(meta) = payload
        .get("meta")
        .filter(|m| m.get("base").is_some() || m.get("quote").is_some())
    {
        if !["base", "quote"].iter().any(|side| {
            meta[*side]["address"]
                .as_str()
                .is_some_and(|a| a.eq_ignore_ascii_case(token))
        }) {
            bail!("OHLCV token metadata mismatch")
        }
    }
    let data = decode(&payload)?;
    if data.is_empty() {
        bail!("OHLCV provider has no candles for this pool/token")
    }
    let as_of = data
        .last()
        .and_then(|r| r["timestamp"].as_i64())
        .map(|t| t / 1000);
    Ok(
        json!({"source":"geckoterminal_onchain_ohlcv","price_unit":"USD","volume_unit":"USD","pool":pool,"token":token,"resolution":resolution,"data":data,"as_of":as_of,"stale":false,"fetched_at":store::now()}),
    )
}
async fn chart(
    client: &reqwest::Client,
    base: &str,
    path: &PathBuf,
    pool: &str,
    token: &str,
    resolution: &str,
    limit: i64,
) -> Result<Value> {
    // Only duplicate requests share a lock. A slow old pool must not block a
    // chart for a newly selected token, including reads already in the cache.
    let key = format!(
        "{}:{base}:{pool}:{token}:{resolution}:{limit}",
        path.display()
    );
    let lock = FETCH
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .entry(key)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;
    let previous = cached(path, pool, token, resolution, limit)?;
    if let Some((body, time)) = &previous {
        if store::now() - time < 60 {
            return Ok(body.clone());
        }
    }
    match fetch(client, base, pool, token, resolution, limit).await {
        Ok(body) => {
            cache_db(path)?.execute("INSERT INTO monad_ohlcv_cache VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(pool,token,resolution,limit_count) DO UPDATE SET body=excluded.body,fetched_at=excluded.fetched_at",rusqlite::params![pool,token,resolution,limit,body.to_string(),store::now()])?;
            Ok(body)
        }
        Err(error) => {
            if let Some((mut body, _)) = previous {
                body["stale"] = json!(true);
                body["cache_reason"] = json!(error.to_string());
                Ok(body)
            } else {
                Err(error)
            }
        }
    }
}
pub async fn candles(
    State(s): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    let pool = q
        .get("pool")
        .map(|v| v.to_ascii_lowercase())
        .ok_or_else(|| bad(StatusCode::BAD_REQUEST, "pool is required"))?;
    let rows=read_db(&s.sqlite_path,&format!("SELECT token_a_address,token_b_address FROM pools WHERE chain='monad' AND lower(pool_address)=lower('{}')",escape(&pool))).map_err(internal)?;
    let p = rows
        .first()
        .ok_or_else(|| bad(StatusCode::NOT_FOUND, "pool not found"))?;
    let token = q
        .get("token")
        .or_else(|| q.get("target_mint"))
        .cloned()
        .or_else(|| p["token_a_address"].as_str().map(str::to_owned))
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !address(&token) || !(eq(p, "token_a_address", &token) || eq(p, "token_b_address", &token)) {
        return Err(bad(StatusCode::BAD_REQUEST, "token must belong to pool"));
    }
    let resolution = q.get("resolution").map(String::as_str).unwrap_or("1h");
    if interval(resolution).is_none() {
        return Err(bad(StatusCode::BAD_REQUEST, "unsupported resolution"));
    }
    let limit = q
        .get("limit")
        .map(|v| v.parse::<i64>())
        .transpose()
        .map_err(|_| bad(StatusCode::BAD_REQUEST, "invalid limit"))?
        .unwrap_or(24)
        .clamp(1, 1000);
    let path = env::var_os("MONAD_MARKET_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| store::path().with_file_name("monad_market.db"));
    let base = env::var("MONAD_OHLCV_BASE_URL")
        .unwrap_or_else(|_| "https://api.geckoterminal.com/api/v2/networks/monad".into());
    chart(&s.rpc_http, &base, &path, &pool, &token, resolution, limit)
        .await
        .map(Json)
        .map_err(|e| bad(StatusCode::BAD_GATEWAY, &e.to_string()))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn external_chart_cache_token_isolation_and_failure() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let signal = entered.clone();
        let unblock = release.clone();
        let app=axum::Router::new().route("/pools/{pool}/ohlcv/{tf}",axum::routing::get(move |Query(q):Query<HashMap<String,String>>|{let count=count.clone();let signal=signal.clone();let unblock=unblock.clone();async move{
            if q["token"]=="slow"{signal.notify_one();unblock.notified().await;}
            count.fetch_add(1,Ordering::SeqCst);
            assert_eq!(q["currency"],"usd");
            let price=if q["token"]=="base"{2.}else{4.};
            Json(json!({"data":{"attributes":{"ohlcv_list":[[200,price,price,price,price,10],[100,price,price,price,price,5]]}}}))
        }}));
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", tcp.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(tcp, app).await.unwrap() });
        let path = env::temp_dir().join(format!(
            "monad-market-{}-{}.db",
            std::process::id(),
            store::now()
        ));
        let client = reqwest::Client::new();
        let a = chart(&client, &base, &path, "pool", "base", "1h", 24)
            .await
            .unwrap();
        assert_eq!(a["price_unit"], "USD");
        assert_eq!(a["data"][0]["timestamp"], 100000);
        assert_eq!(a["as_of"], 200);
        assert_eq!(
            chart(&client, &base, &path, "pool", "base", "1h", 24)
                .await
                .unwrap(),
            a
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let b = chart(&client, &base, &path, "pool", "quote", "1h", 24)
            .await
            .unwrap();
        assert_eq!(b["data"][0]["close"], 4.);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let slow = {
            let client = client.clone();
            let base = base.clone();
            let path = path.clone();
            tokio::spawn(
                async move { chart(&client, &base, &path, "pool", "slow", "1h", 24).await },
            )
        };
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_millis(500),
            chart(&client, &base, &path, "pool", "fast", "1h", 24),
        )
        .await
        .expect("new token chart blocked behind old token")
        .unwrap();
        release.notify_one();
        slow.await.unwrap().unwrap();
        cache_db(&path)
            .unwrap()
            .execute("UPDATE monad_ohlcv_cache SET fetched_at=0", [])
            .unwrap();
        server.abort();
        let stale = chart(&client, &base, &path, "pool", "base", "1h", 24)
            .await
            .unwrap();
        assert_eq!(stale["stale"], true);
        assert_eq!(stale["as_of"], 200);
        assert_eq!(stale["data"], a["data"]);
        assert!(chart(&client, &base, &path, "another", "base", "1h", 24)
            .await
            .is_err());
        std::fs::remove_file(path).unwrap();
    }
}
