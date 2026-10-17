mod monad;
mod selection;
mod wallet;
mod swap;
use std::{env, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;

use lp_metrics::{attach_runtime_volatility, pool_volatility_report, token_volatility_report};

#[derive(Clone)]
struct AppState {
    sqlite_path: PathBuf,
    market_db_path: PathBuf,
    rpc_refresh_path: PathBuf,
    watchlist_user_id: String,
    rpc_price_url: String,
    rpc_http: reqwest::Client,
    vybe_api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PoolQuery {
    chain: Option<String>,
    q: Option<String>,
    token: Option<String>,
    protocol: Option<String>,
    protocols: Option<String>,
    pair: Option<String>,
    addresses: Option<String>,
    pair_category: Option<String>,
    pool_type: Option<String>,
    min_liquidity_usd: Option<f64>,
    min_volume_24h_usd: Option<f64>,
    min_fee_bps: Option<f64>,
    min_lp_fee_apr: Option<f64>,
    fee_apr_status: Option<String>,
    rpc_status: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    page: Option<u32>,
    page_size: Option<u32>,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

#[derive(Debug, Deserialize)]
struct WatchlistRequest {
    pool_address: String,
}

#[derive(Debug, Deserialize)]
struct SolanaPoolQuery {
    token: Option<String>,
    per_protocol: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct SolanaResolveQuery {
    address: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SolanaStartRequest {
    token: String,
}

const SOLANA_QUOTE_MINTS: [&str; 3] = [
    "So11111111111111111111111111111111111111112",
    "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",
];

// Catalog fee fields are not unit-stable across indexer snapshots. V4 trusts
// only the live StateView decoder; a missing V4 RPC fee remains NULL. Older
// V2/V3 catalog rows can contain the protocol fee tier in hundredths of a bip
// (3000 means 30 bps), so normalize that representation only when it is used
// as a fallback. Live RPC values are already ordinary basis points.
const CATALOG_FEE_BPS: &str = "CASE WHEN p.fee_bps IS NULL THEN NULL WHEN p.pool_type IN ('constant_product_v2', 'concentrated_v3') AND p.fee_bps > 1000 THEN p.fee_bps / 100.0 WHEN p.pool_type = 'uniswap_v4' THEN NULL ELSE p.fee_bps END";
const EFFECTIVE_FEE_BPS: &str =
    "CASE WHEN p.pool_type = 'uniswap_v4' THEN s.fee_bps ELSE coalesce(s.fee_bps, {CATALOG_FEE_BPS}) END";
const EFFECTIVE_APR: &str = "CASE WHEN p.lp_fee_apr_estimate > 0 AND p.lp_fee_apr_estimate <= 100 THEN p.lp_fee_apr_estimate WHEN coalesce(p.volume_24h_usd, 0) > 0 AND coalesce(p.liquidity_usd, 0) >= 100 AND coalesce(CASE WHEN p.pool_type = 'uniswap_v4' THEN s.fee_bps ELSE coalesce(s.fee_bps, {CATALOG_FEE_BPS}) END, 0) > 0 AND p.volume_24h_usd * (CASE WHEN p.pool_type = 'uniswap_v4' THEN s.fee_bps ELSE coalesce(s.fee_bps, {CATALOG_FEE_BPS}) END / 10000.0) * 365.0 / p.liquidity_usd * coalesce(p.lp_fee_share_assumption, 1.0) <= 100 THEN p.volume_24h_usd * (CASE WHEN p.pool_type = 'uniswap_v4' THEN s.fee_bps ELSE coalesce(s.fee_bps, {CATALOG_FEE_BPS}) END / 10000.0) * 365.0 / p.liquidity_usd * coalesce(p.lp_fee_share_assumption, 1.0) ELSE NULL END";
const FEE_APR: &str = "CASE WHEN coalesce(p.volume_24h_usd, 0) > 0 AND coalesce(p.liquidity_usd, 0) >= 100 AND coalesce(CASE WHEN p.pool_type = 'uniswap_v4' THEN s.fee_bps ELSE coalesce(s.fee_bps, {CATALOG_FEE_BPS}) END, 0) > 0 THEN p.volume_24h_usd * (CASE WHEN p.pool_type = 'uniswap_v4' THEN s.fee_bps ELSE coalesce(s.fee_bps, {CATALOG_FEE_BPS}) END / 10000.0) * 365.0 / p.liquidity_usd * coalesce(p.lp_fee_share_assumption, 1.0) ELSE NULL END";
const SEVEN_DAY_FEE_APR: &str = "CASE WHEN coalesce(p.volume_7d_usd, 0) > 0 AND coalesce(p.liquidity_usd, 0) >= 100 AND coalesce(CASE WHEN p.pool_type = 'uniswap_v4' THEN s.fee_bps ELSE coalesce(s.fee_bps, {CATALOG_FEE_BPS}) END, 0) > 0 THEN p.volume_7d_usd * (CASE WHEN p.pool_type = 'uniswap_v4' THEN s.fee_bps ELSE coalesce(s.fee_bps, {CATALOG_FEE_BPS}) END / 10000.0) * 365.0 / 7.0 / p.liquidity_usd * coalesce(p.lp_fee_share_assumption, 1.0) ELSE NULL END";
const SORTS: &[&str] = &[
    "liquidity_usd",
    "volume_24h_usd",
    "volume_7d_usd",
    "lp_fee_apr_estimate",
    "effective_lp_fee_apr",
    "fee_apr",
    "7d_fee_apr",
    "effective_fee_bps",
    "volume_24h_to_tvl",
    "price_usd",
    "fee_bps",
];

// Keep pools with no executable price out of the user-facing candidate set.
// The history row is intentionally retained, so a pool becomes visible again
// automatically when a later refresh records a priced observation.
const LATEST_POOL_PRICE_JOIN: &str = "LEFT JOIN (SELECT h.pool_address, h.chain, h.price_status, h.status_reason FROM pool_price_history h INNER JOIN (SELECT chain, pool_address, max(timestamp) AS timestamp FROM pool_price_history GROUP BY chain, pool_address) latest ON latest.chain = h.chain AND latest.pool_address = h.pool_address AND latest.timestamp = h.timestamp) ph ON ph.chain = p.chain AND ph.pool_address = p.pool_address";
const PRICED_POOL_FILTER: &str = "coalesce(ph.price_status, 'priced') = 'priced'";

fn expand_fee_sql(template: &str) -> String {
    template.replace("{CATALOG_FEE_BPS}", CATALOG_FEE_BPS)
}

fn pool_columns() -> String {
    let effective_fee_bps = expand_fee_sql(EFFECTIVE_FEE_BPS);
    let fee_apr = expand_fee_sql(FEE_APR);
    let seven_day_fee_apr = expand_fee_sql(SEVEN_DAY_FEE_APR);
    let effective_apr = expand_fee_sql(EFFECTIVE_APR);
    format!(
        "p.*, ph.price_status AS pool_price_status, ph.status_reason AS pool_price_status_reason, h.option_status, h.option_premium_usd, h.replication_error_usd, h.net_pnl_lower_usd, h.net_pnl_upper_usd, h.decision, h.p0, h.range_low, h.range_high, s.state_status AS rpc_state_status, s.pool_family AS rpc_pool_family, s.decoder_version AS rpc_decoder_version, s.state_block AS rpc_state_block, s.checked_at AS rpc_checked_at, s.state_json AS rpc_state_json, s.assets_json AS rpc_assets_json, s.quote_supported AS rpc_quote_supported, ({}) AS effective_fee_bps, ({}) AS fee_apr, ({}) AS \"7d_fee_apr\", ({}) AS effective_lp_fee_apr, coalesce(v.pair_vol_30d, v.pair_vol_7d, v.pair_vol_90d) AS iv_annualized, v.pair_vol_7d AS iv_7d_annualized, v.pair_vol_30d AS iv_30d_annualized, v.pair_vol_90d AS iv_90d_annualized, v.sample_count_30d AS iv_sample_count_30d, v.pair_vol_1d AS historical_pair_vol_1d, v.pair_vol_3d AS historical_pair_vol_3d, v.pair_vol_7d AS historical_pair_vol_7d, v.sample_count_1d AS historical_pair_samples_1d, v.sample_count_3d AS historical_pair_samples_3d, v.sample_count_7d AS historical_pair_samples_7d, v.status AS iv_status, 'historical_pair_volatility' AS iv_source",
        effective_fee_bps, fee_apr, seven_day_fee_apr, effective_apr
    )
}

fn env_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn default_sqlite_path() -> PathBuf {
    let local = PathBuf::from("data/lp_screener.db");
    if local.exists() {
        return local;
    }
    let parent = PathBuf::from("../data/lp_screener.db");
    if parent.exists() {
        return parent;
    }
    local
}

fn default_project_path(relative: &str) -> PathBuf {
    let local = PathBuf::from(relative);
    if local.exists() {
        return local;
    }
    PathBuf::from("..").join(relative)
}

fn normalize_market_candles(raw: &Value) -> Vec<Value> {
    let Some(rows) = raw.as_array() else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            if let Some(values) = row.as_array() {
                if values.len() < 6 { return None; }
                return Some(json!({"timestamp":values[0],"open":market_number(&values[1])?,"high":market_number(&values[2])?,"low":market_number(&values[3])?,"close":market_number(&values[4])?,"volume_usd":market_number(&values[5]).unwrap_or_default()}));
            }
            let timestamp = row.get("timestamp").or_else(|| row.get("timestamp_ms")).or_else(|| row.get("time")).or_else(|| row.get("t"))?;
            let open = market_number(row.get("open").or_else(|| row.get("o"))?)?;
            let high = market_number(row.get("high").or_else(|| row.get("h"))?)?;
            let low = market_number(row.get("low").or_else(|| row.get("l"))?)?;
            let close = market_number(row.get("close").or_else(|| row.get("c"))?)?;
            let volume = row.get("volume_usd").or_else(|| row.get("volumeUsd")).or_else(|| row.get("volume")).or_else(|| row.get("v")).and_then(market_number).unwrap_or_default();
            Some(json!({"timestamp":timestamp,"open":open,"high":high,"low":low,"close":close,"volume_usd":volume}))
        })
        .collect()
}

fn market_number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())
}

fn market_timestamp(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|value| value as i64))
        .or_else(|| value.as_str()?.parse::<i64>().ok())
}

