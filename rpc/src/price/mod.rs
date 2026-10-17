//! Token price transport and history maintenance.
//!
//! Source order is deliberately explicit:
//! 1. DefiLlama supplies sparse historical observations when it knows a token.
//! 2. For tokens absent there, the local RPC refresh snapshot is decoded from
//!    `pool_rpc_state` and a current observation is derived through stablecoin
//!    price propagation. Existing sampled prices are also allowed to anchor a
//!    disconnected pool component, so a token learned yesterday can price its
//!    new pair-mate today without another external oracle request.
//!
//! This module never computes volatility. It writes token observations to
//! `token_price_history` and pool-local relative prices to
//! `pool_price_history`; backend code computes all derived metrics.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use base64::Engine;
use reqwest::Client;
use rusqlite::{params, Connection};
use serde_json::Value;
use tiny_keccak::{Hasher, Keccak};
use tokio::task::JoinSet;

mod source;
mod types;

use source::fetch_batch;
use types::*;

const DEFAULT_LP_DB: &str = "data/lp_screener.db";
const LLAMA_URL: &str = "https://coins.llama.fi/chart/";
const RPC_SAMPLE_SECS: i64 = 3_600;

fn price_db_path(catalog_path: &str) -> String {
    env::var_os("LP_PRICE_DB")
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            Path::new(catalog_path)
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("lp_price_history.db")
                .to_string_lossy()
                .into_owned()
        })
}

fn arg(name: &str, default: &str) -> String {
    let args: Vec<String> = env::args().collect();
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .unwrap_or_else(|| default.to_string())
}

fn enabled(name: &str) -> bool {
    matches!(
        arg(name, "false").to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

fn normalize_address(value: &str) -> String {
    if value.starts_with("0x") {
        value.to_ascii_lowercase()
    } else {
        value.to_string()
    }
}

fn llama_chain(chain: &str) -> &str {
    match chain {
        "avalanche" => "avax",
        "zksync" => "era",
        other => other,
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default()
}

fn main_table_columns(db: &Connection, table: &str) -> Result<HashMap<String, bool>> {
    let mut columns = HashMap::new();
    let exists = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get::<_, i64>(0),
    )?;
    if exists == 0 {
        return Ok(columns);
    }
    let mut statement = db.prepare(&format!("PRAGMA main.table_info({table})"))?;
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, i64>(3)?))
    })? {
        let (name, not_null) = row?;
        columns.insert(name, not_null != 0);
    }
    Ok(columns)
}

fn ensure_schema(db: &mut Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS price.token_price_history (
           chain TEXT NOT NULL,
           address TEXT NOT NULL,
           timestamp INTEGER NOT NULL,
           price_usd REAL NOT NULL,
           source TEXT NOT NULL,
           observed_at INTEGER NOT NULL,
           PRIMARY KEY (chain, address, timestamp)
         );
         CREATE TABLE IF NOT EXISTS token_metadata (
           chain TEXT NOT NULL,
           address TEXT NOT NULL,
           symbol TEXT,
           name TEXT,
           decimals INTEGER,
           status TEXT NOT NULL,
           source TEXT NOT NULL,
           checked_at INTEGER NOT NULL,
           error TEXT,
           code_present INTEGER,
           metadata_issue TEXT,
           PRIMARY KEY (chain, address)
         );
         CREATE INDEX IF NOT EXISTS idx_token_metadata_lookup
           ON token_metadata(chain, address, decimals);
         CREATE TABLE IF NOT EXISTS price.pool_price_history (
           chain TEXT NOT NULL,
           pool_address TEXT NOT NULL,
           timestamp INTEGER NOT NULL,
           token0_address TEXT NOT NULL,
           token1_address TEXT NOT NULL,
           token1_per_token0 REAL,
           token0_price_usd REAL,
           token1_price_usd REAL,
           source TEXT NOT NULL,
           price_status TEXT NOT NULL DEFAULT 'priced',
           status_reason TEXT,
           observed_at INTEGER NOT NULL,
           PRIMARY KEY (chain, pool_address, timestamp)
         );
         CREATE TABLE IF NOT EXISTS price.token_price_fetch_state (
           chain TEXT NOT NULL,
           address TEXT NOT NULL,
           source TEXT NOT NULL,
           last_attempt_at INTEGER NOT NULL,
           last_success_at INTEGER,
           last_error TEXT,
           PRIMARY KEY (chain, address, source)
         );
         ",
    )?;

    // First-run migration for the former single-file layout. The source
    // tables are exact, known legacy price tables; catalog tables are left in
    // the main database. INSERT OR IGNORE makes a rerun harmless.
    let token_columns = main_table_columns(db, "token_price_history")?;
    if !token_columns.is_empty() {
        db.execute(
            "INSERT OR IGNORE INTO price.token_price_history
             (chain,address,timestamp,price_usd,source,observed_at)
             SELECT chain,address,timestamp,price_usd,source,observed_at
             FROM main.token_price_history",
            [],
        )?;
        db.execute("DROP TABLE main.token_price_history", [])?;
    }

    let pool_columns = main_table_columns(db, "pool_price_history")?;
    if !pool_columns.is_empty() {
        let token0_usd = if pool_columns.contains_key("token0_price_usd") {
            "token0_price_usd"
        } else {
            "NULL"
        };
        let token1_usd = if pool_columns.contains_key("token1_price_usd") {
            "token1_price_usd"
        } else {
            "NULL"
        };
        let price_status = if pool_columns.contains_key("price_status") {
            "price_status"
        } else {
            "'priced'"
        };
        let status_reason = if pool_columns.contains_key("status_reason") {
            "status_reason"
        } else {
            "NULL"
        };
        db.execute(
            &format!(
                "INSERT OR IGNORE INTO price.pool_price_history
             (chain,pool_address,timestamp,token0_address,token1_address,
              token1_per_token0,token0_price_usd,token1_price_usd,source,
              price_status,status_reason,observed_at)
             SELECT chain,pool_address,timestamp,token0_address,token1_address,
                    token1_per_token0,{token0_usd},{token1_usd},source,
                    {price_status},{status_reason},observed_at
             FROM main.pool_price_history"
            ),
            [],
        )?;
        db.execute("DROP TABLE main.pool_price_history", [])?;
    }

    let fetch_columns = main_table_columns(db, "token_price_fetch_state")?;
    if !fetch_columns.is_empty() {
        db.execute(
            "INSERT OR IGNORE INTO price.token_price_fetch_state
             (chain,address,source,last_attempt_at,last_success_at,last_error)
             SELECT chain,address,source,last_attempt_at,last_success_at,last_error
             FROM main.token_price_fetch_state",
            [],
        )?;
        db.execute("DROP TABLE main.token_price_fetch_state", [])?;
    }
    Ok(())
}

fn load_fetch_attempts(db: &Connection) -> Result<HashMap<(String, String), i64>> {
    let mut attempts = HashMap::new();
    let mut statement = db.prepare(
        "SELECT chain, address, last_attempt_at
         FROM price.token_price_fetch_state
         WHERE source='defillama'",
    )?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })? {
        let (chain, address, attempted_at) = row?;
        attempts.insert((chain, normalize_address(&address)), attempted_at);
    }
    Ok(attempts)
}

fn load_tokens(
    db: &Connection,
    limit: usize,
    now: i64,
    retry_after: i64,
    refresh_external: bool,
) -> Result<Vec<TokenRef>> {
    let attempts = load_fetch_attempts(db)?;
    let mut statement = db.prepare(
        "SELECT token_a_address, token_a_symbol, token_b_address, token_b_symbol, chain
         FROM pools ORDER BY id",
    )?;
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
        ))
    })? {
        let (a, _sa, b, _sb, chain) = row?;
        for address in [a, b] {
            let Some(address) = address.filter(|value| !value.trim().is_empty()) else {
                continue;
            };
            let key = (chain.clone(), normalize_address(&address));
            if !seen.insert(key.clone()) {
                continue;
            }
            let recently_attempted = attempts
                .get(&key)
                .is_some_and(|attempted_at| now.saturating_sub(*attempted_at) < retry_after);
            if refresh_external || !recently_attempted {
                result.push(TokenRef {
                    chain: key.0,
                    address: key.1,
                });
            }
            if limit > 0 && result.len() >= limit {
                return Ok(result);
            }
        }
    }
    Ok(result)
}

fn metadata_decimals(db: &Connection) -> Result<HashMap<(String, String), u32>> {
    let mut result = HashMap::new();
    let mut statement = db.prepare(
        "SELECT chain, address, decimals FROM token_metadata WHERE decimals IS NOT NULL",
    )?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })? {
        let (chain, address, decimals) = row?;
        if (0..=36).contains(&decimals) {
            result.insert((chain, normalize_address(&address)), decimals as u32);
        }
    }
    Ok(result)
}

fn stable_symbols(symbol: &str) -> bool {
    matches!(
        symbol.to_ascii_uppercase().as_str(),
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
    )
}

fn native_address(address: &str) -> bool {
    matches!(
        address,
        "0x0000000000000000000000000000000000000000"
            | "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            | "0x0000000000000000000000000000000000000001"
    )
}

