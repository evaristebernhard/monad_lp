//! Current indexed 24h volume/TVL snapshots, separate from RPC spot prices.
use crate::{monad_store as store, state::AppState};
use anyhow::{bail, Context, Result};
use rusqlite::params;
use serde_json::{json, Value};

fn number(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str()?.parse().ok())
        .filter(|v| v.is_finite() && *v >= 0.)
}

fn decode(row: &Value, pool: &Value, observed: i64, age: i64) -> Result<Value> {
    let attrs = &row["attributes"];
    let id = pool["pool_address"].as_str().context("pool ID missing")?;
    if !attrs["address"]
        .as_str()
        .is_some_and(|a| a.eq_ignore_ascii_case(id))
    {
        bail!("market pool ID mismatch")
    }
    let token = |side: &str| {
        row["relationships"][side]["data"]["id"]
            .as_str()
            .and_then(|s| s.strip_prefix("monad_"))
            .map(str::to_ascii_lowercase)
    };
    let mut actual = [
        token("base_token").context("base missing")?,
        token("quote_token").context("quote missing")?,
    ];
    actual.sort();
    let mut expected = [
        pool["token_a_address"]
            .as_str()
            .unwrap_or("")
            .to_ascii_lowercase(),
        pool["token_b_address"]
            .as_str()
            .unwrap_or("")
            .to_ascii_lowercase(),
    ];
    expected.sort();
    if actual != expected {
        bail!("market token identity mismatch")
    }
    let volume = number(&attrs["volume_usd"]["h24"]).context("24h volume unavailable")?;
    let tvl = number(&attrs["reserve_in_usd"]).context("TVL unavailable")?;
    // The provider publishes no per-field source timestamp. Explicitly expose
    // collection time/cache age; never call this the latest swap/block time.
    Ok(
        json!({"pool":id,"volume_usd":volume,"tvl_usd":tvl,"as_of":observed-age,"observed_at":observed,"provider_cache_age_seconds":age,"source_as_of":null,"as_of_kind":"provider_snapshot_observation","source":"geckoterminal_pool_snapshot"}),
    )
}

async fn refresh(s: &AppState, pools: &[Value]) -> Result<()> {
    let origin = std::env::var("MONAD_OHLCV_BASE_URL")
        .unwrap_or_else(|_| "https://api.geckoterminal.com/api/v2/networks/monad".into());
    // Public endpoint accepts up to 30 pool identifiers per request.
    for chunk in pools.chunks(20) {
        let ids = chunk
            .iter()
            .filter_map(|p| p["pool_address"].as_str())
            .collect::<Vec<_>>()
            .join(",");
        let response = s
            .http
            .get(format!(
                "{}/pools/multi/{ids}",
                origin.trim_end_matches('/')
            ))
            .header("Accept", "application/json;version=20230203")
            .timeout(std::time::Duration::from_secs(12))
            .send()
            .await?
            .error_for_status()?;
        let age = response
            .headers()
            .get("age")
            .and_then(|s| s.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
            .max(0);
        let payload = response.json::<Value>().await?;
        let observed = store::now();
        let rows = payload["data"]
            .as_array()
            .context("market pool rows missing")?;
        let mut c = store::open()?;
        let tx = c.transaction()?;
        for pool in chunk {
            let id = pool["pool_address"].as_str().unwrap_or("");
            if let Some(row) = rows.iter().find(|r| {
                r["attributes"]["address"]
                    .as_str()
                    .is_some_and(|a| a.eq_ignore_ascii_case(id))
            }) {
                match decode(row, pool, observed, age) {
                    Ok(body) => {
                        tx.execute("INSERT INTO live_market VALUES(?1,?2) ON CONFLICT(pool) DO UPDATE SET body=excluded.body",params![id,body.to_string()])?;
                    }
                    Err(e) => eprintln!("Monad market {id}: {e}"),
                }
            } else {
                eprintln!(
                    "Monad market {id}: provider omitted pool; retaining previous observation"
                )
            }
        }
        tx.commit()?;
    }
    Ok(())
}

pub fn spawn(s: AppState) {
    let history = s.clone();
    tokio::spawn(async move {
        let mut index = 0usize;
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(10));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            if let Ok(pools) = store::fixed_pools() {
                if pools.is_empty() {
                    continue;
                }
                let pool = &pools[index % pools.len()];
                index += 1;
                if let Err(e) = backfill(&history, pool).await {
                    eprintln!("Monad hourly market {}: {e}", pool["pool_address"])
                }
            }
        }
    });
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(30));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            let result = async {
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
                refresh(&s, &pools).await
            }
            .await;
            if let Err(e) = result {
                eprintln!("Monad market refresh: {e}")
            }
        }
    });
}