fn initialize_market_db(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS solana_queried_pools (
          pool_address TEXT PRIMARY KEY, first_queried_at_ms INTEGER NOT NULL,
          last_queried_at_ms INTEGER NOT NULL, query_count INTEGER NOT NULL DEFAULT 1,
          last_resolution TEXT, last_source TEXT
        );
        CREATE TABLE IF NOT EXISTS solana_ohlcv_candles (
          pool_address TEXT NOT NULL, resolution TEXT NOT NULL, timestamp_ms INTEGER NOT NULL,
          open REAL NOT NULL, high REAL NOT NULL, low REAL NOT NULL, close REAL NOT NULL,
          volume_usd REAL NOT NULL DEFAULT 0, source TEXT NOT NULL, fetched_at_ms INTEGER NOT NULL,
          PRIMARY KEY(pool_address, resolution, timestamp_ms, source)
        );",
    )?;
    let legacy_exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='solana_market_candles'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if legacy_exists {
        connection.execute("INSERT OR IGNORE INTO solana_ohlcv_candles(pool_address,resolution,timestamp_ms,open,high,low,close,volume_usd,source,fetched_at_ms) SELECT pool_address,resolution,timestamp_ms,open,high,low,close,volume_usd,source,fetched_at_ms FROM solana_market_candles", [])?;
        connection.execute_batch("DROP TABLE solana_market_candles;")?;
    }
    Ok(())
}

fn market_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn touch_queried_pool(path: &PathBuf, pool: &str, resolution: &str) -> Result<()> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute("INSERT INTO solana_queried_pools(pool_address,first_queried_at_ms,last_queried_at_ms,query_count,last_resolution) VALUES(?1,?2,?2,1,?3) ON CONFLICT(pool_address) DO UPDATE SET last_queried_at_ms=excluded.last_queried_at_ms,query_count=query_count+1,last_resolution=excluded.last_resolution", rusqlite::params![pool,market_now_ms(),resolution])?;
    Ok(())
}

fn save_market_candles(
    path: &PathBuf,
    pool: &str,
    resolution: &str,
    candles: &[Value],
    source: &str,
) -> Result<()> {
    let mut connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    let tx = connection.transaction()?;
    let fetched = market_now_ms();
    for candle in candles {
        let Some(timestamp) = candle.get("timestamp").and_then(market_timestamp) else {
            continue;
        };
        let timestamp_ms = if timestamp.abs() < 100_000_000_000 {
            timestamp * 1000
        } else {
            timestamp
        };
        let value = |key| candle.get(key).and_then(market_number).unwrap_or_default();
        tx.execute("INSERT OR REPLACE INTO solana_ohlcv_candles(pool_address,resolution,timestamp_ms,open,high,low,close,volume_usd,source,fetched_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", rusqlite::params![pool,resolution,timestamp_ms,value("open"),value("high"),value("low"),value("close"),value("volume_usd"),source,fetched])?;
    }
    tx.execute(
        "UPDATE solana_queried_pools SET last_source=?2 WHERE pool_address=?1",
        rusqlite::params![pool, source],
    )?;
    tx.commit()?;
    Ok(())
}