fn missing_evm_decimals(
    db: &Connection,
    chain_filter: Option<&str>,
    protocol_filter: Option<&str>,
) -> Result<Vec<TokenRef>> {
    let mut statement = db.prepare(
        "WITH candidates AS (
           SELECT p.chain, lower(coalesce(s.token0_address, p.token_a_address)) AS address
           FROM pools p JOIN pool_rpc_state s ON s.pool_address=p.pool_address
           WHERE s.state_status='live'
             AND (?1 IS NULL OR lower(p.chain)=lower(?1))
             AND (?2 IS NULL OR lower(p.protocol)=lower(?2))
           UNION
           SELECT p.chain, lower(coalesce(s.token1_address, p.token_b_address)) AS address
           FROM pools p JOIN pool_rpc_state s ON s.pool_address=p.pool_address
           WHERE s.state_status='live'
             AND (?1 IS NULL OR lower(p.chain)=lower(?1))
             AND (?2 IS NULL OR lower(p.protocol)=lower(?2))
         )
         SELECT c.chain, c.address
         FROM candidates c
         LEFT JOIN token_metadata m
           ON m.chain=c.chain AND m.address=c.address
         WHERE c.address IS NOT NULL
           AND c.address <> ''
           AND c.chain <> 'solana'
           AND m.decimals IS NULL
         ORDER BY c.chain, c.address",
    )?;
    let tokens = statement
        .query_map(params![chain_filter, protocol_filter], |row| {
            Ok(TokenRef {
                chain: row.get(0)?,
                address: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(tokens
        .into_iter()
        .filter(|token| !native_address(&token.address))
        .filter(|token| token.address.starts_with("0x") && token.address.len() == 42)
        .collect())
}

async fn fetch_erc20_decimals(
    client: Client,
    rpc_proxy: String,
    token: TokenRef,
) -> Option<(String, String, u32)> {
    let url = format!("{}/rpc/{}", rpc_proxy.trim_end_matches('/'), token.chain);
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_call",
        "params": [{"to": token.address, "data": "0x313ce567"}, "latest"]
    });
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Value>()
        .await
        .ok()?;
    let raw = response.get("result")?.as_str()?;
    let decimals = u64::from_str_radix(raw.trim_start_matches("0x"), 16).ok()?;
    let decimals = u32::try_from(decimals).ok()?.min(36);
    Some((token.chain, token.address, decimals))
}

async fn resolve_missing_decimals(
    client: &Client,
    db: &mut Connection,
    rpc_proxy: &str,
    workers: usize,
    chain_filter: Option<&str>,
    protocol_filter: Option<&str>,
    observed_at: i64,
) -> Result<usize> {
    let tokens = missing_evm_decimals(db, chain_filter, protocol_filter)?;
    if tokens.is_empty() {
        return Ok(0);
    }
    let mut tasks = JoinSet::new();
    let mut resolved = Vec::new();
    for token in tokens {
        tasks.spawn(fetch_erc20_decimals(
            client.clone(),
            rpc_proxy.to_string(),
            token,
        ));
        while tasks.len() >= workers {
            if let Some(Ok(Some(value))) = tasks.join_next().await {
                resolved.push(value);
            }
        }
    }
    while let Some(item) = tasks.join_next().await {
        if let Some(value) = item? {
            resolved.push(value);
        }
    }
    if resolved.is_empty() {
        return Ok(0);
    }
    let tx = db.transaction()?;
    for (chain, address, decimals) in &resolved {
        tx.execute(
            "INSERT INTO token_metadata(chain,address,symbol,name,decimals,status,source,checked_at,error,code_present,metadata_issue)
             VALUES(?1,?2,NULL,NULL,?3,'complete','rpc_erc20_decimals',?4,NULL,1,NULL)
             ON CONFLICT(chain,address) DO UPDATE SET
               decimals=excluded.decimals,
               status='complete',
               source='rpc_erc20_decimals',
               checked_at=excluded.checked_at,
               error=NULL,
               metadata_issue=NULL",
            params![chain, address, *decimals as i64, observed_at],
        )?;
    }
    tx.commit()?;
    Ok(resolved.len())
}

#[derive(Debug, Clone)]
struct SolanaVaultPool {
    chain: String,
    protocol: String,
    pool_address: String,
    token0_address: String,
    token1_address: String,
    vault0: String,
    vault1: String,
    decimals0: u32,
    decimals1: u32,
}

#[derive(Debug, Clone)]
struct EvmDirectPool {
    chain: String,
    protocol: String,
    pool_address: String,
    pool_type: String,
    token0_address: String,
    token1_address: String,
    decimals0: u32,
    decimals1: u32,
}

fn load_evm_direct_pools(
    db: &Connection,
    chain_filter: Option<&str>,
    protocol_filter: Option<&str>,
) -> Result<Vec<EvmDirectPool>> {
    let decimals = metadata_decimals(db)?;
    let mut statement = db.prepare(
        "SELECT p.chain, p.protocol, p.pool_address, p.pool_type,
                coalesce(s.token0_address,p.token_a_address),
                coalesce(s.token1_address,p.token_b_address)
         FROM pools p JOIN pool_rpc_state s ON s.pool_address=p.pool_address
         WHERE s.state_status='live' AND p.chain <> 'solana'
           AND p.pool_type IN ('constant_product_v2','concentrated_v3')
           AND NOT EXISTS (
             SELECT 1 FROM price.pool_price_history h
             WHERE h.chain=p.chain AND h.pool_address=p.pool_address
           )
           AND (?1 IS NULL OR lower(p.chain)=lower(?1))
           AND (?2 IS NULL OR lower(p.protocol)=lower(?2))",
    )?;
    let mut result = Vec::new();
    for row in statement.query_map(params![chain_filter, protocol_filter], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })? {
        let (chain, protocol, pool_address, pool_type, token0, token1) = row?;
        let (Some(token0_address), Some(token1_address)) = (token0, token1) else {
            continue;
        };
        let (Some(decimals0), Some(decimals1)) = (
            decimals.get(&(chain.clone(), normalize_address(&token0_address))),
            decimals.get(&(chain.clone(), normalize_address(&token1_address))),
        ) else {
            continue;
        };
        result.push(EvmDirectPool {
            chain,
            protocol,
            pool_address,
            pool_type,
            token0_address: normalize_address(&token0_address),
            token1_address: normalize_address(&token1_address),
            decimals0: *decimals0,
            decimals1: *decimals1,
        });
    }
    Ok(result)
}

fn abi_u128_word(data: &str, index: usize) -> Option<u128> {
    u128::from_str_radix(abi_word(data, index)?.trim_start_matches('0'), 16).ok()
}

fn abi_f64_word(data: &str, index: usize) -> Option<f64> {
    let word = abi_word(data, index)?;
    let mut value = 0.0;
    for byte in word.as_bytes() {
        let digit = match byte {
            b'0'..=b'9' => (byte - b'0') as f64,
            b'a'..=b'f' => (byte - b'a' + 10) as f64,
            b'A'..=b'F' => (byte - b'A' + 10) as f64,
            _ => return None,
        };
        value = value * 16.0 + digit;
    }
    value.is_finite().then_some(value)
}

async fn read_evm_direct_prices(
    client: &Client,
    rpc_proxy: &str,
    pools: Vec<EvmDirectPool>,
) -> Result<Vec<PoolRatio>> {
    let v2_selector = selector("getReserves()");
    let v3_selector = selector("slot0()");
    let mut result = Vec::new();
    // Keep batches conservative: public upstreams behind the local proxy
    // commonly reject a large heterogeneous eth_call batch with 502.
    // Some RPC gateways fail an entire batch when one pool is not the
    // expected implementation. These are fallback reads only, so isolate
    // each call and let the rest of the pools proceed.
    for chunk in pools.chunks(1) {
        let requests = chunk
            .iter()
            .enumerate()
            .map(|(index, pool)| serde_json::json!({
                "jsonrpc": "2.0",
                "id": 70_000 + index as i64,
                "method": "eth_call",
                "params": [{
                    "to": pool.pool_address,
                    "data": if pool.pool_type == "constant_product_v2" { &v2_selector } else { &v3_selector }
                }, "latest"]
            }))
            .collect::<Vec<_>>();
        let url = format!("{}/rpc/{}", rpc_proxy.trim_end_matches('/'), chunk[0].chain);
        let payload = match client
            .post(url)
            .header("content-type", "application/json")
            .json(&requests)
            .send()
            .await
            .and_then(|response| response.error_for_status())
        {
            Ok(response) => match response.json::<Value>().await {
                Ok(payload) => payload,
                Err(error) => {
                    eprintln!("direct EVM response decode failed: {error}");
                    continue;
                }
            },
            Err(error) => {
                eprintln!("direct EVM batch failed: {error}");
                continue;
            }
        };
        let mut responses = HashMap::new();
        if let Value::Array(items) = payload {
            for item in items {
                if let (Some(id), Some(raw)) = (
                    item.get("id").and_then(Value::as_i64),
                    item.get("result").and_then(Value::as_str),
                ) {
                    responses.insert(id, raw.to_string());
                }
            }
        }
        for (index, pool) in chunk.iter().enumerate() {
            let Some(raw) = responses.get(&(70_000 + index as i64)) else {
                continue;
            };
            let (raw0, raw1) = if pool.pool_type == "constant_product_v2" {
                let (Some(reserve0), Some(reserve1)) =
                    (abi_u128_word(raw, 0), abi_u128_word(raw, 1))
                else {
                    continue;
                };
                (reserve0.to_string(), reserve1.to_string())
            } else {
                let Some(sqrt_price) = abi_f64_word(raw, 0) else {
                    continue;
                };
                if sqrt_price == 0.0 {
                    continue;
                }
                // Feed the same canonical decoder used for persisted state;
                // this keeps direct and snapshot reads numerically identical.
                let ratio = raw_ratio(
                    &pool.pool_type,
                    None,
                    None,
                    Some(&sqrt_price.to_string()),
                    None,
                    pool.decimals0,
                    pool.decimals1,
                );
                let Some(ratio) = ratio else {
                    continue;
                };
                let token1_per_token0 = 1.0 / ratio;
                if token1_per_token0.is_finite() && token1_per_token0 > 0.0 {
                    result.push(PoolRatio {
                        chain: pool.chain.clone(),
                        protocol: pool.protocol.clone(),
                        pool_address: pool.pool_address.clone(),
                        token0_address: pool.token0_address.clone(),
                        token1_address: pool.token1_address.clone(),
                        token1_per_token0,
                        source: "rpc_pool_direct",
                    });
                }
                continue;
            };
            let Some(ratio) = raw_ratio(
                &pool.pool_type,
                Some(&raw0),
                Some(&raw1),
                None,
                None,
                pool.decimals0,
                pool.decimals1,
            ) else {
                continue;
            };
            let token1_per_token0 = 1.0 / ratio;
            if token1_per_token0.is_finite() && token1_per_token0 > 0.0 {
                result.push(PoolRatio {
                    chain: pool.chain.clone(),
                    protocol: pool.protocol.clone(),
                    pool_address: pool.pool_address.clone(),
                    token0_address: pool.token0_address.clone(),
                    token1_address: pool.token1_address.clone(),
                    token1_per_token0,
                    source: "rpc_pool_direct",
                });
            }
        }
    }
    Ok(result)
}