async fn backfill(s: &AppState, pool: &Value) -> Result<()> {
    let catalog = std::env::var_os("MONAD_POOL_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "data/lp_screener.db".into());
    let path = std::env::var_os("LP_PRICE_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| catalog.parent().unwrap().join("lp_price_history.db"));
    let id = pool["pool_address"].as_str().context("pool missing")?;
    let end = store::now() / 3600 * 3600;
    let c = rusqlite::Connection::open(&path)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    let count=c.query_row("SELECT count(*) FROM pool_price_history WHERE chain='monad' AND pool_address=?1 AND timestamp>=?2 AND timestamp<?3 AND price_status='priced' AND token1_per_token0>0",params![id,end-86400,end],|r|r.get::<_,i64>(0))?;
    if count == 24 {
        return Ok(());
    }
    drop(c);
    let origin = std::env::var("MONAD_OHLCV_BASE_URL")
        .unwrap_or_else(|_| "https://api.geckoterminal.com/api/v2/networks/monad".into());
    let payload = s
        .http
        .get(format!(
            "{}/pools/{id}/ohlcv/hour",
            origin.trim_end_matches('/')
        ))
        .query(&[
            ("aggregate", "1"),
            ("limit", "48"),
            ("currency", "token"),
            ("token", "base"),
            ("before_timestamp", &end.to_string()),
            ("include_empty_intervals", "false"),
        ])
        .timeout(std::time::Duration::from_secs(12))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let a = pool["token_a_address"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    let b = pool["token_b_address"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    let base = payload["meta"]["base"]["address"]
        .as_str()
        .context("OHLCV base missing")?
        .to_ascii_lowercase();
    let quote = payload["meta"]["quote"]["address"]
        .as_str()
        .context("OHLCV quote missing")?
        .to_ascii_lowercase();
    if !((base == a && quote == b) || (base == b && quote == a)) {
        bail!("OHLCV token identity mismatch")
    }
    let (t0, t1) = if a < b { (a, b) } else { (b, a) };
    let mut c = rusqlite::Connection::open(&path)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    let tx = c.transaction()?;
    for row in payload["data"]["attributes"]["ohlcv_list"]
        .as_array()
        .context("OHLCV rows missing")?
    {
        let Some(t) = row[0]
            .as_i64()
            .filter(|t| *t < end && *t >= end - 86400 && *t % 3600 == 0)
        else {
            continue;
        };
        let Some(close) = number(&row[4]).filter(|v| *v > 0.) else {
            continue;
        };
        let ratio = if base == t0 { close } else { 1. / close };
        if !ratio.is_finite() || ratio <= 0. {
            continue;
        }
        tx.execute("INSERT INTO pool_price_history(chain,pool_address,timestamp,token0_address,token1_address,token1_per_token0,source,price_status,observed_at) VALUES('monad',?1,?2,?3,?4,?5,'geckoterminal_pool_ratio_1h','priced',?6) ON CONFLICT(chain,pool_address,timestamp) DO UPDATE SET token0_address=excluded.token0_address,token1_address=excluded.token1_address,token1_per_token0=excluded.token1_per_token0,source=excluded.source,price_status='priced',observed_at=excluded.observed_at WHERE pool_price_history.price_status!='priced' OR pool_price_history.source='geckoterminal_pool_ratio_1h'",params![id,t,&t0,&t1,ratio,t+3600])?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_identities_and_retains_observation_semantics() {
        let pool = json!({"pool_address":"pool","token_a_address":"a","token_b_address":"b"});
        let mut row = json!({"attributes":{"address":"pool","volume_usd":{"h24":"123"},"reserve_in_usd":"50"},"relationships":{"base_token":{"data":{"id":"monad_b"}},"quote_token":{"data":{"id":"monad_a"}}}});
        let value = decode(&row, &pool, 1000, 20).unwrap();
        assert_eq!(value["as_of"], 980);
        assert_eq!(value["volume_usd"], 123.);
        assert!(value["source_as_of"].is_null());
        row["relationships"]["base_token"]["data"]["id"] = json!("monad_fake");
        assert!(decode(&row, &pool, 1000, 0).is_err());
    }
}