fn load_cached_candles(
    path: &PathBuf,
    pool: &str,
    resolution: &str,
    limit: u32,
) -> Result<Option<Vec<Value>>> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    let mut statement = connection.prepare("SELECT timestamp_ms,open,high,low,close,volume_usd FROM (SELECT timestamp_ms,open,high,low,close,volume_usd,row_number() OVER (PARTITION BY timestamp_ms ORDER BY CASE source WHEN 'geckoterminal' THEN 0 WHEN 'vybe' THEN 1 ELSE 2 END,fetched_at_ms DESC) AS source_rank FROM solana_ohlcv_candles WHERE pool_address=?1 AND resolution=?2 AND source IN ('geckoterminal','vybe')) WHERE source_rank=1 ORDER BY timestamp_ms DESC LIMIT ?3")?;
    let rows = statement.query_map(rusqlite::params![pool,resolution,limit as i64], |row| Ok(json!({"timestamp":row.get::<_,i64>(0)?/1000,"open":row.get::<_,f64>(1)? ,"high":row.get::<_,f64>(2)? ,"low":row.get::<_,f64>(3)? ,"close":row.get::<_,f64>(4)? ,"volume_usd":row.get::<_,f64>(5)?})))?;
    let mut values = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    if values.is_empty() {
        let mut local = connection.prepare("SELECT timestamp_ms,open,high,low,close,volume_usd FROM solana_ohlcv_candles WHERE pool_address=?1 AND resolution=?2 AND source='listener_event_aggregation' ORDER BY timestamp_ms DESC LIMIT ?3")?;
        values = local.query_map(rusqlite::params![pool,resolution,limit as i64], |row| Ok(json!({"timestamp":row.get::<_,i64>(0)?/1000,"open":row.get::<_,f64>(1)? ,"high":row.get::<_,f64>(2)? ,"low":row.get::<_,f64>(3)? ,"close":row.get::<_,f64>(4)? ,"volume_usd":row.get::<_,f64>(5)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    }
    values.reverse();
    Ok((!values.is_empty()).then_some(values))
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "''")
}

fn exact_filter(column: &str, value: &str) -> String {
    format!("{} = '{}'", column, escape(value.trim()))
}

fn where_clause(query: &PoolQuery, path_chain: Option<&str>) -> String {
    let mut parts = vec![PRICED_POOL_FILTER.to_string()];
    let effective_fee_bps = expand_fee_sql(EFFECTIVE_FEE_BPS);
    let effective_apr = expand_fee_sql(EFFECTIVE_APR);
    let chain = path_chain.or(query.chain.as_deref());
    if let Some(value) = chain.filter(|x| *x != "all" && !x.trim().is_empty()) {
        parts.push(exact_filter("p.chain", value));
    }
    if let Some(value) = query
        .protocol
        .as_deref()
        .filter(|x| *x != "all" && !x.trim().is_empty())
    {
        parts.push(exact_filter("p.protocol", value));
    }
    if let Some(values) = query
        .protocols
        .as_deref()
        .map(str::trim)
        .filter(|x| !x.is_empty())
    {
        let values = values
            .split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty() && *x != "all")
            .map(|x| format!("'{}'", escape(x)))
            .collect::<Vec<_>>();
        if !values.is_empty() {
            parts.push(format!("p.protocol IN ({})", values.join(", ")));
        }
    }
    if let Some(value) = query
        .pair
        .as_deref()
        .filter(|x| *x != "all" && !x.trim().is_empty())
    {
        parts.push(exact_filter("p.pair", value));
    }
    if let Some(value) = query
        .pair_category
        .as_deref()
        .filter(|x| *x != "all" && !x.trim().is_empty())
    {
        parts.push(exact_filter("p.pair_category", value));
    }
    if let Some(values) = query
        .addresses
        .as_deref()
        .map(str::trim)
        .filter(|x| !x.is_empty())
    {
        let values = values
            .split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(|x| format!("'{}'", escape(x)))
            .collect::<Vec<_>>();
        if !values.is_empty() {
            parts.push(format!(
                "lower(p.pool_address) IN ({})",
                values
                    .iter()
                    .map(|value| value.to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    if let Some(value) = query
        .pool_type
        .as_deref()
        .filter(|x| *x != "all" && !x.trim().is_empty())
    {
        parts.push(exact_filter("p.pool_type", value));
    }
    if let Some(value) = query.min_liquidity_usd {
        parts.push(format!("coalesce(p.liquidity_usd, 0) >= {}", value));
    }
    if let Some(value) = query.min_volume_24h_usd {
        parts.push(format!("coalesce(p.volume_24h_usd, 0) >= {}", value));
    }
    if let Some(value) = query.min_fee_bps {
        parts.push(format!("{} >= {}", effective_fee_bps, value));
    }
    if let Some(value) = query.min_lp_fee_apr {
        parts.push(format!("{} >= {}", effective_apr, value));
    }
    if let Some(value) = query
        .fee_apr_status
        .as_deref()
        .filter(|x| *x != "all" && !x.trim().is_empty())
    {
        match value {
            "available" => parts.push(format!("{} IS NOT NULL", effective_apr)),
            "missing" => parts.push(format!("{} IS NULL", effective_apr)),
            _ => {}
        }
    }
    if let Some(value) = query
        .rpc_status
        .as_deref()
        .filter(|x| *x != "all" && !x.trim().is_empty())
    {
        match value {
            "live" => parts.push("s.state_status IN ('live', 'account_present')".into()),
            "unavailable" => parts.push(
                "s.state_status IN ('rpc_error', 'transport_error', 'account_missing')".into(),
            ),
            "unsupported" => parts.push("s.state_status = 'unsupported_protocol'".into()),
            "missing" => parts.push("s.pool_address IS NULL".into()),
            _ => {}
        }
    }
    if let Some(value) = query.q.as_deref().map(str::trim).filter(|x| !x.is_empty()) {
        let like = format!("%{}%", escape(value));
        parts.push(format!("(p.pair LIKE '{}' OR p.pool_address LIKE '{}' OR p.protocol LIKE '{}' OR p.asset_symbol LIKE '{}' OR p.stable_symbol LIKE '{}')", like, like, like, like, like));
    }
    if let Some(value) = query
        .token
        .as_deref()
        .map(str::trim)
        .filter(|x| !x.is_empty())
    {
        let token = escape(value);
        parts.push(format!(
            "(lower(p.token_a_address) = lower('{}') OR lower(p.token_b_address) = lower('{}'))",
            token, token
        ));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", parts.join(" AND "))
    }
}

fn db_query(sqlite_path: &PathBuf, sql: &str) -> Result<Vec<Value>> {
    let connection = Connection::open(sqlite_path).context("open SQLite")?;
    let price_path = env::var_os("LP_PRICE_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            sqlite_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("lp_price_history.db")
        });
    if price_path != sqlite_path.clone() {
        connection.execute(
            "ATTACH DATABASE ?1 AS price",
            [price_path.to_string_lossy().as_ref()],
        )?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS price.token_price_history (
               chain TEXT NOT NULL, address TEXT NOT NULL, timestamp INTEGER NOT NULL,
               price_usd REAL NOT NULL, source TEXT NOT NULL, observed_at INTEGER NOT NULL,
               PRIMARY KEY (chain, address, timestamp)
             );
             CREATE TABLE IF NOT EXISTS price.pool_price_history (
               chain TEXT NOT NULL, pool_address TEXT NOT NULL, timestamp INTEGER NOT NULL,
               token0_address TEXT NOT NULL, token1_address TEXT NOT NULL,
               token1_per_token0 REAL, token0_price_usd REAL, token1_price_usd REAL,
               source TEXT NOT NULL, price_status TEXT NOT NULL DEFAULT 'priced',
               status_reason TEXT, observed_at INTEGER NOT NULL,
               PRIMARY KEY (chain, pool_address, timestamp)
             );",
        )?;
    }
    let mut statement = connection.prepare(sql)?;
    let names = statement
        .column_names()
        .iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>();
    let mut rows = statement.query([])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let mut object = serde_json::Map::new();
        for (index, name) in names.iter().enumerate() {
            let value = match row.get_ref(index)? {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(value) => json!(value),
                rusqlite::types::ValueRef::Real(value) => json!(value),
                rusqlite::types::ValueRef::Text(value) => json!(String::from_utf8_lossy(value)),
                rusqlite::types::ValueRef::Blob(_) => Value::Null,
            };
            object.insert(name.clone(), value);
        }
        result.push(Value::Object(object));
    }
    Ok(result)
}

fn sqlite_has_column(sqlite_path: &PathBuf, table: &str, column: &str) -> Result<bool> {
    Ok(
        db_query(sqlite_path, &format!("PRAGMA table_info({table})"))?
            .iter()
            .any(|row| row.get("name").and_then(Value::as_str) == Some(column)),
    )
}

fn chrono_like_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn ensure_sqlite_watchlist(path: &PathBuf) -> Result<()> {
    let connection = Connection::open(path).context("open SQLite for watchlist")?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS watchlist (
            user_id TEXT NOT NULL,
            pool_address TEXT NOT NULL,
            is_deleted INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (user_id, pool_address)
        );",
    )?;
    Ok(())
}

fn normal_watchlist_address(address: &str) -> Result<String> {
    let address = address.trim();
    if address.is_empty() || address.len() > 256 {
        anyhow::bail!("pool_address must be between 1 and 256 characters")
    }
    Ok(address.to_string())
}

async fn query(state: &AppState, sql: &str) -> Result<(Vec<Value>, &'static str)> {
    Ok((db_query(&state.sqlite_path, sql)?, "sqlite"))
}

async fn solana_resolve(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SolanaResolveQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let address = query
        .address
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: "address is required".to_string(),
                }),
            )
        })?;
    let escaped = escape(address);
    let pool_sql = format!(
        "SELECT pool_address, protocol, pair, token_a_symbol, token_b_symbol,
                token_a_address, token_b_address, liquidity_usd, volume_24h_usd
         FROM pools
         WHERE lower(chain) = 'solana'
           AND lower(pool_address) = lower('{escaped}')
         ORDER BY COALESCE(liquidity_usd, 0) DESC",
    );
    let mut pools = db_query(&state.sqlite_path, &pool_sql).map_err(internal)?;
    if !pools.is_empty() {
        let resolved_token =
            preferred_solana_token(&pools[0]).unwrap_or_else(|| address.to_string());
        return Ok(Json(json!({
            "success": true,
            "input_address": address,
            "address_type": "pool",
            "resolved_token": resolved_token,
            "matched_pools": pools,
            "hint": "pool address resolved to the non-quote token where possible",
            "source": "sqlite",
        })));
    }

    let token_sql = format!(
        "SELECT pool_address, protocol, pair, token_a_symbol, token_b_symbol,
                token_a_address, token_b_address, liquidity_usd, volume_24h_usd
         FROM pools
         WHERE lower(chain) = 'solana'
           AND (lower(token_a_address) = lower('{escaped}')
                OR lower(token_b_address) = lower('{escaped}'))
         ORDER BY COALESCE(liquidity_usd, 0) DESC
         LIMIT 20",
    );
    pools = db_query(&state.sqlite_path, &token_sql).map_err(internal)?;
    if !pools.is_empty() {
        return Ok(Json(json!({
            "success": true,
            "input_address": address,
            "address_type": "mint",
            "resolved_token": address,
            "matched_pools": pools,
            "hint": "mint address matched directly in the Solana pool catalog",
            "source": "sqlite",
        })));
    }

    let symbol_sql = format!(
        "SELECT pool_address, protocol, pair, token_a_symbol, token_b_symbol,
                token_a_address, token_b_address, liquidity_usd, volume_24h_usd
         FROM pools
         WHERE lower(chain) = 'solana'
           AND (lower(token_a_symbol) = lower('{escaped}')
                OR lower(token_b_symbol) = lower('{escaped}'))
         ORDER BY COALESCE(liquidity_usd, 0) DESC
         LIMIT 20",
    );
    pools = db_query(&state.sqlite_path, &symbol_sql).map_err(internal)?;
    if !pools.is_empty() {
        let resolved_token = pools
            .first()
            .and_then(|pool| solana_token_for_symbol(pool, address))
            .or_else(|| preferred_solana_token(&pools[0]))
            .unwrap_or_else(|| address.to_string());
        return Ok(Json(json!({
            "success": true,
            "input_address": address,
            "address_type": "symbol",
            "resolved_token": resolved_token,
            "matched_pools": pools,
            "hint": "token symbol matched in the Solana pool catalog",
            "source": "sqlite",
        })));
    }

    let hint = if address.eq_ignore_ascii_case("solana") {
        "SOLANA is the chain name, not a token mint or symbol; enter SOL/WSOL, a token mint, or a pool address"
    } else {
        "no catalog match; treating the address as a token mint candidate"
    };
    Ok(Json(json!({
        "success": true,
        "input_address": address,
        "address_type": "unknown",
        "resolved_token": address,
        "matched_pools": [],
        "hint": hint,
        "source": "sqlite",
    })))
}

fn solana_token_for_symbol(pool: &Value, symbol: &str) -> Option<String> {
    if pool
        .get("token_a_symbol")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case(symbol))
    {
        return pool
            .get("token_a_address")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
    }
    if pool
        .get("token_b_symbol")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case(symbol))
    {
        return pool
            .get("token_b_address")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
    }
    None
}

fn preferred_solana_token(pool: &Value) -> Option<String> {
    let token_a = pool.get("token_a_address").and_then(Value::as_str)?;
    let token_b = pool.get("token_b_address").and_then(Value::as_str)?;
    let a_quote = SOLANA_QUOTE_MINTS
        .iter()
        .any(|mint| mint.eq_ignore_ascii_case(token_a));
    let b_quote = SOLANA_QUOTE_MINTS
        .iter()
        .any(|mint| mint.eq_ignore_ascii_case(token_b));
    if !a_quote {
        Some(token_a.to_string())
    } else if !b_quote {
        Some(token_b.to_string())
    } else {
        Some(token_a.to_string())
    }
}