#[derive(Debug, Clone)]
struct FluidPool {
    chain: String,
    protocol: String,
    pool_address: String,
    token0_address: String,
    token1_address: String,
}

fn load_fluid_pools(
    db: &Connection,
    chain_filter: Option<&str>,
    protocol_filter: Option<&str>,
) -> Result<Vec<FluidPool>> {
    let mut statement = db.prepare(
        "SELECT p.chain,p.protocol,p.pool_address,s.token0_address,s.token1_address
         FROM pools p JOIN pool_rpc_state s ON s.pool_address=p.pool_address
         WHERE s.state_status='live' AND p.pool_type='fluid_dex'
           AND (?1 IS NULL OR lower(p.chain)=lower(?1))
           AND (?2 IS NULL OR lower(p.protocol)=lower(?2))",
    )?;
    let mut result = Vec::new();
    for row in statement.query_map(params![chain_filter, protocol_filter], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })? {
        let (chain, protocol, pool_address, token0, token1) = row?;
        let (Some(token0_address), Some(token1_address)) = (token0, token1) else {
            continue;
        };
        result.push(FluidPool {
            chain,
            protocol,
            pool_address,
            token0_address: normalize_address(&token0_address),
            token1_address: normalize_address(&token1_address),
        });
    }
    Ok(result)
}

async fn read_fluid_prices(
    client: &Client,
    rpc_proxy: &str,
    pools: Vec<FluidPool>,
) -> Result<Vec<PoolRatio>> {
    let call_data = selector("getPricesAndExchangePrices()");
    // Fluid deliberately returns its price through a custom-error revert:
    // FluidDexPricesAndExchangeRates(PricesAndExchangePrice). The first ABI
    // word after the four-byte error selector is lastStoredPrice, already in
    // 1e27 decimal units and quoting token1/token0.
    let mut result = Vec::new();
    for pool in pools {
        let url = format!("{}/rpc/{}", rpc_proxy.trim_end_matches('/'), pool.chain);
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 71_000,
            "method": "eth_call",
            "params": [{"to": pool.pool_address, "data": call_data}, "latest"]
        });
        let response = match client
            .post(&url)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
        {
            Ok(response) => response.json::<Value>().await?,
            Err(error) => {
                eprintln!("Fluid pool read failed on {}: {error}", pool.chain);
                continue;
            }
        };
        let Some(data) = response
            .get("error")
            .and_then(|error| error.get("data"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let encoded_args = data.strip_prefix("0x").and_then(|value| value.get(8..));
        let Some(price_word) = encoded_args.and_then(|value| abi_f64_word(value, 0)) else {
            continue;
        };
        let token1_per_token0 = 1.0 / (price_word / 1e27);
        if token1_per_token0.is_finite() && token1_per_token0 > 0.0 {
            result.push(PoolRatio {
                chain: pool.chain,
                protocol: pool.protocol,
                pool_address: pool.pool_address,
                token0_address: pool.token0_address,
                token1_address: pool.token1_address,
                token1_per_token0,
                source: "rpc_pool_direct",
            });
        }
    }
    Ok(result)
}

fn load_solana_vault_pools(
    db: &Connection,
    chain_filter: Option<&str>,
    protocol_filter: Option<&str>,
) -> Result<Vec<SolanaVaultPool>> {
    let mut statement = db.prepare(
        "SELECT p.chain, p.protocol, p.pool_address, s.state_json
         FROM pools p JOIN pool_rpc_state s ON s.pool_address=p.pool_address
         WHERE s.state_status='live' AND p.chain='solana'
           AND p.pool_type='solana_standard'
           AND (?1 IS NULL OR lower(p.chain)=lower(?1))
           AND (?2 IS NULL OR lower(p.protocol)=lower(?2))",
    )?;
    let mut result = Vec::new();
    for row in statement.query_map(params![chain_filter, protocol_filter], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })? {
        let (chain, protocol, pool_address, state_json) = row?;
        let Some(state) = state_json
            .as_deref()
            .and_then(|value| serde_json::from_str::<Value>(value).ok())
        else {
            continue;
        };
        let text = |key: &str| state.get(key).and_then(Value::as_str).map(str::to_string);
        let number = |key: &str| {
            state
                .get(key)
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value <= 36)
        };
        let (Some(token0_address), Some(token1_address), Some(vault0), Some(vault1)) = (
            text("token0_address"),
            text("token1_address"),
            text("token0_vault"),
            text("token1_vault"),
        ) else {
            continue;
        };
        let (Some(decimals0), Some(decimals1)) =
            (number("mint_decimals_0"), number("mint_decimals_1"))
        else {
            continue;
        };
        result.push(SolanaVaultPool {
            chain,
            protocol,
            pool_address,
            token0_address,
            token1_address,
            vault0,
            vault1,
            decimals0,
            decimals1,
        });
    }
    Ok(result)
}

fn solana_account_amount(response: &Value) -> Option<String> {
    let encoded = response
        .get("result")?
        .get("value")?
        .get("data")?
        .as_array()?
        .first()?
        .as_str()?;
    let data = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let bytes: [u8; 8] = data.get(64..72)?.try_into().ok()?;
    Some(u64::from_le_bytes(bytes).to_string())
}

async fn read_solana_vault_prices(
    client: &Client,
    rpc_proxy: &str,
    pools: Vec<SolanaVaultPool>,
) -> Result<Vec<PoolRatio>> {
    let mut result = Vec::new();
    for chunk in pools.chunks(40) {
        let mut requests = Vec::with_capacity(chunk.len() * 2);
        for (index, pool) in chunk.iter().enumerate() {
            let id0 = 50_000 + (index as i64) * 2;
            requests.push(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id0,
                "method": "getAccountInfo",
                "params": [pool.vault0, {"encoding":"base64","commitment":"confirmed"}]
            }));
            requests.push(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id0 + 1,
                "method": "getAccountInfo",
                "params": [pool.vault1, {"encoding":"base64","commitment":"confirmed"}]
            }));
        }
        let url = format!("{}/rpc/solana", rpc_proxy.trim_end_matches('/'));
        let response = client
            .post(url)
            .header("content-type", "application/json")
            .json(&requests)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let mut amounts = HashMap::new();
        if let Value::Array(items) = response {
            for item in items {
                if let Some(id) = item.get("id").and_then(Value::as_i64) {
                    if let Some(amount) = solana_account_amount(&item) {
                        amounts.insert(id, amount);
                    }
                }
            }
        }
        for (index, pool) in chunk.iter().enumerate() {
            let id0 = 50_000 + (index as i64) * 2;
            let (Some(raw0), Some(raw1)) = (amounts.get(&id0), amounts.get(&(id0 + 1))) else {
                continue;
            };
            let (Some(raw0), Some(raw1)) = (raw0.parse::<f64>().ok(), raw1.parse::<f64>().ok())
            else {
                continue;
            };
            if raw0 <= 0.0 || raw1 <= 0.0 {
                continue;
            }
            let raw_ratio =
                (raw1 / raw0) * 10f64.powi(pool.decimals0 as i32 - pool.decimals1 as i32);
            let token1_per_token0 = 1.0 / raw_ratio;
            if !token1_per_token0.is_finite() || token1_per_token0 <= 0.0 {
                continue;
            }
            result.push(PoolRatio {
                chain: pool.chain.clone(),
                protocol: pool.protocol.clone(),
                pool_address: pool.pool_address.clone(),
                token0_address: pool.token0_address.clone(),
                token1_address: pool.token1_address.clone(),
                token1_per_token0,
                source: "rpc_pool_direct",
            });
        }
    }
    Ok(result)
}

fn merge_pool_ratios(base: &mut Vec<PoolRatio>, direct: Vec<PoolRatio>) {
    let mut by_pool = base
        .drain(..)
        .map(|ratio| ((ratio.chain.clone(), ratio.pool_address.clone()), ratio))
        .collect::<HashMap<_, _>>();
    for ratio in direct {
        by_pool.insert((ratio.chain.clone(), ratio.pool_address.clone()), ratio);
    }
    base.extend(by_pool.into_values());
}

#[derive(Debug, Clone)]
struct BalancerPool {
    chain: String,
    protocol: String,
    pool_address: String,
    vault: String,
    pool_id: String,
    token0_address: String,
    token1_address: String,
    decimals0: u32,
    decimals1: u32,
    weight0: Option<f64>,
    weight1: Option<f64>,
    stable_variant: bool,
}

fn selector(signature: &str) -> String {
    let mut hasher = Keccak::v256();
    let mut output = [0u8; 32];
    hasher.update(signature.as_bytes());
    hasher.finalize(&mut output);
    format!(
        "0x{:02x}{:02x}{:02x}{:02x}",
        output[0], output[1], output[2], output[3]
    )
}

fn abi_word(data: &str, index: usize) -> Option<&str> {
    let data = data.strip_prefix("0x").unwrap_or(data);
    let start = index.checked_mul(64)?;
    data.get(start..start + 64)
}

fn abi_usize(data: &str, index: usize) -> Option<usize> {
    usize::from_str_radix(abi_word(data, index)?.trim_start_matches('0'), 16).ok()
}

fn parse_balancer_tokens(
    data: &str,
    token0_address: &str,
    token1_address: &str,
) -> Option<(String, String)> {
    let token_offset = abi_usize(data, 0)? / 32;
    let balance_offset = abi_usize(data, 1)? / 32;
    let token_count = abi_usize(data, token_offset)?;
    let balance_count = abi_usize(data, balance_offset)?;
    let mut token_indexes = HashMap::new();
    for index in 0..token_count {
        let word = abi_word(data, token_offset + index + 1)?;
        let address = normalize_address(&format!("0x{}", &word[24..]));
        token_indexes.insert(address, index);
    }
    let index0 = *token_indexes.get(&normalize_address(token0_address))?;
    let index1 = *token_indexes.get(&normalize_address(token1_address))?;
    if index0 >= balance_count || index1 >= balance_count {
        return None;
    }
    let balance0 = u128::from_str_radix(abi_word(data, balance_offset + index0 + 1)?, 16).ok()?;
    let balance1 = u128::from_str_radix(abi_word(data, balance_offset + index1 + 1)?, 16).ok()?;
    Some((balance0.to_string(), balance1.to_string()))
}

fn load_balancer_pools(
    db: &Connection,
    chain_filter: Option<&str>,
    protocol_filter: Option<&str>,
) -> Result<Vec<BalancerPool>> {
    let decimals = metadata_decimals(db)?;
    let mut statement = db.prepare(
        "SELECT p.chain, p.protocol, p.pool_address,
                coalesce(s.token0_address,p.token_a_address),
                coalesce(s.token1_address,p.token_b_address), s.state_json
         FROM pools p JOIN pool_rpc_state s ON s.pool_address=p.pool_address
         WHERE s.state_status='live' AND p.pool_type='weighted'
           AND (?1 IS NULL OR lower(p.chain)=lower(?1))
           AND (?2 IS NULL OR lower(p.protocol)=lower(?2))",
    )?;
    let mut result = Vec::new();
    for row in statement.query_map(params![chain_filter, protocol_filter], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })? {
        let (chain, protocol, pool_address, token0, token1, state_json) = row?;
        let (Some(token0_address), Some(token1_address), Some(state_json)) =
            (token0, token1, state_json)
        else {
            continue;
        };
        let Some(state) = serde_json::from_str::<Value>(&state_json).ok() else {
            continue;
        };
        let (Some(vault), Some(pool_id)) = (
            state.get("vault").and_then(Value::as_str),
            state.get("pool_id").and_then(Value::as_str),
        ) else {
            continue;
        };
        let (Some(decimals0), Some(decimals1)) = (
            decimals.get(&(chain.clone(), normalize_address(&token0_address))),
            decimals.get(&(chain.clone(), normalize_address(&token1_address))),
        ) else {
            continue;
        };
        let weights = state
            .get("normalized_weights_raw")
            .and_then(Value::as_array);
        let weight0 = weights
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<f64>().ok())
            .map(|value| value / 1e18);
        let weight1 = weights
            .and_then(|values| values.get(1))
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<f64>().ok())
            .map(|value| value / 1e18);
        let stable_variant = weight0.is_none() || weight1.is_none();
        if !stable_variant
            && (weight0.unwrap_or_default() <= 0.0 || weight1.unwrap_or_default() <= 0.0)
        {
            continue;
        }
        result.push(BalancerPool {
            chain,
            protocol,
            pool_address,
            vault: vault.to_string(),
            pool_id: pool_id.to_string(),
            token0_address,
            token1_address,
            decimals0: *decimals0,
            decimals1: *decimals1,
            weight0,
            weight1,
            stable_variant,
        });
    }
    Ok(result)
}

fn abi_word_hex(value: &str) -> String {
    let value = value.trim_start_matches("0x");
    format!("{value:0>64}")
}

fn encode_balancer_query_batch_swap(pool: &BalancerPool, amount_in: u128) -> String {
    // queryBatchSwap(uint8,(bytes32,uint256,uint256,uint256,bytes)[],address[],
    // (address,bool,address,bool)). The offsets and tuple layout are the
    // canonical ABI encoding for one exact-in swap step.
    let word = |value: &str| abi_word_hex(value);
    let zero_address = "0x0000000000000000000000000000000000000000";
    let head = [
        word("0x0"),
        word("0xe0"),
        word("0x1e0"),
        word(zero_address),
        word("0x0"),
        word(zero_address),
        word("0x0"),
    ];
    let swap = [
        word("0x1"),
        word("0x20"),
        word(&pool.pool_id),
        word("0x0"),
        word("0x1"),
        word(&format!("0x{amount_in:x}")),
        word("0xa0"),
        word("0x0"),
    ];
    let assets = [
        word("0x2"),
        word(&pool.token0_address),
        word(&pool.token1_address),
    ];
    format!(
        "{}{}",
        selector("queryBatchSwap(uint8,(bytes32,uint256,uint256,uint256,bytes)[],address[],(address,bool,address,bool))"),
        head.into_iter()
            .chain(swap)
            .chain(assets)
            .collect::<String>()
    )
}

fn abi_signed_magnitude_u128(data: &str, index: usize) -> Option<u128> {
    let word = abi_word(data, index)?;
    let negative = word.as_bytes().first().is_some_and(|value| *value >= b'8');
    let low = u128::from_str_radix(&word[32..], 16).ok()?;
    if negative {
        Some((!low).wrapping_add(1))
    } else if word[..32].chars().all(|value| value == '0') {
        Some(low)
    } else {
        None
    }
}

fn parse_balancer_query_output(data: &str) -> Option<u128> {
    let length = abi_usize(data, 1)?;
    (length >= 2).then(|| abi_signed_magnitude_u128(data, 3))?
}

async fn read_balancer_prices(
    client: &Client,
    rpc_proxy: &str,
    pools: Vec<BalancerPool>,
) -> Result<Vec<PoolRatio>> {
    let call_selector = selector("getPoolTokens(bytes32)");
    let query_contract = "0xe39b5e3b6d74016b2f6a9673d7d7493b6df549d5";
    let mut result = Vec::new();
    let mut pools_by_chain: HashMap<String, Vec<BalancerPool>> = HashMap::new();
    for pool in pools {
        pools_by_chain
            .entry(pool.chain.clone())
            .or_default()
            .push(pool);
    }
    for chain_pools in pools_by_chain.into_values() {
        for chunk in chain_pools.chunks(40) {
            let requests = chunk
                .iter()
                .enumerate()
                .map(|(index, pool)| {
                    let amount_in = 10u128.pow(pool.decimals0.saturating_sub(6));
                    let (to, data) = if pool.stable_variant {
                        (
                            query_contract,
                            encode_balancer_query_batch_swap(pool, amount_in),
                        )
                    } else {
                        (
                            pool.vault.as_str(),
                            format!("{}{}", call_selector, pool.pool_id.trim_start_matches("0x")),
                        )
                    };
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 60_000 + index as i64,
                        "method": "eth_call",
                        "params": [{"to": to, "data": data}, "latest"]
                    })
                })
                .collect::<Vec<_>>();
            let mut responses = HashMap::new();
            let url = format!("{}/rpc/{}", rpc_proxy.trim_end_matches('/'), chunk[0].chain);
            let payload = client
                .post(url)
                .header("content-type", "application/json")
                .json(&requests)
                .send()
                .await?
                .error_for_status()?
                .json::<Value>()
                .await?;
            if let Value::Array(items) = payload {
                for item in items {
                    if let (Some(id), Some(raw)) = (
                        item.get("id").and_then(Value::as_i64),
                        item.get("result").and_then(Value::as_str),
                    ) {
                        responses.insert(id, raw.to_string());
                    }
                }
            }
            for (index, pool) in chunk.iter().enumerate() {
                let Some(raw) = responses.get(&(60_000 + index as i64)) else {
                    continue;
                };
                if pool.stable_variant {
                    let amount_in = 10u128.pow(pool.decimals0.saturating_sub(6));
                    let Some(amount_out) = parse_balancer_query_output(raw) else {
                        continue;
                    };
                    if amount_out == 0 {
                        continue;
                    }
                    let raw_ratio = (amount_out as f64 / amount_in as f64)
                        * 10f64.powi(pool.decimals0 as i32 - pool.decimals1 as i32);
                    let token1_per_token0 = 1.0 / raw_ratio;
                    if token1_per_token0.is_finite() && token1_per_token0 > 0.0 {
                        result.push(PoolRatio {
                            chain: pool.chain.clone(),
                            protocol: pool.protocol.clone(),
                            pool_address: pool.pool_address.clone(),
                            token0_address: pool.token0_address.clone(),
                            token1_address: pool.token1_address.clone(),
                            token1_per_token0,
                            source: "rpc_pool_direct",
                        });
                    }
                    continue;
                }
                let Some((balance0, balance1)) =
                    parse_balancer_tokens(raw, &pool.token0_address, &pool.token1_address)
                else {
                    continue;
                };
                let (Some(balance0), Some(balance1), Some(weight0), Some(weight1)) = (
                    balance0.parse::<f64>().ok(),
                    balance1.parse::<f64>().ok(),
                    pool.weight0,
                    pool.weight1,
                ) else {
                    continue;
                };
                if balance0 <= 0.0 || balance1 <= 0.0 || weight0 <= 0.0 || weight1 <= 0.0 {
                    continue;
                }
                let raw_ratio = (balance1 * weight0 / (balance0 * weight1))
                    * 10f64.powi(pool.decimals0 as i32 - pool.decimals1 as i32);
                let token1_per_token0 = 1.0 / raw_ratio;
                if token1_per_token0.is_finite() && token1_per_token0 > 0.0 {
                    result.push(PoolRatio {
                        chain: pool.chain.clone(),
                        protocol: pool.protocol.clone(),
                        pool_address: pool.pool_address.clone(),
                        token0_address: pool.token0_address.clone(),
                        token1_address: pool.token1_address.clone(),
                        token1_per_token0,
                        source: "rpc_pool_direct",
                    });
                }
            }
        }
    }
    Ok(result)
}