async fn solana_pools(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SolanaPoolQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let token = query
        .token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: "token is required".to_string(),
                }),
            )
        })?;
    let per_protocol = query.per_protocol.unwrap_or(2).min(100);
    let token_value = token.to_string();
    let token = escape(token);
    let address_sql = format!(
        "WITH ranked AS (
           SELECT pool_address, protocol, pair, token_a_symbol, token_b_symbol,
                  token_a_address, token_b_address, liquidity_usd,
                  volume_24h_usd, price_usd, pool_type, updated_at,
                  ROW_NUMBER() OVER (
                    PARTITION BY protocol
                    ORDER BY COALESCE(liquidity_usd, 0) DESC,
                             COALESCE(volume_24h_usd, 0) DESC,
                             pool_address
                  ) AS protocol_rank
           FROM pools
           WHERE lower(chain) = 'solana'
             AND (lower(token_a_address) = lower('{token}')
                  OR lower(token_b_address) = lower('{token}'))
         )
         SELECT * FROM ranked
         WHERE {per_protocol} = 0 OR protocol_rank <= {per_protocol}
         ORDER BY protocol, protocol_rank",
    );
    let mut pools = db_query(&state.sqlite_path, &address_sql).map_err(internal)?;
    if pools.is_empty() {
        let symbol = escape(token_value.as_str());
        let symbol_sql = format!(
            "WITH ranked AS (
               SELECT pool_address, protocol, pair, token_a_symbol, token_b_symbol,
                      token_a_address, token_b_address, liquidity_usd,
                      volume_24h_usd, price_usd, pool_type, updated_at,
                      ROW_NUMBER() OVER (
                        PARTITION BY protocol
                        ORDER BY COALESCE(liquidity_usd, 0) DESC,
                                 COALESCE(volume_24h_usd, 0) DESC,
                                 pool_address
                      ) AS protocol_rank
               FROM pools
               WHERE lower(chain) = 'solana'
                 AND (lower(token_a_symbol) = lower('{symbol}')
                      OR lower(token_b_symbol) = lower('{symbol}'))
             )
             SELECT * FROM ranked
             WHERE {per_protocol} = 0 OR protocol_rank <= {per_protocol}
             ORDER BY protocol, protocol_rank",
        );
        pools = db_query(&state.sqlite_path, &symbol_sql).map_err(internal)?;
    }
    let protocols = pools
        .iter()
        .filter_map(|pool| pool.get("protocol").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    Ok(Json(json!({
        "success": true,
        "token": token_value,
        "per_protocol": per_protocol,
        "protocol_count": protocols.len(),
        "pool_count": pools.len(),
        "pools": pools,
        "source": "sqlite",
    })))
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct SolanaEventQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    whale_only: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct SolanaSnapshotQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct SolanaPoolHistoryQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_mint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hours: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct SolanaPoolCandlesQuery {
    pool: Option<String>,
    resolution: Option<String>,
    limit: Option<u32>,
}

async fn proxy_solana_events(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SolanaEventQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    proxy_solana_json(&state, "events", &query).await
}

async fn proxy_solana_alerts(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SolanaEventQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    proxy_solana_json(&state, "whale-alerts", &query).await
}

async fn proxy_solana_snapshots(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SolanaSnapshotQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    proxy_solana_json(&state, "pool-snapshots", &query).await
}

async fn proxy_solana_pool_history(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SolanaPoolHistoryQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let mut response = proxy_solana_json(&state, "pool-history", &query).await?.0;
    if let Some(pool) = query.pool.as_deref().filter(|pool| !pool.trim().is_empty()) {
        let resolution = query.resolution.as_deref().unwrap_or("15m");
        touch_queried_pool(&state.market_db_path, pool, resolution).map_err(internal)?;
        if let Some(candles) = response.get("candles") {
            let normalized = normalize_market_candles(candles);
            if !normalized.is_empty() {
                save_market_candles(
                    &state.market_db_path,
                    pool,
                    resolution,
                    &normalized,
                    "listener_event_aggregation",
                )
                .map_err(internal)?;
            } else if let Some(cached) = load_cached_candles(
                &state.market_db_path,
                pool,
                resolution,
                query.limit.unwrap_or(1_000).clamp(1, 1_000) as u32,
            )
            .map_err(internal)?
            {
                response["candles"] = json!(cached);
                response["ohlcv_source"] = json!("solana_market_cache");
            }
        }
    }
    Ok(Json(response))
}

async fn proxy_solana_pool_candles(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SolanaPoolCandlesQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let pool = query
        .pool
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: "pool is required for market candles".to_string(),
                }),
            )
        })?;
    let resolution = query.resolution.as_deref().unwrap_or("1h");
    let limit = query.limit.unwrap_or(24).clamp(1, 1_000);
    touch_queried_pool(&state.market_db_path, pool, resolution).map_err(internal)?;
    if let Some(api_key) = state.vybe_api_key.as_deref() {
        let url = format!(
            "https://api.vybenetwork.xyz/v4/markets/{}/candles",
            pool.trim()
        );
        if let Ok(response) = state
            .rpc_http
            .get(url)
            .header("X-API-Key", api_key)
            .header("Accept", "application/json")
            .query(&[
                ("resolution", resolution),
                ("limit", &limit.to_string()),
                ("eliminateCloseToOpenGaps", "true"),
            ])
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            if response.status().is_success() {
                if let Ok(payload) = response.json::<Value>().await {
                    let raw = payload.get("data").unwrap_or(&payload);
                    let normalized = normalize_market_candles(raw);
                    if !normalized.is_empty() {
                        save_market_candles(
                            &state.market_db_path,
                            pool,
                            resolution,
                            &normalized,
                            "vybe",
                        )
                        .map_err(internal)?;
                    }
                    return Ok(Json(json!({
                        "source": "vybe_market_candles",
                        "pool": pool,
                        "resolution": resolution,
                        "data": if normalized.is_empty() { raw.clone() } else { json!(normalized) },
                    })));
                }
            }
        }
    }

    // GeckoTerminal's public OHLCV API is keyed by the DEX pool address and
    // returns USD candles without requiring a provider key. Keep this behind
    // the API so the browser never depends on third-party CORS behavior.
    let (timeframe, aggregate) = match resolution {
        "1m" => ("minute", "1"),
        "3m" => ("minute", "3"),
        "5m" => ("minute", "5"),
        "15m" => ("minute", "15"),
        "30m" => ("minute", "30"),
        "1h" => ("hour", "1"),
        _ => ("minute", "1"),
    };
    let url = format!(
        "https://api.geckoterminal.com/api/v2/networks/solana/pools/{}/ohlcv/{}",
        pool.trim(),
        timeframe
    );
    let response = state
        .rpc_http
        .get(url)
        .header("Accept", "application/json;version=20230203")
        .query(&[
            ("aggregate", aggregate),
            ("limit", &limit.to_string()),
            ("currency", "usd"),
        ])
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            if let Some(data) = load_cached_candles(&state.market_db_path, pool, resolution, limit)
                .map_err(internal)?
            {
                return Ok(Json(
                    json!({"source":"solana_market_cache","pool":pool,"resolution":resolution,"stale":true,"data":data}),
                ));
            }
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(ErrorBody {
                    error: format!("On-chain OHLCV unavailable: {error}"),
                }),
            ));
        }
    };
    let status = response.status();
    let payload = match response.json::<Value>().await {
        Ok(payload) => payload,
        Err(error) => {
            if let Some(data) = load_cached_candles(&state.market_db_path, pool, resolution, limit)
                .map_err(internal)?
            {
                return Ok(Json(
                    json!({"source":"solana_market_cache","pool":pool,"resolution":resolution,"stale":true,"data":data}),
                ));
            }
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(ErrorBody {
                    error: format!("Invalid on-chain OHLCV response: {error}"),
                }),
            ));
        }
    };
    if !status.is_success() {
        if let Some(data) =
            load_cached_candles(&state.market_db_path, pool, resolution, limit).map_err(internal)?
        {
            return Ok(Json(
                json!({"source":"solana_market_cache","pool":pool,"resolution":resolution,"stale":true,"data":data}),
            ));
        }
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(ErrorBody {
                error: payload
                    .pointer("/errors/0/detail")
                    .or_else(|| payload.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("On-chain OHLCV request failed")
                    .to_string(),
            }),
        ));
    }
    let mut data = payload
        .pointer("/data/attributes/ohlcv_list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let values = row.as_array()?;
            if values.len() < 6 {
                return None;
            }
            Some(json!({
                "timestamp": values[0],
                "open": values[1],
                "high": values[2],
                "low": values[3],
                "close": values[4],
                "volume_usd": values[5],
            }))
        })
        .collect::<Vec<_>>();
    data.reverse();
    if data.is_empty() {
        if let Some(cached) =
            load_cached_candles(&state.market_db_path, pool, resolution, limit).map_err(internal)?
        {
            return Ok(Json(
                json!({"source":"solana_market_cache","pool":pool,"resolution":resolution,"stale":true,"data":cached}),
            ));
        }
    }
    save_market_candles(
        &state.market_db_path,
        pool,
        resolution,
        &data,
        "geckoterminal",
    )
    .map_err(internal)?;
    Ok(Json(json!({
        "source": "geckoterminal_onchain_ohlcv",
        "pool": pool,
        "resolution": resolution,
        "data": data,
    })))
}