fn raw_ratio(
    pool_type: &str,
    reserve0: Option<&str>,
    reserve1: Option<&str>,
    sqrt_price: Option<&str>,
    state_json: Option<&str>,
    decimals0: u32,
    decimals1: u32,
) -> Option<f64> {
    let state = state_json.and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    let decimal_scale = 10f64.powi(decimals0 as i32 - decimals1 as i32);
    let state_sqrt = state.as_ref().and_then(|value| {
        ["sqrt_price_x96", "sqrt_price_x64", "sqrt_price"]
            .iter()
            .find_map(|key| value.get(*key).and_then(Value::as_str))
    });
    let json_number = |key: &str| {
        state.as_ref().and_then(|value| {
            value.get(key).and_then(|item| {
                item.as_f64()
                    .or_else(|| item.as_str().and_then(|text| text.parse::<f64>().ok()))
            })
        })
    };
    // Ekubo stores sqrt(price) in its compact 96-bit dynamic format. The top
    // two bits select a 64.30/32.62/0.94/0.126 representation; toFixed() in
    // the protocol expands it to Q64.128 before squaring.
    let ekubo_ratio = |raw: f64| {
        let region = (raw / 2f64.powi(94)).floor();
        let mantissa = raw - region * 2f64.powi(94);
        let shift = 2 + 32 * region as i32;
        (mantissa * mantissa * 2f64.powi(2 * shift - 256))
            .is_finite()
            .then_some(mantissa * mantissa * 2f64.powi(2 * shift - 256))
    };
    // Liquidity Book's total reserves are not its marginal price.  The
    // active bin is the spot-price source.  Trader Joe/LFJ encodes the real
    // bin id as activeId - 2^23; older LFJ rows without binStep are V1-style
    // reserve pairs and intentionally fall back to reserves below.
    let active_bin_ratio = (pool_type == "liquidity_book")
        .then(|| {
            let active_id = json_number("active_id")?;
            let bin_step = json_number("bin_step")?;
            let real_id = active_id - 8_388_608.0;
            Some((1.0 + bin_step / 10_000.0).powf(real_id) * decimal_scale)
        })
        .flatten();
    let ratio = if let Some(active_bin_ratio) = active_bin_ratio {
        active_bin_ratio
    } else if let Some(dynamic_sqrt_ratio) = json_number("sqrt_ratio_dynamic_raw") {
        ekubo_ratio(dynamic_sqrt_ratio)? * decimal_scale
    } else if let (Some(left), Some(right)) = (reserve0, reserve1) {
        let left = left.parse::<f64>().ok()?;
        let right = right.parse::<f64>().ok()?;
        if left <= 0.0 || right <= 0.0 {
            return None;
        }
        (right / left) * decimal_scale
    } else if let Some(sqrt) = sqrt_price.or(state_sqrt) {
        let sqrt = sqrt.parse::<f64>().ok()?;
        let denominator = if pool_type.starts_with("solana_") {
            2f64.powi(128)
        } else {
            2f64.powi(192)
        };
        (sqrt * sqrt / denominator) * decimal_scale
    } else {
        state.as_ref()?;
        let active_id = json_number("active_id")?;
        let bin_step = json_number("bin_step")?;
        (1.0 + bin_step / 10_000.0).powf(active_id) * decimal_scale
    };
    ratio
        .is_finite()
        .then_some(ratio)
        // Very small tokens can legitimately produce a human-unit ratio well
        // above 1e30 (for example, a whole WBNB against a token with 0
        // decimals and tiny reserves). The reciprocal is still a valid pool
        // price, so do not drop it with an arbitrary protocol-independent
        // ceiling.
        .filter(|value| *value > 0.0)
}

fn state_decimals(state: Option<&Value>, index: usize) -> Option<u32> {
    let keys = match index {
        0 => ["mint_decimals_0", "mint_0_decimals"],
        _ => ["mint_decimals_1", "mint_1_decimals"],
    };
    keys.into_iter().find_map(|key| {
        state
            .and_then(|value| value.get(key))
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
    })
}

fn load_pool_groups(
    db: &Connection,
    chain_filter: Option<&str>,
    protocol_filter: Option<&str>,
) -> Result<Vec<PoolGroup>> {
    let mut statement = db.prepare(
        "SELECT p.chain, p.protocol, count(*)
         FROM pools p JOIN pool_rpc_state s ON s.pool_address=p.pool_address
         WHERE s.state_status='live'
           AND (?1 IS NULL OR lower(p.chain)=lower(?1))
           AND (?2 IS NULL OR lower(p.protocol)=lower(?2))
         GROUP BY p.chain, p.protocol
         ORDER BY p.chain, p.protocol",
    )?;
    let groups = statement
        .query_map(params![chain_filter, protocol_filter], |row| {
            Ok(PoolGroup {
                chain: row.get(0)?,
                protocol: row.get(1)?,
                live_pools: row.get::<_, i64>(2)?.max(0) as usize,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(groups)
}

#[derive(Debug, Clone)]
struct PoolObservation {
    chain: String,
    pool_address: String,
    token0_address: String,
    token1_address: String,
    unavailable_reason: String,
}

fn unavailable_pool_reason(
    chain: &str,
    pool_type: &str,
    reserve0: Option<&str>,
    reserve1: Option<&str>,
    active_liquidity: Option<&str>,
    state_json: Option<&str>,
) -> String {
    if reserve0.is_some_and(|value| value == "0") || reserve1.is_some_and(|value| value == "0") {
        return "non_positive_reserves".into();
    }
    if active_liquidity.is_some_and(|value| value == "0") {
        return "zero_active_liquidity".into();
    }
    if chain == "solana" && pool_type == "solana_standard" {
        return "missing_or_zero_vault_balance".into();
    }
    if pool_type == "weighted"
        && state_json
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|state| state.get("stable_variant").and_then(Value::as_bool))
            .is_some_and(|stable| stable)
    {
        return "stable_pool_quote_unavailable".into();
    }
    "no_valid_pool_ratio".into()
}

fn load_pool_observations(
    db: &Connection,
    chain_filter: Option<&str>,
    protocol_filter: Option<&str>,
) -> Result<Vec<PoolObservation>> {
    let mut statement = db.prepare(
        "SELECT p.chain, p.pool_address,
                coalesce(s.token0_address,p.token_a_address,''),
                coalesce(s.token1_address,p.token_b_address,''),
                p.pool_type, coalesce(s.state_status,'no_rpc_state'),
                s.reserve0_raw, s.reserve1_raw, s.active_liquidity_raw,
                s.state_json
         FROM pools p LEFT JOIN pool_rpc_state s ON s.pool_address=p.pool_address
         WHERE (?1 IS NULL OR lower(p.chain)=lower(?1))
           AND (?2 IS NULL OR lower(p.protocol)=lower(?2))",
    )?;
    let mut result = Vec::new();
    for row in statement.query_map(params![chain_filter, protocol_filter], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, Option<String>>(9)?,
        ))
    })? {
        let (
            chain,
            pool_address,
            token0_address,
            token1_address,
            pool_type,
            state_status,
            reserve0,
            reserve1,
            active_liquidity,
            state_json,
        ) = row?;
        let unavailable_reason = if state_status != "live" {
            format!("rpc_state_{state_status}")
        } else {
            unavailable_pool_reason(
                &chain,
                &pool_type,
                reserve0.as_deref(),
                reserve1.as_deref(),
                active_liquidity.as_deref(),
                state_json.as_deref(),
            )
        };
        result.push(PoolObservation {
            chain,
            pool_address,
            token0_address,
            token1_address,
            unavailable_reason,
        });
    }
    Ok(result)
}