async fn proxy_solana_protocols(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let query = SolanaEventQuery::default();
    proxy_solana_json(&state, "protocols", &query).await
}

async fn proxy_solana_listener_start(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SolanaStartRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let url = format!(
        "{}/solana/listener/start",
        state.rpc_price_url.trim_end_matches('/')
    );
    let response = state
        .rpc_http
        .post(url)
        .json(&request)
        .send()
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorBody {
                    error: format!("Solana listener unavailable: {error}"),
                }),
            )
        })?;
    let status = response.status();
    let payload = response.json::<Value>().await.map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            Json(ErrorBody {
                error: format!("invalid Solana listener response: {error}"),
            }),
        )
    })?;
    if !status.is_success() || payload.get("success") == Some(&Value::Bool(false)) {
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(ErrorBody {
                error: payload
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Solana listener start failed")
                    .to_string(),
            }),
        ));
    }
    Ok(Json(payload))
}

async fn proxy_solana_listener_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let query = SolanaEventQuery::default();
    proxy_solana_json(&state, "listener/status", &query).await
}

async fn proxy_solana_json<Q: Serialize + ?Sized>(
    state: &AppState,
    resource: &str,
    query: &Q,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let url = format!(
        "{}/solana/{resource}",
        state.rpc_price_url.trim_end_matches('/')
    );
    let response = state
        .rpc_http
        .get(url)
        .query(query)
        .send()
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorBody {
                    error: format!("Solana listener unavailable: {error}"),
                }),
            )
        })?;
    let status = response.status();
    let payload = response.json::<Value>().await.map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            Json(ErrorBody {
                error: format!("invalid Solana listener response: {error}"),
            }),
        )
    })?;
    if !status.is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(ErrorBody {
                error: payload
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Solana listener request failed")
                    .to_string(),
            }),
        ));
    }
    Ok(Json(payload))
}

async fn watchlist(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let user = escape(&state.watchlist_user_id);
    let sql = format!(
        "SELECT pool_address FROM watchlist WHERE user_id = '{}' AND is_deleted = 0 ORDER BY updated_at DESC",
        user
    );
    let rows = db_query(&state.sqlite_path, &sql).map_err(internal)?;
    let addresses = rows
        .into_iter()
        .filter_map(|row| row.get("pool_address").cloned())
        .collect::<Vec<_>>();
    let count = addresses.len();
    Ok(Json(json!({
        "user_id": state.watchlist_user_id,
        "addresses": addresses,
        "count": count,
        "source": "sqlite",
    })))
}

fn write_sqlite_watchlist(
    path: &PathBuf,
    user: &str,
    address: &str,
    is_deleted: bool,
) -> Result<()> {
    let connection = Connection::open(path)?;
    connection.execute(
        "INSERT INTO watchlist (user_id, pool_address, is_deleted, updated_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(user_id, pool_address) DO UPDATE SET is_deleted=excluded.is_deleted, updated_at=excluded.updated_at",
        rusqlite::params![user, address, if is_deleted { 1 } else { 0 }, chrono_like_now()],
    )?;
    Ok(())
}

async fn watchlist_add(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<WatchlistRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let address = normal_watchlist_address(&payload.pool_address).map_err(internal)?;
    write_sqlite_watchlist(
        &state.sqlite_path,
        &state.watchlist_user_id,
        &address,
        false,
    )
    .map_err(internal)?;
    Ok(Json(
        json!({"ok": true, "pool_address": address, "source": "sqlite"}),
    ))
}

async fn watchlist_remove(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let address = normal_watchlist_address(&address).map_err(internal)?;
    write_sqlite_watchlist(&state.sqlite_path, &state.watchlist_user_id, &address, true)
        .map_err(internal)?;
    Ok(Json(
        json!({"ok": true, "pool_address": address, "source": "sqlite"}),
    ))
}

async fn watchlist_clear(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let user = escape(&state.watchlist_user_id);
    let sql = format!(
        "SELECT pool_address FROM watchlist WHERE user_id = '{}' AND is_deleted = 0",
        user
    );
    let rows = db_query(&state.sqlite_path, &sql).map_err(internal)?;
    let mut removed = 0usize;
    for row in rows {
        if let Some(address) = row.get("pool_address").and_then(Value::as_str) {
            write_sqlite_watchlist(&state.sqlite_path, &state.watchlist_user_id, address, true)
                .map_err(internal)?;
            removed += 1;
        }
    }
    Ok(Json(json!({"ok": true, "removed": removed})))
}

#[derive(Debug, Deserialize)]
struct HedgeSurfaceQuery {
    currency: Option<String>,
    chain: Option<String>,
    tenor: Option<String>,
    limit: Option<u32>,
}

fn option_surface(snapshot: &Value) -> Value {
    let options = snapshot
        .get("options")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut puts = 0u64;
    let mut calls = 0u64;
    for option in &options {
        match option.get("option_type").and_then(Value::as_str) {
            Some("put") => puts += 1,
            Some("call") => calls += 1,
            _ => {}
        }
    }
    json!({
        "status": snapshot.get("status").cloned().unwrap_or(json!("missing")),
        "currency": snapshot.get("currency").cloned().unwrap_or(Value::Null),
        "tenor": snapshot.get("tenor").cloned().unwrap_or(json!("nearest")),
        "generated_at": snapshot.get("generated_at").cloned().unwrap_or(Value::Null),
        "expiry_timestamp": snapshot.get("expiry_timestamp").cloned().unwrap_or(Value::Null),
        "days_to_expiry": snapshot.get("days_to_expiry").cloned().unwrap_or(Value::Null),
        "index_price": snapshot.get("index_price").cloned().unwrap_or(Value::Null),
        "option_count": options.len(),
        "put_count": puts,
        "call_count": calls,
        "options": options,
    })
}

fn option_snapshot_from_db(sqlite_path: &PathBuf, currency: &str, tenor: &str) -> Value {
    let snapshots = db_query(
        sqlite_path,
        &format!(
            "SELECT id, status, generated_at, expiry_timestamp, days_to_expiry, index_price FROM option_snapshots WHERE venue='deribit' AND underlying='{}' AND tenor='{}' ORDER BY generated_at DESC, id DESC LIMIT 1",
            escape(currency),
            escape(tenor)
        ),
    )
    .unwrap_or_default();
    let Some(snapshot) = snapshots.first() else {
        return json!({
            "status": "missing",
            "currency": currency,
            "tenor": tenor,
            "options": [],
        });
    };
    let snapshot_id = snapshot
        .get("id")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let options = db_query(
        sqlite_path,
        &format!(
            "SELECT instrument_name, option_type, strike, expiration_timestamp, contract_size, settlement_currency, best_bid_usd, best_ask_usd, mark_price_usd, mark_iv, open_interest FROM option_quotes WHERE snapshot_id={} ORDER BY strike, option_type, instrument_name",
            snapshot_id
        ),
    )
    .unwrap_or_default();
    json!({
        "status": snapshot.get("status").cloned().unwrap_or_else(|| json!("missing")),
        "currency": currency,
        "tenor": tenor,
        "generated_at": snapshot.get("generated_at").cloned().unwrap_or(Value::Null),
        "expiry_timestamp": snapshot.get("expiry_timestamp").cloned().unwrap_or(Value::Null),
        "days_to_expiry": snapshot.get("days_to_expiry").cloned().unwrap_or(Value::Null),
        "index_price": snapshot.get("index_price").cloned().unwrap_or(Value::Null),
        "options": options,
    })
}