fn rpc_edges(
    db: &Connection,
    chain_filter: Option<&str>,
    protocol_filter: Option<&str>,
) -> Result<(Vec<Edge>, HashMap<(String, String), f64>, Vec<PoolRatio>)> {
    let decimals = metadata_decimals(db)?;
    let mut prices = HashMap::new();
    let mut edges = Vec::new();
    let mut pool_ratios = Vec::new();
    let mut statement = db.prepare(
        "SELECT p.chain, p.pool_address, p.pool_type, p.token_a_address, p.token_b_address,
                p.token_a_symbol, p.token_b_symbol,
                s.token0_address, s.token1_address, s.reserve0_raw, s.reserve1_raw,
                s.sqrt_price_x96, s.active_liquidity_raw, s.state_json, p.protocol
         FROM pools p JOIN pool_rpc_state s ON s.pool_address=p.pool_address
         WHERE s.state_status='live'
           AND (?1 IS NULL OR lower(p.chain)=lower(?1))
           AND (?2 IS NULL OR lower(p.protocol)=lower(?2))",
    )?;
    for row in statement.query_map(params![chain_filter, protocol_filter], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, Option<String>>(9)?,
            row.get::<_, Option<String>>(10)?,
            row.get::<_, Option<String>>(11)?,
            row.get::<_, Option<String>>(12)?,
            row.get::<_, Option<String>>(13)?,
            row.get::<_, String>(14)?,
        ))
    })? {
        let (
            chain,
            pool_address,
            pool_type,
            pool_a,
            pool_b,
            symbol_a,
            symbol_b,
            token0,
            token1,
            reserve0,
            reserve1,
            sqrt,
            active_liquidity,
            state,
            protocol,
        ) = row?;
        // Some protocols (notably Uniswap V4) keep the pool key and price in
        // the RPC state but do not repeat the two assets in that state row.
        // The catalog is the authoritative fallback for those pools.
        let (Some(token0), Some(token1)) = (token0.or(pool_a.clone()), token1.or(pool_b.clone()))
        else {
            continue;
        };
        // Concentrated pools can retain a boundary sqrt price after all
        // liquidity has been removed. That value is valid ABI data, but it
        // is not a tradable quote and must not become a token-price edge.
        if active_liquidity
            .as_deref()
            .and_then(|value| value.parse::<u128>().ok())
            == Some(0)
        {
            continue;
        }
        let key0 = (chain.clone(), normalize_address(&token0));
        let key1 = (chain.clone(), normalize_address(&token1));
        let state_value = state
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
        let decimals0 = decimals
            .get(&key0)
            .copied()
            .or_else(|| state_decimals(state_value.as_ref(), 0))
            .or_else(|| native_address(&key0.1).then_some(18));
        let decimals1 = decimals
            .get(&key1)
            .copied()
            .or_else(|| state_decimals(state_value.as_ref(), 1))
            .or_else(|| native_address(&key1.1).then_some(18));
        let (Some(decimals0), Some(decimals1)) = (decimals0, decimals1) else {
            continue;
        };
        let Some(ratio) = raw_ratio(
            &pool_type,
            reserve0.as_deref(),
            reserve1.as_deref(),
            sqrt.as_deref(),
            state.as_deref(),
            decimals0,
            decimals1,
        ) else {
            continue;
        };
        let token1_per_token0 = 1.0 / ratio;
        if ratio > 0.0
            && ratio.is_finite()
            && token1_per_token0.is_finite()
            && token1_per_token0 > 0.0
        {
            pool_ratios.push(PoolRatio {
                chain: chain.clone(),
                protocol,
                pool_address,
                token0_address: key0.1.clone(),
                token1_address: key1.1.clone(),
                token1_per_token0,
                source: "rpc_pool_state",
            });
            // `raw_ratio` is the human token1/token0 quote. For USD-price
            // propagation the edge must carry price(right) / price(left),
            // which is its inverse. Keeping this direction explicit avoids
            // inverting pools whose stablecoin happens to be token0.
            // Keep USD propagation bounded. The pool-local ratio itself is
            // still valid when a token is extremely cheap or expensive; it
            // must not be discarded just because it is unsafe as a global USD
            // graph edge.
            if token1_per_token0 > 1e-10 && token1_per_token0 < 1e6 {
                edges.push(Edge {
                    chain: chain.clone(),
                    left: key0.1.clone(),
                    right: key1.1.clone(),
                    right_per_left: token1_per_token0,
                });
                edges.push(Edge {
                    chain: chain.clone(),
                    left: key1.1.clone(),
                    right: key0.1.clone(),
                    right_per_left: ratio,
                });
            }
        }
        let symbol0 =
            if pool_a.as_deref().map(normalize_address).as_deref() == Some(key0.1.as_str()) {
                symbol_a.as_deref().unwrap_or_default()
            } else if pool_b.as_deref().map(normalize_address).as_deref() == Some(key0.1.as_str()) {
                symbol_b.as_deref().unwrap_or_default()
            } else {
                ""
            };
        let symbol1 =
            if pool_a.as_deref().map(normalize_address).as_deref() == Some(key1.1.as_str()) {
                symbol_a.as_deref().unwrap_or_default()
            } else if pool_b.as_deref().map(normalize_address).as_deref() == Some(key1.1.as_str()) {
                symbol_b.as_deref().unwrap_or_default()
            } else {
                ""
            };
        if stable_symbols(symbol0) {
            prices.entry(key0.clone()).or_insert(1.0);
        }
        if stable_symbols(symbol1) {
            prices.entry(key1.clone()).or_insert(1.0);
        }
    }
    Ok((edges, prices, pool_ratios))
}

fn propagate_rpc_prices(
    edges: Vec<Edge>,
    mut prices: HashMap<(String, String), f64>,
) -> HashMap<(String, String), f64> {
    let mut adjacency: HashMap<(String, String), Vec<Edge>> = HashMap::new();
    for edge in edges {
        adjacency
            .entry((edge.chain.clone(), edge.left.clone()))
            .or_default()
            .push(edge);
    }
    let mut queue = VecDeque::from_iter(prices.keys().cloned());
    while let Some(node) = queue.pop_front() {
        let Some(base_price) = prices.get(&node).copied() else {
            continue;
        };
        for edge in adjacency.get(&node).into_iter().flatten() {
            let key = (node.0.clone(), edge.right.clone());
            let candidate = base_price * edge.right_per_left;
            if prices.contains_key(&key)
                || !candidate.is_finite()
                || candidate <= 0.0
                || candidate > 1e6
            {
                continue;
            }
            prices.insert(key.clone(), candidate);
            queue.push_back(key);
        }
    }
    prices
}

fn latest_history_seeds(db: &Connection) -> Result<HashMap<(String, String), f64>> {
    let mut seeds = HashMap::new();
    let mut statement = db.prepare(
        "SELECT h.chain, h.address, h.price_usd
         FROM price.token_price_history h
         JOIN (
           SELECT chain, address, max(timestamp) AS timestamp
           FROM price.token_price_history
           GROUP BY chain, address
         ) latest
           ON latest.chain=h.chain AND latest.address=h.address
          AND latest.timestamp=h.timestamp
         WHERE h.price_usd > 0 AND h.price_usd < 1e6",
    )?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
        ))
    })? {
        let (chain, address, price) = row?;
        if price.is_finite() && price > 0.0 {
            seeds.insert((chain, normalize_address(&address)), price);
        }
    }
    Ok(seeds)
}

fn catalog_price_seeds(db: &Connection) -> Result<HashMap<(String, String), f64>> {
    let mut seeds = HashMap::new();
    let mut statement = db.prepare(
        "SELECT p.chain, s.token0_address, p.price_usd
         FROM pools p
         JOIN pool_rpc_state s ON s.pool_address=p.pool_address
         WHERE s.state_status='live'
           AND s.token0_address IS NOT NULL
           AND p.price_usd IS NOT NULL
           AND p.price_usd > 0
           AND p.price_usd < 1e6
           AND (p.asset_symbol IS NULL OR p.asset_symbol='' OR p.asset_symbol=p.token_a_symbol)",
    )?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
        ))
    })? {
        let (chain, address, price) = row?;
        if price.is_finite() && price > 0.0 {
            seeds.insert((chain, normalize_address(&address)), price);
        }
    }
    Ok(seeds)
}