async fn hedge_surface(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<HedgeSurfaceQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let currency = filters
        .currency
        .as_deref()
        .unwrap_or("SOL")
        .to_ascii_uppercase();
    let limit = filters.limit.unwrap_or(40).clamp(1, 100);
    let (asset_clause, pair_clause) = match currency.as_str() {
        "SOL" => ("lower(coalesce(p.asset_symbol, '')) IN ('sol','wsol')", "(lower(p.pair) LIKE 'sol/%' OR lower(p.pair) LIKE 'wsol/%' OR lower(p.pair) LIKE '%/sol' OR lower(p.pair) LIKE '%/wsol')"),
        "ETH" => ("lower(coalesce(p.asset_symbol, '')) IN ('eth','weth')", "(lower(p.pair) LIKE 'eth/%' OR lower(p.pair) LIKE 'weth/%' OR lower(p.pair) LIKE '%/eth' OR lower(p.pair) LIKE '%/weth')"),
        "BTC" => ("lower(coalesce(p.asset_symbol, '')) IN ('btc','wbtc')", "(lower(p.pair) LIKE 'btc/%' OR lower(p.pair) LIKE 'wbtc/%' OR lower(p.pair) LIKE '%/btc' OR lower(p.pair) LIKE '%/wbtc')"),
        _ => ("1=0", "1=0"),
    };
    let chain_clause = filters
        .chain
        .as_deref()
        .filter(|value| !value.is_empty() && *value != "all")
        .map(|value| format!(" AND lower(p.chain) = lower('{}')", escape(value)))
        .unwrap_or_default();
    let sql = format!(
        "SELECT {} FROM pools p INNER JOIN hedge_analysis h ON h.pool_address = p.pool_address LEFT JOIN pool_rpc_state s ON s.pool_address = p.pool_address LEFT JOIN pool_volatility v ON v.pool_address = p.pool_address {} WHERE {} AND h.option_status = 'complete' AND p.pair_category = 'stable_quote' AND ({} OR {}) AND coalesce(p.liquidity_usd, 0) > 0{} ORDER BY fee_apr DESC LIMIT {}",
        pool_columns(), LATEST_POOL_PRICE_JOIN, PRICED_POOL_FILTER, asset_clause, pair_clause,
        chain_clause, limit
    );
    let (rows, source) = query(&state, &sql).await.map_err(internal)?;
    let rows = attach_runtime_volatility(&state.sqlite_path, rows).map_err(internal)?;
    let quarterly = filters.tenor.as_deref() == Some("quarterly");
    let tenor = if quarterly { "quarterly" } else { "nearest" };
    let snapshot = option_surface(&option_snapshot_from_db(
        &state.sqlite_path,
        &currency,
        tenor,
    ));
    let pools = if snapshot.get("status").and_then(Value::as_str) == Some("ok") {
        rows.into_iter().map(decorate_pool).collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    Ok(Json(json!({
        "currency": currency,
        "source": source,
        "selection": {
            "venue": "deribit",
            "tenor": if quarterly { "quarterly" } else { "nearest" },
            "hedgeable_only": true,
            "chain": filters.chain,
        },
        "option_snapshot": snapshot,
        "pools": pools,
        "notes": [
            "Only pools with a complete direct Deribit hedge plan are shown.",
            "Fee APR is shown for both the 24h and 7d volume windows and is not a guaranteed return.",
            "Refresh rpc/option, then run backend/lp-options to calculate the hedge plan before execution."
        ]
    })))
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(json!({"ok": true, "source": "sqlite", "sqlite": state.sqlite_path.display().to_string()}))
}

fn chain_scope(filters: &PoolQuery) -> String {
    filters.chain.as_deref()
        .filter(|chain| *chain != "all" && !chain.trim().is_empty())
        .map(|chain| format!(" WHERE {}", exact_filter("chain", chain)))
        .unwrap_or_default()
}

async fn summary(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<PoolQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let sql = "SELECT count(*) AS pools, coalesce(sum(liquidity_usd), 0) AS liquidity, coalesce(sum(volume_24h_usd), 0) AS volume, count(DISTINCT chain) AS chains, sum(CASE WHEN lp_fee_apr_estimate >= 1 THEN 1 ELSE 0 END) AS high_fee FROM pools";
    let scope = chain_scope(&filters);
    let (mut rows, source) = query(&state, &format!("{sql}{scope}")).await.map_err(internal)?;
    let mut value = rows.pop().unwrap_or_else(|| json!({}));
    if let Value::Object(ref mut map) = value {
        let hedge_scope = if scope.is_empty() { String::new() } else {
            format!(" AND pool_address IN (SELECT pool_address FROM pools{scope})")
        };
        let hedge_sql = format!(
            "SELECT count(*) AS hedgeable FROM hedge_analysis WHERE option_status = 'complete'{hedge_scope}");
        let (hedge, _) = query(&state, &hedge_sql).await.map_err(internal)?;
        map.insert(
            "hedgeable".into(),
            hedge
                .first()
                .and_then(|x| x.get("hedgeable"))
                .cloned()
                .unwrap_or(json!(0)),
        );
        map.insert("source".into(), json!(source));
    }
    Ok(Json(value))
}

async fn rpc_state_summary(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let totals_sql = "SELECT count(*) AS total, sum(CASE WHEN state_status IN ('live', 'account_present') THEN 1 ELSE 0 END) AS live, sum(CASE WHEN state_status = 'rpc_error' THEN 1 ELSE 0 END) AS rpc_error, sum(CASE WHEN state_status = 'transport_error' THEN 1 ELSE 0 END) AS transport_error, sum(CASE WHEN state_status = 'account_missing' THEN 1 ELSE 0 END) AS account_missing, sum(CASE WHEN state_status = 'unsupported_protocol' THEN 1 ELSE 0 END) AS unsupported_protocol, max(checked_at) AS checked_at, max(state_block) AS latest_block FROM pool_rpc_state";
    let chains_sql = "SELECT chain, count(*) AS total, sum(CASE WHEN state_status IN ('live', 'account_present') THEN 1 ELSE 0 END) AS live, sum(CASE WHEN state_status IN ('rpc_error', 'transport_error', 'account_missing') THEN 1 ELSE 0 END) AS unavailable, sum(CASE WHEN state_status = 'unsupported_protocol' THEN 1 ELSE 0 END) AS unsupported_protocol, max(checked_at) AS checked_at, max(state_block) AS latest_block FROM pool_rpc_state GROUP BY chain ORDER BY chain";
    let (mut totals, source) = query(&state, totals_sql).await.map_err(internal)?;
    let (chains, _) = query(&state, chains_sql).await.map_err(internal)?;
    let mut value = totals.pop().unwrap_or_else(|| json!({}));
    if let Value::Object(ref mut map) = value {
        map.insert("source".into(), json!(source));
        map.insert("chains".into(), Value::Array(chains));
    }
    Ok(Json(value))
}

async fn price_history_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let history = db_query(
        &state.sqlite_path,
        "SELECT count(*) AS rows, count(DISTINCT chain || ':' || address) AS tokens, min(timestamp) AS oldest_timestamp, max(timestamp) AS newest_timestamp FROM token_price_history",
    )
    .map_err(internal)?;
    let token_metrics = db_query(
        &state.sqlite_path,
        "SELECT count(*) AS tokens, sum(avg_price_1d IS NOT NULL) AS avg_1d, sum(avg_price_3d IS NOT NULL) AS avg_3d, sum(avg_price_7d IS NOT NULL) AS avg_7d, sum(avg_price_30d IS NOT NULL) AS avg_30d, sum(vol_1d IS NOT NULL) AS vol_1d, sum(vol_3d IS NOT NULL) AS vol_3d, sum(vol_7d IS NOT NULL) AS vol_7d, sum(vol_30d IS NOT NULL) AS vol_30d FROM token_volatility",
    )
    .map_err(internal)?;
    let has_pool_status =
        sqlite_has_column(&state.sqlite_path, "pool_price_history", "price_status")
            .map_err(internal)?;
    let pair_status_filter = if has_pool_status {
        " WHERE coalesce(price_status, 'priced') = 'priced'"
    } else {
        ""
    };
    let pair_metrics = db_query(
        &state.sqlite_path,
        &format!("SELECT count(*) AS pools, sum(sample_count >= 24) AS complete_1d, sum(sample_count >= 168) AS complete_7d, max(sample_count) AS max_hourly_points FROM (SELECT chain, pool_address, count(*) AS sample_count FROM pool_price_history{} GROUP BY chain, pool_address)", pair_status_filter),
    )
    .map_err(internal)?;
    let pool_usd_columns = if sqlite_has_column(
        &state.sqlite_path,
        "pool_price_history",
        "token0_price_usd",
    )
    .map_err(internal)?
        && sqlite_has_column(&state.sqlite_path, "pool_price_history", "token1_price_usd")
            .map_err(internal)?
    {
        "sum(token0_price_usd IS NOT NULL) AS token0_usd_rows, sum(token1_price_usd IS NOT NULL) AS token1_usd_rows"
    } else {
        "0 AS token0_usd_rows, 0 AS token1_usd_rows"
    };
    let pool_status_columns = if has_pool_status {
        "sum(price_status = 'priced') AS priced_rows, sum(price_status = 'unavailable') AS unavailable_rows"
    } else {
        "count(*) AS priced_rows, 0 AS unavailable_rows"
    };
    let pool_history = db_query(
        &state.sqlite_path,
        &format!("SELECT count(*) AS rows, count(DISTINCT chain || ':' || pool_address) AS pools, min(timestamp) AS oldest_timestamp, max(timestamp) AS newest_timestamp, {pool_usd_columns}, {pool_status_columns} FROM pool_price_history"),
    )
    .map_err(internal)?;
    let live_pool_coverage = if has_pool_status {
        db_query(
            &state.sqlite_path,
            "SELECT count(DISTINCT p.chain || ':' || p.pool_address) AS live_pools, count(DISTINCT CASE WHEN h.price_status = 'priced' THEN p.chain || ':' || p.pool_address END) AS priced_live_pools, count(DISTINCT CASE WHEN h.price_status = 'unavailable' THEN p.chain || ':' || p.pool_address END) AS unavailable_live_pools, count(DISTINCT CASE WHEN h.pool_address IS NULL THEN p.chain || ':' || p.pool_address END) AS missing_live_pools FROM pools p JOIN pool_rpc_state s ON s.pool_address = p.pool_address LEFT JOIN (SELECT h.chain, h.pool_address, h.price_status FROM pool_price_history h JOIN (SELECT chain, pool_address, max(timestamp) AS timestamp FROM pool_price_history GROUP BY chain, pool_address) latest ON latest.chain = h.chain AND latest.pool_address = h.pool_address AND latest.timestamp = h.timestamp) h ON h.chain = p.chain AND h.pool_address = p.pool_address WHERE s.state_status = 'live'",
        )
        .map_err(internal)?
    } else {
        Vec::new()
    };
    let pool_sources = db_query(
        &state.sqlite_path,
        "SELECT source, count(*) AS rows, count(DISTINCT chain || ':' || pool_address) AS pools FROM pool_price_history GROUP BY source ORDER BY source",
    )
    .map_err(internal)?;
    let sources = db_query(
        &state.sqlite_path,
        "SELECT source, count(*) AS rows, count(DISTINCT chain || ':' || address) AS tokens FROM token_price_history GROUP BY source ORDER BY source",
    )
    .map_err(internal)?;
    Ok(Json(json!({
        "retention_days": 30,
        "bucket_seconds": 3600,
        "history": history.first().cloned().unwrap_or_else(|| json!({})),
        "pools": pool_history.first().cloned().unwrap_or_else(|| json!({})),
        "pool_sources": pool_sources,
        "tokens": token_metrics.first().cloned().unwrap_or_else(|| json!({})),
        "pairs": pair_metrics.first().cloned().unwrap_or_else(|| json!({})),
        "live_pool_coverage": live_pool_coverage.first().cloned().unwrap_or_else(|| json!({})),
        "sources": sources,
        "rpc_policy": "raw price/history only; backend computes averages and volatility",
        "refresh_policy": "background refresh when local history is missing or stale; pool list reads never synchronously fan out to RPC",
    })))
}

async fn token_volatility(
    State(state): State<Arc<AppState>>,
    Path((chain, address)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let report = token_volatility_report(&state.sqlite_path, &chain, &address).map_err(internal)?;
    Ok(Json(report))
}

async fn pool_volatility(
    State(state): State<Arc<AppState>>,
    Path((chain, address)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let Some(report) =
        pool_volatility_report(&state.sqlite_path, &chain, &address).map_err(internal)?
    else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorBody {
                error: "pool not found".into(),
            }),
        ));
    };
    Ok(Json(report))
}

async fn meta(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<PoolQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let scope = chain_scope(&filters);
    let (chains, _) = query(&state, &format!("SELECT DISTINCT chain FROM pools{scope} ORDER BY chain"))
        .await
        .map_err(internal)?;
    let (protocols, _) = query(
        &state,
        &format!("SELECT DISTINCT protocol FROM pools{scope} ORDER BY protocol"),
    )
    .await
    .map_err(internal)?;
    let (pairs, _) = query(
        &state,
        &format!("SELECT DISTINCT pair FROM pools{scope} ORDER BY pair LIMIT 5000"),
    )
    .await
    .map_err(internal)?;
    let pick = |rows: Vec<Value>, key: &str| {
        rows.into_iter()
            .filter_map(|x| x.get(key).cloned())
            .collect::<Vec<_>>()
    };
    Ok(Json(
        json!({"chains": pick(chains, "chain"), "protocols": pick(protocols, "protocol"), "pairs": pick(pairs, "pair")}),
    ))
}

async fn pools_impl(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<PoolQuery>,
    path_chain: Option<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let page = filters.page.unwrap_or(1).max(1);
    let page_size = filters.page_size.unwrap_or(50).clamp(1, 100);
    let sort = filters
        .sort
        .as_deref()
        .filter(|x| SORTS.contains(x))
        .unwrap_or("volume_24h_usd");
    let order = if filters.order.as_deref() == Some("asc") {
        "ASC"
    } else {
        "DESC"
    };
    let order_column = if matches!(
        sort,
        "effective_lp_fee_apr" | "effective_fee_bps" | "fee_apr" | "7d_fee_apr"
    ) {
        if sort == "7d_fee_apr" {
            "\"7d_fee_apr\"".to_string()
        } else {
            sort.to_string()
        }
    } else {
        format!("p.{sort}")
    };
    let where_sql = where_clause(&filters, path_chain.as_deref());
    let count_sql = format!("SELECT count(*) AS total FROM pools p LEFT JOIN pool_rpc_state s ON s.pool_address = p.pool_address {} {}", LATEST_POOL_PRICE_JOIN, where_sql);
    let (count_rows, _) = query(&state, &count_sql).await.map_err(internal)?;
    let total = count_rows
        .first()
        .and_then(|x| x.get("total"))
        .cloned()
        .unwrap_or(json!(0));
    let offset = (page - 1) * page_size;
    let items_sql = format!("SELECT {} FROM pools p LEFT JOIN hedge_analysis h ON h.pool_address = p.pool_address LEFT JOIN pool_rpc_state s ON s.pool_address = p.pool_address LEFT JOIN pool_volatility v ON v.pool_address = p.pool_address {} {} ORDER BY {} {} LIMIT {} OFFSET {}", pool_columns(), LATEST_POOL_PRICE_JOIN, where_sql, order_column, order, page_size, offset);
    let (items, source) = query(&state, &items_sql).await.map_err(internal)?;
    let items = attach_runtime_volatility(&state.sqlite_path, items).map_err(internal)?;
    let items = items.into_iter().map(decorate_pool).collect::<Vec<_>>();
    Ok(Json(json!({
        "items": items,
        "total": total,
        "page": page,
        "page_size": page_size,
        "source": source,
        "filters": {
            "chain": path_chain.as_deref().or(filters.chain.as_deref()).unwrap_or("all"),
            "protocol": filters.protocol.as_deref().unwrap_or("all"),
            "protocols": filters.protocols.as_deref().unwrap_or("all"),
            "pair": filters.pair.as_deref().unwrap_or("all"),
            "pair_category": filters.pair_category.as_deref().unwrap_or("all"),
            "pool_type": filters.pool_type.as_deref().unwrap_or("all"),
            "min_liquidity_usd": filters.min_liquidity_usd,
            "min_volume_24h_usd": filters.min_volume_24h_usd,
            "min_fee_bps": filters.min_fee_bps,
            "min_lp_fee_apr": filters.min_lp_fee_apr,
            "fee_apr_status": filters.fee_apr_status.as_deref().unwrap_or("all"),
            "rpc_status": filters.rpc_status.as_deref().unwrap_or("all"),
        }
    })))
}

async fn pools(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<PoolQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    pools_impl(State(state), Query(filters), None).await
}

async fn chain_pools(
    State(state): State<Arc<AppState>>,
    Path(chain): Path<String>,
    Query(filters): Query<PoolQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    pools_impl(State(state), Query(filters), Some(chain)).await
}

async fn pool(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let sql = format!("SELECT {} FROM pools p LEFT JOIN hedge_analysis h ON h.pool_address = p.pool_address LEFT JOIN pool_rpc_state s ON s.pool_address = p.pool_address LEFT JOIN pool_volatility v ON v.pool_address = p.pool_address {} WHERE {} AND lower(p.pool_address) = lower('{}') LIMIT 1", pool_columns(), LATEST_POOL_PRICE_JOIN, PRICED_POOL_FILTER, escape(&address));
    let (rows, _) = query(&state, &sql).await.map_err(internal)?;
    let rows = attach_runtime_volatility(&state.sqlite_path, rows).map_err(internal)?;
    let Some(row) = rows.into_iter().next() else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorBody {
                error: "pool not found".into(),
            }),
        ));
    };
    let row = attach_live_price(&state, row).await;
    Ok(Json(decorate_pool(row)))
}

async fn attach_live_price(state: &AppState, mut row: Value) -> Value {
    let (chain, pool_address, token_a, token_b, body) = {
        let Some(object) = row.as_object() else {
            return row;
        };
        let Some(chain) = object.get("chain").and_then(Value::as_str) else {
            return row;
        };
        let Some(pool_address) = object.get("pool_address").and_then(Value::as_str) else {
            return row;
        };
        let Some(pool_type) = object.get("pool_type").and_then(Value::as_str) else {
            return row;
        };
        (
            chain.to_string(),
            pool_address.to_string(),
            object
                .get("token_a_address")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            object
                .get("token_b_address")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            json!({
                "pool_type": pool_type,
                "token0_symbol": object.get("token_a_symbol").and_then(Value::as_str),
                "token1_symbol": object.get("token_b_symbol").and_then(Value::as_str),
                "token0_anchor_usd": object.get("token_a_price_usd").and_then(Value::as_f64),
                "token1_anchor_usd": object.get("token_b_price_usd").and_then(Value::as_f64),
            }),
        )
    };
    let route_chain = if chain.eq_ignore_ascii_case("solana") {
        "solana-mainnet"
    } else {
        &chain
    };
    let url = format!(
        "{}/price/{}/{}",
        state.rpc_price_url.trim_end_matches('/'),
        route_chain,
        pool_address
    );
    let Ok(response) = state
        .rpc_http
        .post(url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
    else {
        return row;
    };
    if !response.status().is_success() {
        return row;
    }
    let Ok(quote) = response.json::<Value>().await else {
        return row;
    };
    let Some(quote_object) = quote.as_object() else {
        return row;
    };
    if quote_object.get("status").and_then(Value::as_str).is_none() {
        return row;
    }
    let q0 = quote_object
        .get("token0_address")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let q1 = quote_object
        .get("token1_address")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let p0 = quote_object
        .get("token0_price_usd")
        .cloned()
        .unwrap_or(Value::Null);
    let p1 = quote_object
        .get("token1_price_usd")
        .cloned()
        .unwrap_or(Value::Null);
    if let Some(target) = row.as_object_mut() {
        if token_a.eq_ignore_ascii_case(q0) {
            target.insert("token_a_price_usd".into(), p0.clone());
            target.insert("token_b_price_usd".into(), p1.clone());
        } else if token_a.eq_ignore_ascii_case(q1) {
            target.insert("token_a_price_usd".into(), p1.clone());
            target.insert("token_b_price_usd".into(), p0.clone());
        } else if token_a.is_empty() || token_b.is_empty() {
            target.insert("token_a_price_usd".into(), p0.clone());
            target.insert("token_b_price_usd".into(), p1.clone());
        }
        target.insert(
            "live_price_status".into(),
            quote_object.get("status").cloned().unwrap_or(Value::Null),
        );
        target.insert("live_price_source".into(), json!("rpc_atomic_pool_read"));
    }
    row
}

async fn rpc_refresh(State(state): State<Arc<AppState>>) -> Json<Value> {
    let path = &state.rpc_refresh_path;
    Json(
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| json!({"available": false, "path": path.display().to_string()})),
    )
}