fn write_prices(
    db: &mut Connection,
    prices: &HashMap<(String, String), Vec<(i64, f64)>>,
    fallback: &HashMap<(String, String), f64>,
    catalog_fallback: &HashMap<(String, String), f64>,
    observed_at: i64,
    retention_days: u64,
) -> Result<usize> {
    let tx = db.transaction()?;
    let mut written = 0;
    for ((chain, address), points) in prices {
        for (timestamp, price) in points {
            written += tx.execute(
                "INSERT OR REPLACE INTO price.token_price_history(chain,address,timestamp,price_usd,source,observed_at) VALUES(?1,?2,?3,?4,'defillama',?5)",
                params![chain, address, timestamp, price, observed_at],
            )?;
        }
    }
    for ((chain, address), price) in fallback {
        // Keep one RPC observation per hour. INSERT OR IGNORE also prevents a
        // fallback point from replacing a DefiLlama point at the same exact
        // timestamp, while preserving older RPC observations for volatility.
        let sample_timestamp = observed_at.div_euclid(RPC_SAMPLE_SECS) * RPC_SAMPLE_SECS;
        written += tx.execute(
            "INSERT INTO price.token_price_history(chain,address,timestamp,price_usd,source,observed_at) VALUES(?1,?2,?3,?4,'rpc_pool_state',?5)
             ON CONFLICT(chain,address,timestamp) DO UPDATE SET
               price_usd=excluded.price_usd, source=excluded.source, observed_at=excluded.observed_at
             WHERE price.token_price_history.source <> 'defillama'",
            params![chain, address, sample_timestamp, price, observed_at],
        )?;
    }
    for ((chain, address), price) in catalog_fallback {
        let sample_timestamp = observed_at.div_euclid(RPC_SAMPLE_SECS) * RPC_SAMPLE_SECS;
        written += tx.execute(
            "INSERT OR IGNORE INTO price.token_price_history(chain,address,timestamp,price_usd,source,observed_at) VALUES(?1,?2,?3,?4,'catalog_pool_snapshot',?5)",
            params![chain, address, sample_timestamp, price, observed_at],
        )?;
    }
    let cutoff = observed_at - (retention_days.max(1) as i64) * 86_400;
    tx.execute(
        "DELETE FROM price.token_price_history WHERE timestamp < ?1",
        params![cutoff],
    )?;
    tx.commit()?;
    Ok(written)
}