fn internal(error: anyhow::Error) -> (StatusCode, Json<ErrorBody>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorBody {
            error: error.to_string(),
        }),
    )
}

fn short_token_label(symbol: Option<&Value>, address: Option<&Value>) -> String {
    let symbol = symbol.and_then(Value::as_str).unwrap_or("").trim();
    if !symbol.is_empty() && !symbol.eq_ignore_ascii_case("unknown") {
        return symbol.to_string();
    }
    let address = address.and_then(Value::as_str).unwrap_or("").trim();
    if address.eq_ignore_ascii_case("0x0000000000000000000000000000000000000000") {
        return "NATIVE".into();
    }
    if address.len() >= 12 && address.starts_with("0x") {
        return format!("{}…{}", &address[..8], &address[address.len() - 4..]);
    }
    if !address.is_empty() {
        return format!(
            "{}…{}",
            &address[..address.len().min(6)],
            &address[address.len().saturating_sub(4)..]
        );
    }
    "UNRESOLVED".into()
}

fn is_stable_symbol(symbol: &str, stable_symbol: &str) -> bool {
    let normalized = symbol.trim().to_ascii_uppercase();
    !normalized.is_empty()
        && (normalized == stable_symbol.trim().to_ascii_uppercase()
            || matches!(
                normalized.as_str(),
                "USDC" | "USDT" | "DAI" | "USDE" | "USDS" | "USD" | "FRAX" | "LUSD" | "USDBC"
            ))
}

fn decorate_pool(mut row: Value) -> Value {
    let Value::Object(ref mut object) = row else {
        return row;
    };
    let token_a_symbol_raw = object
        .get("token_a_symbol")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let token_b_symbol_raw = object
        .get("token_b_symbol")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let stable_symbol = object
        .get("stable_symbol")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let asset_symbol = object
        .get("asset_symbol")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let asset_price = object.get("price_usd").and_then(Value::as_f64);
    let token_a = short_token_label(object.get("token_a_symbol"), object.get("token_a_address"));
    let token_b = short_token_label(object.get("token_b_symbol"), object.get("token_b_address"));
    let raw_pair = object.get("pair").and_then(Value::as_str).unwrap_or("");
    let has_unknown = raw_pair.to_ascii_lowercase().contains("unknown")
        || object
            .get("token_a_symbol")
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("unknown"))
        || object
            .get("token_b_symbol")
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("unknown"));
    let display_pair = if has_unknown || raw_pair.is_empty() {
        format!("{token_a}/{token_b}")
    } else {
        raw_pair.to_string()
    };
    object.insert("pair_display".into(), json!(display_pair));
    object.insert(
        "metadata_status".into(),
        json!(if has_unknown {
            "token_symbol_missing"
        } else {
            "complete"
        }),
    );
    for (key, fallback_price) in [
        (
            "token_a_price_usd",
            if is_stable_symbol(&token_a_symbol_raw, &stable_symbol) {
                Some(1.0)
            } else if token_a_symbol_raw.eq_ignore_ascii_case(&asset_symbol) {
                asset_price
            } else {
                None
            },
        ),
        (
            "token_b_price_usd",
            if is_stable_symbol(&token_b_symbol_raw, &stable_symbol) {
                Some(1.0)
            } else if token_b_symbol_raw.eq_ignore_ascii_case(&asset_symbol) {
                asset_price
            } else {
                None
            },
        ),
    ] {
        if object.get(key).map_or(true, Value::is_null) {
            if let Some(value) = fallback_price {
                object.insert(key.into(), json!(value));
            }
        }
    }
    row
}

#[tokio::main]
async fn main() -> Result<()> {
    let sqlite_path = env::var("LP_SQLITE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_sqlite_path());
    let price_db_path = env::var("LP_PRICE_DB").unwrap_or_else(|_| {
        sqlite_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("lp_price_history.db")
            .to_string_lossy()
            .into_owned()
    });
    env::set_var("LP_PRICE_DB", &price_db_path);
    let market_db_path = env::var("SOLANA_MARKET_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_project_path("rpc/data/solana_market.db"));
    initialize_market_db(&market_db_path)?;
    let state = Arc::new(AppState {
        sqlite_path,
        market_db_path,
        rpc_refresh_path: env::var("LP_RPC_REFRESH_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_project_path("lp_refresh_latest.json")),
        watchlist_user_id: env_or("LP_WATCHLIST_USER", "local"),
        rpc_price_url: env_or("LP_RPC_PRICE_URL", "http://127.0.0.1:8545"),
        rpc_http: reqwest::Client::new(),
        vybe_api_key: env::var("VYBE_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    });
    ensure_sqlite_watchlist(&state.sqlite_path)?;
    let app = Router::new()
        .route("/api/monad/swap/quote", axum::routing::post(swap::quote))
        .route("/api/monad/swap/build", axum::routing::post(swap::build))
        .route("/api/monad/wallet", get(wallet::query))
        .route("/api/monad/wallet/{address}", get(wallet::by_address))
        .route("/monad/display", get(monad::display).put(monad::save))
        .route("/monad/demo", get(monad::demo))
        .route("/monad/prices", get(monad::prices))
        .route("/api/monad/prices", get(monad::prices))
        .route("/api/monad/demo", get(monad::demo))
        .route("/api/selection/quote", get(selection::quote_get).post(selection::quote_post))
        .route("/api/selection/pools", get(selection::pools))
        .route("/api/selection/quote-stream", get(selection::quotes_stream_get).post(selection::quotes_stream_post))
        .route("/api/monad/prices-stream", get(monad::prices_stream))
        .route("/api/selection/testnet-rpc", post(monad::selection_testnet_rpc))
        .route(
            "/api/monad/trade-rpc",
            axum::routing::post(monad::trade_rpc),
        )
        .route("/monad/{pool}/{metric}", get(monad::metric))
        .route("/api/monad/display", get(monad::display).put(monad::save))
        .route("/api/monad/resolve", get(monad::resolve))
        .route("/api/monad/pools", get(monad::pools))
        .route(
            "/api/monad/listener/start",
            axum::routing::post(monad::save),
        )
        .route("/api/monad/listener/status", get(monad::status))
        .route("/api/monad/pool-candles", get(monad::candles))
        .route("/api/monad/{endpoint}", get(monad::proxy))
        .route("/api/health", get(health))
        .route("/api/summary", get(summary))
        .route("/api/rpc-state/summary", get(rpc_state_summary))
        .route("/api/meta", get(meta))
        .route("/api/price-history/status", get(price_history_status))
        .route("/api/vol/{chain}/token/{address}", get(token_volatility))
        .route("/api/vol/{chain}/pool/{address}", get(pool_volatility))
        .route("/vol/{chain}/token/{address}", get(token_volatility))
        .route("/vol/{chain}/pool/{address}", get(pool_volatility))
        .route("/api/pools", get(pools))
        .route("/api/chains/{chain}/pools", get(chain_pools))
        .route("/api/pools/{address}", get(pool))
        .route("/api/solana/resolve", get(solana_resolve))
        .route("/api/solana/pools", get(solana_pools))
        .route("/api/solana/events", get(proxy_solana_events))
        .route("/api/solana/whale-alerts", get(proxy_solana_alerts))
        .route("/api/solana/pool-snapshots", get(proxy_solana_snapshots))
        .route("/api/solana/pool-history", get(proxy_solana_pool_history))
        .route("/api/solana/pool-candles", get(proxy_solana_pool_candles))
        .route("/api/solana/protocols", get(proxy_solana_protocols))
        .route(
            "/api/solana/listener/start",
            axum::routing::post(proxy_solana_listener_start),
        )
        .route(
            "/api/solana/listener/status",
            get(proxy_solana_listener_status),
        )
        .route(
            "/api/watchlist",
            get(watchlist).post(watchlist_add).delete(watchlist_clear),
        )
        .route("/api/watchlist/{address}", delete(watchlist_remove))
        .route("/api/option-hedge", get(hedge_surface))
        .route("/api/hedge-surface", get(hedge_surface))
        .route("/api/rpc-refresh", get(rpc_refresh))
        .layer(CorsLayer::permissive())
        .with_state(state);
    let address: SocketAddr = env_or("LP_API_ADDR", "0.0.0.0:8100").parse()?;
    println!("LP Rust API listening on http://{}", address);
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