fn write_pool_ratios(
    db: &mut Connection,
    ratios: &[PoolRatio],
    pools: &[PoolObservation],
    token_prices: &HashMap<(String, String), f64>,
    observed_at: i64,
    retention_days: u64,
) -> Result<usize> {
    let tx = db.transaction()?;
    let timestamp = observed_at.div_euclid(RPC_SAMPLE_SECS) * RPC_SAMPLE_SECS;
    let mut written = 0;
    let ratios_by_pool = ratios
        .iter()
        .map(|ratio| ((ratio.chain.as_str(), ratio.pool_address.as_str()), ratio))
        .collect::<HashMap<_, _>>();
    for pool in pools {
        let Some(ratio) = ratios_by_pool.get(&(pool.chain.as_str(), pool.pool_address.as_str()))
        else {
            written += tx.execute(
                "INSERT OR REPLACE INTO price.pool_price_history(chain,pool_address,timestamp,token0_address,token1_address,token1_per_token0,token0_price_usd,token1_price_usd,source,price_status,status_reason,observed_at) VALUES(?1,?2,?3,?4,?5,NULL,NULL,NULL,'rpc_pool_unavailable','unavailable',?6,?7)",
                params![
                    pool.chain,
                    pool.pool_address,
                    timestamp,
                    pool.token0_address,
                    pool.token1_address,
                    pool.unavailable_reason,
                    observed_at
                ],
            )?;
            continue;
        };
        let anchor0 = token_prices
            .get(&(ratio.chain.clone(), ratio.token0_address.clone()))
            .copied();
        let anchor1 = token_prices
            .get(&(ratio.chain.clone(), ratio.token1_address.clone()))
            .copied();
        // Keep the observation pool-local even when the USD anchor is shared
        // by many pools. Anchor one side, then derive the other side through
        // this pool's decoded ratio; do not copy both global token prices.
        let (token0_price_usd, token1_price_usd) = if let Some(price0) = anchor0 {
            let price1 = price0 * ratio.token1_per_token0;
            if price1.is_finite() && price1 > 0.0 {
                (Some(price0), Some(price1))
            } else {
                (None, None)
            }
        } else if let Some(price1) = anchor1 {
            let price0 = price1 / ratio.token1_per_token0;
            if price0.is_finite() && price0 > 0.0 {
                (Some(price0), Some(price1))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };
        written += tx.execute(
            "INSERT OR REPLACE INTO price.pool_price_history(chain,pool_address,timestamp,token0_address,token1_address,token1_per_token0,token0_price_usd,token1_price_usd,source,price_status,status_reason,observed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'priced',NULL,?10)",
            params![
                ratio.chain,
                ratio.pool_address,
                timestamp,
                ratio.token0_address,
                ratio.token1_address,
                ratio.token1_per_token0,
                token0_price_usd,
                token1_price_usd,
                ratio.source,
                observed_at
            ],
        )?;
    }
    let cutoff = observed_at - (retention_days.max(1) as i64) * 86_400;
    tx.execute(
        "DELETE FROM price.pool_price_history WHERE timestamp < ?1",
        params![cutoff],
    )?;
    tx.commit()?;
    Ok(written)
}

fn write_fetch_state(
    db: &mut Connection,
    tokens: &[TokenRef],
    prices: &HashMap<(String, String), Vec<(i64, f64)>>,
    attempted_at: i64,
) -> Result<()> {
    let tx = db.transaction()?;
    for token in tokens {
        let key = (token.chain.clone(), token.address.clone());
        let success = prices.contains_key(&key);
        tx.execute(
            "INSERT INTO price.token_price_fetch_state(chain,address,source,last_attempt_at,last_success_at,last_error)
             VALUES(?1,?2,'defillama',?3,?4,?5)
             ON CONFLICT(chain,address,source) DO UPDATE SET
               last_attempt_at=excluded.last_attempt_at,
               last_success_at=coalesce(excluded.last_success_at, price.token_price_fetch_state.last_success_at),
               last_error=excluded.last_error",
            params![
                token.chain,
                token.address,
                attempted_at,
                success.then_some(attempted_at),
                (!success).then_some("no_data_or_request_error"),
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub async fn run() -> Result<()> {
    let db_path = arg("--db", DEFAULT_LP_DB);
    let chain_filter = arg("--chain", "");
    let chain_filter = (!chain_filter.trim().is_empty()).then_some(chain_filter);
    let protocol_filter = arg("--protocol", "");
    let protocol_filter = (!protocol_filter.trim().is_empty()).then_some(protocol_filter);
    let rpc_proxy = arg("--rpc-proxy", "http://127.0.0.1:8545");
    let resolve_decimals = matches!(
        arg("--resolve-decimals", "true")
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    );
    let limit = arg("--limit", "0")
        .parse::<usize>()
        .context("parse --limit")?;
    let batch_size = arg("--batch-size", "50")
        .parse::<usize>()
        .context("parse --batch-size")?
        .max(1);
    let workers = arg("--workers", "8")
        .parse::<usize>()
        .context("parse --workers")?
        .max(1);
    let span_days = arg("--span-days", "30")
        .parse::<u64>()
        .context("parse --span-days")?;
    let retry_after = arg("--external-retry-hours", "24")
        .parse::<i64>()
        .context("parse --external-retry-hours")?
        .max(1)
        * 3_600;
    let refresh_external = matches!(
        arg("--refresh-external", "false")
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    );
    let rpc_only = matches!(
        arg("--rpc-only", "false").to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    );
    let mut rebuild_rpc_history = enabled("--rebuild-rpc-history");
    let duration_secs = arg("--duration-secs", "0")
        .parse::<u64>()
        .context("parse --duration-secs")?;
    let interval_secs = arg("--interval-secs", "3600")
        .parse::<u64>()
        .context("parse --interval-secs")?
        .max(1);
    let price_path = arg("--price-db", &price_db_path(&db_path));
    let mut db = Connection::open(&db_path).with_context(|| format!("open {db_path}"))?;
    if Path::new(&price_path) != Path::new(&db_path) {
        db.execute("ATTACH DATABASE ?1 AS price", [&price_path])?;
    }
    ensure_schema(&mut db)?;
    if enabled("--schema-only") {
        return Ok(());
    }
    let started_at = now();
    loop {
        if rebuild_rpc_history {
            let transaction = db.transaction()?;
            transaction.execute(
                "DELETE FROM price.token_price_history WHERE source='rpc_pool_state'",
                [],
            )?;
            transaction.execute(
                "DELETE FROM price.pool_price_history WHERE source IN ('rpc_pool_state','rpc_pool_direct','rpc_pool_unavailable')",
                [],
            )?;
            transaction.commit()?;
            rebuild_rpc_history = false;
        }
        let observed_at = now();
        let tokens = if rpc_only {
            Vec::new()
        } else {
            load_tokens(&db, limit, observed_at, retry_after, refresh_external)?
        };
        let mut batches = Vec::new();
        let mut by_chain: HashMap<String, Vec<TokenRef>> = HashMap::new();
        for token in &tokens {
            by_chain
                .entry(token.chain.clone())
                .or_default()
                .push(token.clone());
        }
        for chain_tokens in by_chain.into_values() {
            for chunk in chain_tokens.chunks(batch_size) {
                batches.push(chunk.to_vec());
            }
        }
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(workers * 2)
            .build()?;
        let resolved_decimals = if resolve_decimals {
            resolve_missing_decimals(
                &client,
                &mut db,
                &rpc_proxy,
                workers,
                chain_filter.as_deref(),
                protocol_filter.as_deref(),
                observed_at,
            )
            .await?
        } else {
            0
        };
        let mut tasks = JoinSet::new();
        let mut result = BatchResult::default();
        for batch in batches {
            let client = client.clone();
            tasks.spawn(async move { fetch_batch(client, batch, span_days).await });
            while tasks.len() >= workers {
                let batch = tasks.join_next().await.context("join price batch")??;
                result.errors += batch.errors;
                result.prices.extend(batch.prices);
            }
        }
        while let Some(item) = tasks.join_next().await {
            let batch = item?;
            result.errors += batch.errors;
            result.prices.extend(batch.prices);
        }
        let groups = load_pool_groups(&db, chain_filter.as_deref(), protocol_filter.as_deref())?;
        let pool_observations =
            load_pool_observations(&db, chain_filter.as_deref(), protocol_filter.as_deref())?;
        // Decode pool_rpc_state once. The result is then bucketed in memory by
        // (chain, protocol); scanning and parsing the same JSON state once per
        // group made a full 108-group cycle unnecessarily expensive.
        let (edges, stable_seeds, pool_ratios) =
            rpc_edges(&db, chain_filter.as_deref(), protocol_filter.as_deref())?;
        let direct_solana_pools =
            load_solana_vault_pools(&db, chain_filter.as_deref(), protocol_filter.as_deref())?;
        let direct_solana_ratios =
            match read_solana_vault_prices(&client, &rpc_proxy, direct_solana_pools).await {
                Ok(ratios) => ratios,
                Err(error) => {
                    eprintln!("direct Solana pool price read failed: {error}");
                    Vec::new()
                }
            };
        let direct_solana_price_count = direct_solana_ratios.len();
        let mut pool_ratios = pool_ratios;
        merge_pool_ratios(&mut pool_ratios, direct_solana_ratios);
        let direct_balancer_pools =
            load_balancer_pools(&db, chain_filter.as_deref(), protocol_filter.as_deref())?;
        let direct_balancer_ratios =
            match read_balancer_prices(&client, &rpc_proxy, direct_balancer_pools).await {
                Ok(ratios) => ratios,
                Err(error) => {
                    eprintln!("direct Balancer pool price read failed: {error}");
                    Vec::new()
                }
            };
        let direct_balancer_price_count = direct_balancer_ratios.len();
        merge_pool_ratios(&mut pool_ratios, direct_balancer_ratios);
        let direct_evm_pools =
            load_evm_direct_pools(&db, chain_filter.as_deref(), protocol_filter.as_deref())?;
        let direct_evm_ratios =
            match read_evm_direct_prices(&client, &rpc_proxy, direct_evm_pools).await {
                Ok(ratios) => ratios,
                Err(error) => {
                    eprintln!("direct EVM pool price read failed: {error}");
                    Vec::new()
                }
            };
        let direct_evm_price_count = direct_evm_ratios.len();
        merge_pool_ratios(&mut pool_ratios, direct_evm_ratios);
        let direct_fluid_pools =
            load_fluid_pools(&db, chain_filter.as_deref(), protocol_filter.as_deref())?;
        let direct_fluid_ratios =
            match read_fluid_prices(&client, &rpc_proxy, direct_fluid_pools).await {
                Ok(ratios) => ratios,
                Err(error) => {
                    eprintln!("direct Fluid pool price read failed: {error}");
                    Vec::new()
                }
            };
        let direct_fluid_price_count = direct_fluid_ratios.len();
        merge_pool_ratios(&mut pool_ratios, direct_fluid_ratios);
        let mut group_price_counts: HashMap<(String, String), usize> = HashMap::new();
        for ratio in &pool_ratios {
            *group_price_counts
                .entry((ratio.chain.clone(), ratio.protocol.clone()))
                .or_default() += 1;
        }
        let mut group_summary = Vec::with_capacity(groups.len());
        for group in &groups {
            let pools_with_price = group_price_counts
                .get(&(group.chain.clone(), group.protocol.clone()))
                .copied()
                .unwrap_or_default();
            group_summary.push(serde_json::json!({
                "chain": group.chain,
                "protocol": group.protocol,
                "live_pools": group.live_pools,
                "pools_with_price": pools_with_price,
                "pools_without_price": group.live_pools.saturating_sub(pools_with_price),
            }));
        }
        let live_pools: usize = groups.iter().map(|group| group.live_pools).sum();
        let pools_with_price = pool_ratios.len();
        let stable_prices = propagate_rpc_prices(edges.clone(), stable_seeds);
        let history_seeds = latest_history_seeds(&db)?;
        let catalog_seeds = catalog_price_seeds(&db)?;
        let history_keys = history_seeds.keys().cloned().collect::<HashSet<_>>();
        let mut component_seeds = stable_prices.clone();
        for (key, price) in history_seeds {
            component_seeds.entry(key).or_insert(price);
        }
        for (key, price) in &catalog_seeds {
            component_seeds.entry(key.clone()).or_insert(*price);
        }
        let all_component_prices = propagate_rpc_prices(edges, component_seeds);
        let rpc_prices = all_component_prices
            .clone()
            .into_iter()
            .filter(|(key, _)| !history_keys.contains(key) || stable_prices.contains_key(key))
            .collect::<HashMap<_, _>>();
        write_fetch_state(&mut db, &tokens, &result.prices, observed_at)?;
        let written = write_prices(
            &mut db,
            &result.prices,
            &rpc_prices,
            &catalog_seeds,
            observed_at,
            span_days,
        )?;
        let ratio_points = write_pool_ratios(
            &mut db,
            &pool_ratios,
            &pool_observations,
            &all_component_prices,
            observed_at,
            span_days,
        )?;
        println!(
            "{}",
            serde_json::json!({
                "chain_filter": chain_filter,
                "protocol_filter": protocol_filter,
                "rpc_proxy": rpc_proxy,
                "resolve_decimals": resolve_decimals,
                "resolved_decimals": resolved_decimals,
                "direct_solana_pool_prices": direct_solana_price_count,
                "direct_balancer_pool_prices": direct_balancer_price_count,
                "direct_evm_pool_prices": direct_evm_price_count,
                "direct_fluid_pool_prices": direct_fluid_price_count,
                "groups": group_summary,
                "live_pools": live_pools,
                "pools_with_price": pools_with_price,
                "pools_without_price": live_pools.saturating_sub(pools_with_price),
                "external_tokens": tokens.len(),
                "llama_tokens": result.prices.len(),
                "rpc_tokens": rpc_prices.len(),
                "catalog_tokens": catalog_seeds.len(),
                "pool_ratio_points": pools_with_price,
                "pool_observation_points": ratio_points,
                "pool_observations": pool_observations.len(),
                "pool_unavailable_observations": pool_observations
                    .len()
                    .saturating_sub(pool_ratios.len()),
                "llama_errors": result.errors,
                "written_points": written,
                "external_retry_hours": retry_after / 3_600,
                "refresh_external": refresh_external,
                "rpc_only": rpc_only,
                "duration_secs": duration_secs,
                "interval_secs": interval_secs,
            })
        );
        if duration_secs == 0 || now().saturating_sub(started_at) >= duration_secs as i64 {
            break;
        }
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{parse_balancer_query_output, parse_balancer_tokens, raw_ratio};

    #[test]
    fn decodes_balancer_token_and_balance_arrays() {
        let word = |value: &str| format!("{value:0>64}");
        let data = format!(
            "0x{}{}{}{}{}{}{}{}{}",
            word("40"),
            word("a0"),
            word("2"),
            word("0000000000000000000000000000000000000001"),
            word("0000000000000000000000000000000000000002"),
            word("2"),
            word("64"),
            word("c8"),
            word("1")
        );
        assert_eq!(
            parse_balancer_tokens(
                &data,
                "0x0000000000000000000000000000000000000001",
                "0x0000000000000000000000000000000000000002"
            ),
            Some(("100".into(), "200".into()))
        );
    }

    #[test]
    fn decodes_negative_balancer_query_output() {
        let data = format!(
            "0x{}{}{}{}",
            "20".repeat(32),
            format!("{:0>64}", "2"),
            format!("{:0>64}", "1"),
            format!("{}{:0>32}", "f".repeat(32), format!("{:x}", u128::MAX - 99))
        );
        assert_eq!(parse_balancer_query_output(&data), Some(100));
    }

    #[test]
    fn liquidity_book_uses_active_bin_before_total_reserves() {
        let ratio = raw_ratio(
            "liquidity_book",
            Some("1000000000000"),
            Some("1"),
            None,
            Some(r#"{"active_id":"8388608","bin_step":100}"#),
            18,
            18,
        )
        .expect("active bin ratio");
        assert!((ratio - 1.0).abs() < 1e-12);
    }

    #[test]
    fn zero_reserves_do_not_become_a_price() {
        assert!(raw_ratio(
            "constant_product_v2",
            Some("0"),
            Some("1"),
            None,
            None,
            18,
            18,
        )
        .is_none());
    }

    #[test]
    fn very_large_human_unit_ratios_are_kept_for_tiny_tokens() {
        let ratio = raw_ratio(
            "constant_product_v2",
            Some("1"),
            Some("1000000000000000000000000000000"),
            None,
            None,
            18,
            0,
        )
        .expect("large human-unit ratio");
        assert!(ratio > 1e30);
    }

    #[test]
    fn decodes_ekubo_dynamic_sqrt_ratio() {
        // Ekubo's ONE encoding is 2^95 + 2^62 and expands to Q64.128 == 1.
        let ratio = raw_ratio(
            "concentrated_v3",
            None,
            None,
            None,
            Some(r#"{"sqrt_ratio_dynamic_raw":"39614081261743854815299413056"}"#),
            18,
            18,
        )
        .expect("Ekubo dynamic ratio");
        assert!((ratio - 1.0).abs() < 1e-12);
    }
}
