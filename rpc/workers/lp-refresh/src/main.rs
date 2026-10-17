use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use reqwest::Client;
use rusqlite::{params, params_from_iter, Connection};
use serde_json::{json, Value};
use tiny_keccak::{Hasher, Keccak};
use tokio::sync::Semaphore;

mod model;
use model::*;

const TOKEN0: &str = "0x0dfe1681";
const TOKEN1: &str = "0xd21220a7";
const SLOT0: &str = "0x3850c7bd";
const LIQUIDITY: &str = "0x1a686502";
const FEE: &str = "0xddca3f43";
const RESERVES: &str = "0x0902f1ac";
const STATE_WRITE_BATCH_SIZE: usize = 200;

fn arg(name: &str, default: &str) -> String {
    let args: Vec<String> = env::args().collect();
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .unwrap_or_else(|| default.into())
}

fn has_flag(name: &str) -> bool {
    env::args().any(|value| value == name)
}

fn redact_rpc_error(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0usize;
    while cursor < value.len() {
        let https = value[cursor..]
            .find("https://")
            .map(|offset| cursor + offset);
        let http = value[cursor..]
            .find("http://")
            .map(|offset| cursor + offset);
        let start = match (https, http) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(found), None) | (None, Some(found)) => Some(found),
            (None, None) => None,
        };
        let Some(start) = start else {
            output.push_str(&value[cursor..]);
            break;
        };
        output.push_str(&value[cursor..start]);
        let end = value[start..]
            .find(|character: char| {
                character.is_whitespace() || matches!(character, ')' | ']' | '|' | ',')
            })
            .map(|offset| start + offset)
            .unwrap_or(value.len());
        output.push_str("[rpc endpoint redacted]");
        cursor = end;
    }
    output
}

fn chain_network(chain: &str) -> Option<&'static str> {
    match chain.to_ascii_lowercase().as_str() {
        "ethereum" => Some("eth-mainnet"),
        "polygon" => Some("polygon-mainnet"),
        "arbitrum" => Some("arb-mainnet"),
        "optimism" => Some("opt-mainnet"),
        "base" => Some("base-mainnet"),
        "bsc" | "bnb" => Some("bnb-mainnet"),
        "avalanche" => Some("avax-mainnet"),
        "linea" => Some("linea-mainnet"),
        "scroll" => Some("scroll-mainnet"),
        "blast" => Some("blast-mainnet"),
        "mantle" => Some("mantle-mainnet"),
        "zksync" => Some("zksync-mainnet"),
        "arb-nova" => Some("arb-nova-mainnet"),
        "celo" => Some("celo-mainnet"),
        "berachain" => Some("berachain-mainnet"),
        "sonic" => Some("sonic-mainnet"),
        "sei" => Some("sei-mainnet"),
        "mode" => Some("mode-mainnet"),
        "unichain" => Some("unichain-mainnet"),
        "worldchain" => Some("worldchain-mainnet"),
        "soneium" => Some("soneium-mainnet"),
        "moonbeam" => Some("moonbeam-mainnet"),
        "gnosis" => Some("gnosis-mainnet"),
        "opbnb" => Some("opbnb-mainnet"),
        "frax" | "fraxtal" => Some("frax-mainnet"),
        "astar" => Some("astar-mainnet"),
        "zetachain" => Some("zetachain-mainnet"),
        "zora" => Some("zora-mainnet"),
        "metis" => Some("metis-mainnet"),
        "ink" => Some("ink-mainnet"),
        "monad" => Some("monad-mainnet"),
        "solana" => Some("solana-mainnet"),
        _ => None,
    }
}

fn push_endpoint(result: &mut HashMap<String, Vec<Endpoint>>, endpoint: Endpoint) {
    let entries = result.entry(endpoint.network.clone()).or_default();
    if !entries.iter().any(|existing| existing.url == endpoint.url) {
        entries.push(endpoint);
    }
}

fn effective_batch_size(chain: &str, configured: usize) -> usize {
    // Polygon free-tier gateways often meter eth_call by compute units rather
    // than HTTP request count. A multi-pool JSON-RPC batch can therefore
    // exceed the burst budget even when the request rate is low. Keep the
    // default safe for this chain; callers can still explicitly use a smaller
    // value, while other chains retain the configured throughput.
    if chain.eq_ignore_ascii_case("polygon") {
        configured.min(1).max(1)
    } else {
        configured.max(1)
    }
}

fn local_rpc_proxy() -> Option<String> {
    Some(env::var("RPC_PROXY_URL").unwrap_or_else(|_| "http://127.0.0.1:8545".into()))
        .map(|value| value.trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn price_db_path(catalog_path: &Path) -> PathBuf {
    env::var_os("LP_PRICE_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            catalog_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("lp_price_history.db")
        })
}

fn load_local_proxy_endpoints(base: &str) -> HashMap<String, Vec<Endpoint>> {
    let networks = [
        ("eth-mainnet", "EVM"),
        ("polygon-mainnet", "EVM"),
        ("arb-mainnet", "EVM"),
        ("arb-nova-mainnet", "EVM"),
        ("opt-mainnet", "EVM"),
        ("base-mainnet", "EVM"),
        ("bnb-mainnet", "EVM"),
        ("avax-mainnet", "EVM"),
        ("celo-mainnet", "EVM"),
        ("berachain-mainnet", "EVM"),
        ("linea-mainnet", "EVM"),
        ("scroll-mainnet", "EVM"),
        ("blast-mainnet", "EVM"),
        ("mantle-mainnet", "EVM"),
        ("zksync-mainnet", "EVM"),
        ("sonic-mainnet", "EVM"),
        ("sei-mainnet", "EVM"),
        ("mode-mainnet", "EVM"),
        ("unichain-mainnet", "EVM"),
        ("worldchain-mainnet", "EVM"),
        ("soneium-mainnet", "EVM"),
        ("moonbeam-mainnet", "EVM"),
        ("gnosis-mainnet", "EVM"),
        ("opbnb-mainnet", "EVM"),
        ("frax-mainnet", "EVM"),
        ("astar-mainnet", "EVM"),
        ("zetachain-mainnet", "EVM"),
        ("zora-mainnet", "EVM"),
        ("metis-mainnet", "EVM"),
        ("ink-mainnet", "EVM"),
        ("monad-mainnet", "EVM"),
        ("solana-mainnet", "Solana"),
    ];
    let mut result = HashMap::new();
    for (network, protocol) in networks {
        push_endpoint(
            &mut result,
            Endpoint {
                network: network.into(),
                protocol: protocol.into(),
                url: format!("{base}/rpc/{network}"),
            },
        );
    }
    result
}

fn load_endpoints() -> Result<HashMap<String, Vec<Endpoint>>> {
    if env::var("RPC_DIRECT_CATALOG").ok().as_deref() != Some("1") {
        let proxy = local_rpc_proxy().expect("default local RPC proxy URL is non-empty");
        return Ok(load_local_proxy_endpoints(&proxy));
    }
    let db_path = env::var("RPC_ENDPOINT_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("rpc/data/rpc.db"));
    let connection = Connection::open(&db_path)
        .with_context(|| format!("open RPC endpoint database {}", db_path.display()))?;
    let mut statement = connection.prepare(
        "SELECT c.network_slug, c.protocol, e.url FROM chains c JOIN rpc_endpoints e ON e.chain_id = c.id WHERE c.enabled = 1 AND e.enabled = 1 ORDER BY e.priority, e.id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(Endpoint {
            network: row.get(0)?,
            protocol: row.get(1)?,
            url: row.get(2)?,
        })
    })?;
    let mut result = HashMap::new();
    for row in rows {
        push_endpoint(&mut result, row?);
    }
    if result.is_empty() {
        return Err(anyhow!(
            "RPC endpoint database {} contains no enabled endpoints",
            db_path.display()
        ));
    }
    Ok(result)
}

async fn select_endpoint(
    client: &Client,
    endpoints: &[Endpoint],
    solana: bool,
) -> Result<(Endpoint, i64)> {
    let mut errors = Vec::new();
    for endpoint in endpoints {
        for attempt in 0..3u64 {
            match latest_block(client, endpoint, solana).await {
                Ok(block) => return Ok((endpoint.clone(), block)),
                Err(error) => {
                    errors.push(redact_rpc_error(&error.to_string()));
                    if attempt < 2 {
                        tokio::time::sleep(Duration::from_millis(250 * (attempt + 1))).await;
                    }
                }
            }
        }
    }
    Err(anyhow!("all RPC endpoints failed: {}", errors.join(" | ")))
}

fn load_pools(
    path: &Path,
    chain_filter: Option<&str>,
    state_status: Option<&str>,
    limit: usize,
    maintenance_only: bool,
    exclude_priority: bool,
) -> Result<Vec<Pool>> {
    let connection =
        Connection::open(path).with_context(|| format!("open SQLite {}", path.display()))?;
    connection.execute_batch("PRAGMA busy_timeout=5000;")?;
    // A full refresh visits every row, so ranking the complete table by
    // liquidity only adds a temporary sort. Keep ranking for --limit smoke
    // tests, while the full path walks stable chain/id order.
    let order = if limit > 0 {
        "ORDER BY coalesce(p.liquidity_usd, 0) DESC, coalesce(p.volume_24h_usd, 0) DESC, p.id"
    } else {
        "ORDER BY p.chain, p.id"
    };
    let maintenance_join = if maintenance_only {
        " JOIN lp_maintenance m ON m.pool_address = p.pool_address AND m.enabled = 1"
    } else {
        ""
    };
    let state_join = if state_status.is_some() {
        " JOIN pool_rpc_state s ON s.pool_address = p.pool_address"
    } else {
        ""
    };
    let state_where = if state_status.is_some() {
        " WHERE s.state_status = ?1"
    } else {
        ""
    };
    let priority_join = if exclude_priority {
        " LEFT JOIN priority_pools pr ON pr.pool_address = p.pool_address AND pr.enabled = 1"
    } else {
        ""
    };
    let priority_where = if exclude_priority {
        if state_status.is_some() {
            " AND pr.pool_address IS NULL"
        } else {
            " WHERE pr.pool_address IS NULL"
        }
    } else {
        ""
    };
    let query = format!(
        "SELECT p.pool_address, p.chain, coalesce(p.protocol,''), coalesce(p.pool_type,''), coalesce(p.token_a_symbol,''), coalesce(p.token_b_symbol,''), p.token_a_address, p.token_b_address FROM pools p{maintenance_join}{state_join}{priority_join}{state_where}{priority_where} {order}"
    );
    let mut statement = connection.prepare(&query).with_context(|| {
        if maintenance_only {
            "open lp_maintenance selection; run the LP maintenance database migration first"
        } else {
            "prepare pool selection"
        }
    })?;
    let rows = statement.query_map(params_from_iter(state_status.iter()), |row| {
        Ok(Pool {
            address: row.get(0)?,
            chain: row.get(1)?,
            protocol: row.get(2)?,
            pool_type: row.get(3)?,
            token_a_symbol: row.get(4)?,
            token_b_symbol: row.get(5)?,
            token_a_address: row.get(6)?,
            token_b_address: row.get(7)?,
        })
    })?;
    let mut pools = Vec::new();
    for row in rows {
        let pool = row?;
        if chain_filter.is_some_and(|chain| !pool.chain.eq_ignore_ascii_case(chain)) {
            continue;
        }
        if chain_network(&pool.chain).is_none() {
            continue;
        }
        if !pool.chain.eq_ignore_ascii_case("solana") {
            let is_evm_address = pool.address.starts_with("0x") && pool.address.len() == 42;
            let is_pool_id = (pool.pool_type.eq_ignore_ascii_case("uniswap_v4")
                || pool.protocol.to_ascii_lowercase().contains("ekubo"))
                && pool.address.starts_with("0x")
                && pool.address.len() == 66;
            if !is_evm_address && !is_pool_id {
                continue;
            }
        }
        pools.push(pool);
        if limit > 0 && pools.len() >= limit {
            break;
        }
    }
    Ok(pools)
}

fn load_history_pools(
    path: &Path,
    price_path: &Path,
    chain_filter: Option<&str>,
    required_points: usize,
    limit: usize,
) -> Result<Vec<Pool>> {
    let connection = Connection::open(path)?;
    connection.execute(
        "ATTACH DATABASE ?1 AS price",
        [price_path.to_string_lossy().as_ref()],
    )?;
    let query = r#"
        SELECT p.pool_address, p.chain, coalesce(p.protocol,''),
               coalesce(p.pool_type,''), coalesce(p.token_a_symbol,''),
               coalesce(p.token_b_symbol,''), p.token_a_address, p.token_b_address
        FROM pools p
        JOIN pool_rpc_state s ON s.pool_address = p.pool_address
        LEFT JOIN (
          SELECT chain, pool_address, count(*) AS points
          FROM price.pool_price_history
          WHERE coalesce(price_status, 'priced') = 'priced'
          GROUP BY chain, pool_address
        ) h ON h.chain = p.chain AND h.pool_address = p.pool_address
        WHERE s.state_status = 'live'
          AND p.chain <> 'solana'
          AND coalesce(h.points, 0) < ?1
          AND (?2 IS NULL OR lower(p.chain) = lower(?2))
        ORDER BY p.chain, p.id
    "#;
    let mut statement = connection.prepare(query)?;
    let rows = statement.query_map(params![required_points as i64, chain_filter], |row| {
        Ok(Pool {
            address: row.get(0)?,
            chain: row.get(1)?,
            protocol: row.get(2)?,
            pool_type: row.get(3)?,
            token_a_symbol: row.get(4)?,
            token_b_symbol: row.get(5)?,
            token_a_address: row.get(6)?,
            token_b_address: row.get(7)?,
        })
    })?;
    let mut pools = Vec::new();
    for row in rows {
        let pool = row?;
        if !pool.address.starts_with("0x") {
            continue;
        }
        pools.push(pool);
        if limit > 0 && pools.len() >= limit {
            break;
        }
    }
    Ok(pools)
}

fn load_priority_pools(path: &Path, chain_filter: Option<&str>, limit: usize) -> Result<Vec<Pool>> {
    let connection =
        Connection::open(path).with_context(|| format!("open SQLite {}", path.display()))?;
    connection.execute_batch("PRAGMA busy_timeout=5000;")?;
    let query = "SELECT p.pool_address, p.chain, coalesce(p.protocol,''), coalesce(p.pool_type,''), coalesce(p.token_a_symbol,''), coalesce(p.token_b_symbol,''), p.token_a_address, p.token_b_address FROM priority_pools pr JOIN pools p ON p.pool_address = pr.pool_address WHERE pr.enabled = 1 ORDER BY pr.priority_rank";
    let mut statement = connection.prepare(query)?;
    let rows = statement.query_map([], |row| {
        Ok(Pool {
            address: row.get(0)?,
            chain: row.get(1)?,
            protocol: row.get(2)?,
            pool_type: row.get(3)?,
            token_a_symbol: row.get(4)?,
            token_b_symbol: row.get(5)?,
            token_a_address: row.get(6)?,
            token_b_address: row.get(7)?,
        })
    })?;
    let mut pools = Vec::new();
    for row in rows {
        let pool = row?;
        if chain_filter.is_some_and(|chain| !pool.chain.eq_ignore_ascii_case(chain)) {
            continue;
        }
        if limit > 0 && pools.len() >= limit {
            break;
        }
        pools.push(pool);
    }
    Ok(pools)
}

fn ensure_state_table(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS pool_rpc_state (
          pool_address TEXT PRIMARY KEY,
          chain TEXT NOT NULL,
          rpc_network TEXT NOT NULL,
          rpc_protocol TEXT NOT NULL,
          checked_at INTEGER NOT NULL,
          state_block INTEGER,
          account_exists INTEGER,
          state_status TEXT NOT NULL,
          pool_family TEXT NOT NULL DEFAULT 'unknown',
          decoder_version TEXT NOT NULL DEFAULT 'none',
          state_error TEXT,
          state_json TEXT,
          assets_json TEXT,
          quote_supported INTEGER NOT NULL DEFAULT 0,
          token0_address TEXT,
          token1_address TEXT,
          fee_bps REAL,
          reserve0_raw TEXT,
          reserve1_raw TEXT,
          sqrt_price_x96 TEXT,
          active_liquidity_raw TEXT,
          spot_price_usd REAL,
          account_owner TEXT,
          account_data_bytes INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_pool_rpc_state_chain_status ON pool_rpc_state(chain, state_status);
        CREATE INDEX IF NOT EXISTS idx_pool_rpc_state_checked ON pool_rpc_state(checked_at);
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
        CREATE TABLE IF NOT EXISTS priority_pools (
          pool_address TEXT PRIMARY KEY REFERENCES pools(pool_address) ON DELETE CASCADE,
          chain TEXT NOT NULL,
          protocol TEXT NOT NULL,
          priority_rank INTEGER NOT NULL UNIQUE,
          enabled INTEGER NOT NULL DEFAULT 1,
          selection_reason TEXT NOT NULL,
          updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_priority_pools_enabled_rank
          ON priority_pools(enabled, priority_rank);
        "#,
    )?;
    let priority_count: i64 = connection.query_row(
        "SELECT count(*) FROM priority_pools WHERE enabled = 1",
        [],
        |row| row.get(0),
    )?;
    if priority_count == 0 {
        connection.execute(
            r#"
            WITH ranked AS (
              SELECT p.pool_address, p.chain, coalesce(p.protocol, '') AS protocol,
                     ROW_NUMBER() OVER (
                       ORDER BY coalesce(p.liquidity_usd, 0) DESC,
                                coalesce(p.volume_24h_usd, 0) DESC,
                                p.id
                     ) AS priority_rank
              FROM pools p
              WHERE p.chain IN ('arbitrum','avalanche','base','bsc','ethereum','ink','linea','monad','optimism','polygon','scroll','solana','zksync')
                AND coalesce(p.liquidity_usd, 0) BETWEEN 100 AND 100000000
                AND p.pool_address <> ''
                AND (p.chain = 'solana' OR (p.pool_address LIKE '0x%' AND (length(p.pool_address) = 42 OR (lower(coalesce(p.pool_type,'')) = 'uniswap_v4' AND length(p.pool_address) = 66))))
            )
            INSERT INTO priority_pools
              (pool_address, chain, protocol, priority_rank, enabled, selection_reason, updated_at)
            SELECT pool_address, chain, protocol, priority_rank, 1,
                   'top 1000 by liquidity, then 24h volume; fixed-block refresh universe',
                   unixepoch()
            FROM ranked WHERE priority_rank <= 1000
            "#,
            [],
        )?;
    }
    for statement in [
        "ALTER TABLE pool_rpc_state ADD COLUMN pool_family TEXT NOT NULL DEFAULT 'unknown'",
        "ALTER TABLE pool_rpc_state ADD COLUMN decoder_version TEXT NOT NULL DEFAULT 'none'",
        "ALTER TABLE pool_rpc_state ADD COLUMN state_json TEXT",
        "ALTER TABLE pool_rpc_state ADD COLUMN assets_json TEXT",
        "ALTER TABLE pool_rpc_state ADD COLUMN quote_supported INTEGER NOT NULL DEFAULT 0",
    ] {
        match connection.execute(statement, []) {
            Ok(_) => {}
            Err(error) if error.to_string().contains("duplicate column name") => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn ensure_price_history_table(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS pool_price_history (
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
        CREATE INDEX IF NOT EXISTS idx_pool_price_history_lookup
          ON pool_price_history(chain, pool_address, timestamp);
        "#,
    )?;
    for statement in [
        "ALTER TABLE pool_price_history ADD COLUMN token0_price_usd REAL",
        "ALTER TABLE pool_price_history ADD COLUMN token1_price_usd REAL",
        "ALTER TABLE pool_price_history ADD COLUMN price_status TEXT NOT NULL DEFAULT 'priced'",
        "ALTER TABLE pool_price_history ADD COLUMN status_reason TEXT",
    ] {
        match connection.execute(statement, []) {
            Ok(_) => {}
            Err(error) if error.to_string().contains("duplicate column name") => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn transport_response(id: u64, error: impl ToString) -> RpcResponse {
    RpcResponse {
        id,
        result: None,
        error: Some(json!({
            "code": -32098,
            "message": redact_rpc_error(&error.to_string()),
            "transport": true,
        })),
    }
}

async fn rpc_batch(
    client: &Client,
    endpoint: &str,
    requests: Vec<RpcRequest<'_>>,
) -> Result<Vec<RpcResponse>> {
    let mut pending = vec![requests];
    let mut responses = Vec::new();
    while let Some(group) = pending.pop() {
        let response = match client.post(endpoint).json(&group).send().await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => response,
                Err(_error) if group.len() > 1 => {
                    let midpoint = group.len() / 2;
                    pending.push(group[midpoint..].to_vec());
                    pending.push(group[..midpoint].to_vec());
                    continue;
                }
                Err(error) => {
                    responses.push(transport_response(group[0].id, error));
                    continue;
                }
            },
            Err(_error) if group.len() > 1 => {
                let midpoint = group.len() / 2;
                pending.push(group[midpoint..].to_vec());
                pending.push(group[..midpoint].to_vec());
                continue;
            }
            Err(error) => {
                responses.push(transport_response(group[0].id, error));
                continue;
            }
        };
        let body = match response.json::<Value>().await {
            Ok(body) => body,
            Err(_error) if group.len() > 1 => {
                let midpoint = group.len() / 2;
                pending.push(group[midpoint..].to_vec());
                pending.push(group[..midpoint].to_vec());
                continue;
            }
            Err(error) => {
                responses.push(transport_response(group[0].id, error));
                continue;
            }
        };
        if let Some(values) = body.as_array() {
            // A few public gateways have been observed to put `null`
            // placeholders in a JSON-RPC batch response. Retrying only the
            // missing request IDs prevents a transient gateway omission from
            // being recorded as account_missing.
            let mut returned_ids = HashSet::with_capacity(values.len());
            for value in values.iter().filter(|value| !value.is_null()) {
                if let Ok(response) = serde_json::from_value::<RpcResponse>(value.clone()) {
                    returned_ids.insert(response.id);
                    responses.push(response);
                }
            }
            let missing = group
                .iter()
                .filter(|request| !returned_ids.contains(&request.id))
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                if missing.len() == 1 {
                    pending.push(missing);
                } else {
                    let midpoint = missing.len() / 2;
                    pending.push(missing[midpoint..].to_vec());
                    pending.push(missing[..midpoint].to_vec());
                }
            }
        } else if group.len() == 1 {
            match serde_json::from_value(body) {
                Ok(response) => responses.push(response),
                Err(error) => responses.push(transport_response(group[0].id, error)),
            }
        } else {
            // Some free-tier gateways reject larger batches instead of
            // returning a normal JSON-RPC error. Split only the rejected batch;
            // retrying every call serially multiplies upstream requests.
            let midpoint = group.len() / 2;
            pending.push(group[midpoint..].to_vec());
            pending.push(group[..midpoint].to_vec());
        }
    }
    Ok(responses)
}

fn response_map(responses: Vec<RpcResponse>) -> HashMap<u64, RpcResponse> {
    responses
        .into_iter()
        .map(|response| (response.id, response))
        .collect()
}

fn pool_transport_error(
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
    solana: bool,
) -> Option<String> {
    let end = if solana { base_id + 1 } else { base_id + 100 };
    let relevant = map
        .iter()
        .filter(|(id, _)| **id >= base_id && **id < end)
        .map(|(_, response)| response)
        .collect::<Vec<_>>();
    if relevant.is_empty()
        || relevant.iter().any(|response| {
            !response
                .error
                .as_ref()
                .and_then(|error| error.get("transport"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
    {
        return None;
    }
    relevant.iter().find_map(|response| {
        response
            .error
            .as_ref()
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn result_hex(map: &HashMap<u64, RpcResponse>, id: u64) -> Option<String> {
    map.get(&id)
        .and_then(|response| response.result.as_ref())
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| value != "0x")
}

fn error_text(map: &HashMap<u64, RpcResponse>, ids: &[u64]) -> Option<String> {
    ids.iter().find_map(|id| {
        map.get(id)
            .and_then(|response| response.error.as_ref())
            .map(|error| error.to_string())
    })
}

fn word(data: &str, index: usize) -> Option<u128> {
    u128::from_str_radix(word_hex(data, index)?, 16).ok()
}

fn low_word(data: &str) -> Option<u128> {
    let value = data.strip_prefix("0x")?;
    let low = value.get(value.len().checked_sub(32)?..)?;
    u128::from_str_radix(low, 16).ok()
}

fn word_hex(data: &str, index: usize) -> Option<&str> {
    let start = 2usize.checked_add(index.checked_mul(64)?)?;
    let end = start.checked_add(64)?;
    data.get(start..end)
}

// ABI words can be wider than u128. sqrtPriceX96 is uint160, and token
// balances/packed values can be full uint256. For price math we only need a
// finite IEEE-754 approximation, so accumulate the hexadecimal word in f64
// without rejecting valid values above the u128 boundary.
fn word_f64(data: &str, index: usize) -> Option<f64> {
    let word = word_hex(data, index)?;
    let mut value = 0.0;
    for byte in word.bytes() {
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

fn address_word(data: &str) -> Option<String> {
    let word = data.get(2..66)?;
    Some(format!("0x{}", &word[24..]))
}

fn symbol_decimals(symbol: &str) -> Option<i32> {
    match symbol.to_ascii_uppercase().as_str() {
        "USDC" | "USDT" | "DAI" | "USDE" | "USDS" | "USDB" | "USDBC" | "FDUSD" | "TUSD"
        | "FRAX" => Some(6),
        "ETH" | "WETH" | "STETH" | "WBTC" | "BTC" => Some(18),
        _ => None,
    }
}

fn known_symbol(address: &str) -> Option<&'static str> {
    match address.to_ascii_lowercase().as_str() {
        // Canonical/common stable and wrapped-ether addresses used by the
        // indexed chains. Unknown tokens remain raw-state-only until their
        // ERC-20 metadata is fetched.
        "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
        | "0xaf88d065e77c8cc2239327c5edb3a432268e5831"
        | "0x0b2c639c533813f4aa9d7837caf62653d097ff85"
        | "0x176211869ca2b568f2a7d4ee941e073a821ee1ff"
        | "0x06efdbff2a14a7c8e15944d1f4a48f9f95f663a4"
        | "0x754704bc059f8c67012fed69bc8a327a5aafb603" => Some("USDC"),
        "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
        | "0x82af49447d8a07e3bd95bd0d56f35241523fbab1"
        | "0x4200000000000000000000000000000000000006"
        | "0xe5d7c2a44ffddf6b295a15c148167daaaf5cf34f"
        | "0x5300000000000000000000000000000000000004"
        | "0xee8c0e9f1bffb4eb878d8f15f368a02a35481242" => Some("WETH"),
        _ => None,
    }
}

fn symbol_for(pool: &Pool, address: &str) -> String {
    if let Some(symbol) = known_symbol(address) {
        return symbol.into();
    }
    if pool
        .token_a_address
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case(address))
    {
        return pool.token_a_symbol.clone();
    }
    if pool
        .token_b_address
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case(address))
    {
        return pool.token_b_symbol.clone();
    }
    pool.token_a_symbol.clone()
}

fn spot_price(pool: &Pool, token0: &str, token1: &str, sqrt_price: &str) -> Option<f64> {
    let sqrt = word_f64(sqrt_price, 0)?;
    let raw_ratio = (sqrt * sqrt) / 2f64.powi(192);
    let symbol0 = symbol_for(pool, token0);
    let symbol1 = symbol_for(pool, token1);
    let decimals0 = symbol_decimals(&symbol0)?;
    let decimals1 = symbol_decimals(&symbol1)?;
    let human_ratio = raw_ratio * 10f64.powi(decimals0 - decimals1);
    let stable0 = symbol_decimals(&symbol0) == Some(6);
    let stable1 = symbol_decimals(&symbol1) == Some(6);
    let value = if stable0 && !stable1 {
        1.0 / human_ratio
    } else if stable1 && !stable0 {
        human_ratio
    } else {
        return None;
    };
    value
        .is_finite()
        .then_some(value)
        .filter(|value| *value > 0.0 && *value < 1.0e12)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvmPoolKind {
    ConstantProduct,
    Concentrated,
    Algebra,
    KyberElastic,
    MaverickV2,
    UniswapV4,
    Weighted,
    Fluid,
    Curve,
    LiquidityBook,
    LegacyLiquidityBook,
    EkuboV3,
    IziSwap,
    Unsupported,
}

fn evm_pool_kind(pool: &Pool) -> EvmPoolKind {
    let protocol = pool.protocol.to_ascii_lowercase();
    let pool_type = pool.pool_type.to_ascii_lowercase();
    // The canonical classifier is authoritative. In particular, protocol names
    // such as "LFJ V2" and "Balancer V2" must not fall through to V2 pair
    // selectors just because they contain the string "V2".
    if protocol.contains("ekubo") {
        return EvmPoolKind::EkuboV3;
    }
    if protocol == "fluid" {
        return EvmPoolKind::Fluid;
    }
    if protocol.contains("maverick") {
        return EvmPoolKind::MaverickV2;
    }
    if matches!(
        protocol.as_str(),
        "camelot v3" | "blackhole v3" | "thena v3" | "thena fusion" | "quickswap v3"
    ) {
        return EvmPoolKind::Algebra;
    }
    if protocol == "lynex" {
        return EvmPoolKind::Algebra;
    }
    // The catalog's plain "KyberSwap" rows are Kyber Classic pairs and expose
    // getReserves(). Kyber Elastic is a separate deployment with a custom
    // concentrated-liquidity state API.
    if protocol == "kyberswap" {
        return EvmPoolKind::ConstantProduct;
    }
    if protocol == "kyberswap elastic" {
        return EvmPoolKind::KyberElastic;
    }
    // The indexed "Ramses V2" label refers to the Ramses concentrated pool
    // deployment. It exposes slot0/liquidity/fee, not the Solidly pair
    // getReserves interface.
    if protocol == "ramses v2" {
        return EvmPoolKind::Concentrated;
    }
    // These database labels describe concentrated pools even though older
    // indexing metadata classified them as constant-product V2 rows. Their
    // live contracts expose slot0/liquidity (or the equivalent V3 state),
    // while getReserves() reverts.
    if matches!(protocol.as_str(), "lynex" | "nile" | "retro" | "nuri v2") {
        return EvmPoolKind::Concentrated;
    }
    if protocol == "iziswap" {
        return EvmPoolKind::IziSwap;
    }
    match pool_type.as_str() {
        "constant_product_v2" | "solidly_v2" => return EvmPoolKind::ConstantProduct,
        "concentrated_v3" => return EvmPoolKind::Concentrated,
        "uniswap_v4" => return EvmPoolKind::UniswapV4,
        "stable_swap"
            if protocol.contains("curve") || protocol.contains("pancakeswap stableswap") =>
        {
            return EvmPoolKind::Curve
        }
        "unsupported_protocol" if protocol.contains("syncswap v2.1") => {
            return EvmPoolKind::ConstantProduct
        }
        "liquidity_book" if protocol == "lfj" => return EvmPoolKind::LiquidityBook,
        "liquidity_book" => return EvmPoolKind::LiquidityBook,
        "weighted" => return EvmPoolKind::Weighted,
        "stable_swap" | "unsupported_protocol" => return EvmPoolKind::Unsupported,
        _ => {}
    }

    // Legacy rows without a canonical type may still use the protocol name.
    if protocol.contains(" v2")
        || protocol.ends_with("v2")
        || matches!(
            protocol.as_str(),
            "pancakeswap"
                | "uniswap"
                | "quickswap"
                | "sushiswap"
                | "apeswap"
                | "pangolin"
                | "camelot"
                | "mdex"
                | "bakeryswap"
                | "syncswap"
                | "lynex"
                | "etherex"
                | "retro"
                | "ramses v2"
                | "nuri v2"
                | "pharaoh"
                | "shibaswap"
                | "fraxswap"
                | "nile"
                | "nile v1"
                | "hydrex"
                | "zkswap"
                | "honeypop dex"
                | "dyorswap"
                | "agni finance v2"
        )
    {
        return EvmPoolKind::ConstantProduct;
    }
    if protocol.contains("v3")
        || protocol.contains("slipstream")
        || protocol.contains("clmm")
        || protocol.contains("concentrated")
        || protocol.contains("ekubo")
        || protocol.contains("iziswap")
        || protocol.contains("kyberswap elastic")
    {
        return EvmPoolKind::Concentrated;
    }
    EvmPoolKind::Unsupported
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

fn v4_state_view(network: &str) -> Option<&'static str> {
    match network {
        "eth-mainnet" => Some("0x7ffe42c4a5deea5b0fec41c94c136cf115597227"),
        "arb-mainnet" => Some("0x76fd297e2d437cd7f76d50f01afe6160f86e9990"),
        "base-mainnet" => Some("0xa3c0c9b65bad0b08107aa264b0f3db444b867a71"),
        "bnb-mainnet" => Some("0xd13dd3d6e93f276fafc9db9e6bb47c1180aee0c4"),
        "polygon-mainnet" => Some("0x5ea1bd7974c8a611cbab0bdcafcb1d9cc9b3ba5a"),
        "monad-mainnet" => Some("0x77395f3b2e73ae90843717371294fa97cc419d64"),
        _ => None,
    }
}

fn ekubo_v3_core(network: &str) -> Option<&'static str> {
    match network {
        "eth-mainnet" => Some("0x00000000000014aa86c5d3c41765bb24e11bd701"),
        _ => None,
    }
}

fn bytes32_call(signature: &str, value: &str) -> String {
    format!("{}{}", selector(signature), value.trim_start_matches("0x"))
}

fn uint_call(signature: &str, value: u64) -> String {
    format!("{}{:064x}", selector(signature), value)
}

fn evm_requests(
    pools: &[Pool],
    base_id: u64,
    network: &str,
    block: i64,
) -> Vec<RpcRequest<'static>> {
    let mut requests = Vec::with_capacity(pools.len() * 6);
    for (index, pool) in pools.iter().enumerate() {
        let base = base_id + index as u64 * 100;
        let has_db_tokens = pool.token_a_address.is_some() && pool.token_b_address.is_some();
        let calls: Vec<(u64, String)> = match evm_pool_kind(pool) {
            EvmPoolKind::ConstantProduct => {
                let mut calls = Vec::with_capacity(3);
                if !has_db_tokens {
                    calls.push((0, TOKEN0.into()));
                    calls.push((1, TOKEN1.into()));
                }
                calls.push((5, RESERVES.into()));
                calls
            }
            EvmPoolKind::Concentrated => {
                let mut calls = Vec::with_capacity(5);
                if !has_db_tokens {
                    calls.push((0, TOKEN0.into()));
                    calls.push((1, TOKEN1.into()));
                }
                calls.extend([(2, SLOT0.into()), (3, LIQUIDITY.into()), (4, FEE.into())]);
                calls
            }
            EvmPoolKind::Algebra => {
                let mut calls = Vec::with_capacity(4);
                if !has_db_tokens {
                    calls.push((0, TOKEN0.into()));
                    calls.push((1, TOKEN1.into()));
                }
                calls.extend([(2, selector("globalState()")), (3, LIQUIDITY.into())]);
                calls
            }
            EvmPoolKind::KyberElastic => {
                let mut calls = Vec::with_capacity(5);
                if !has_db_tokens {
                    calls.push((0, TOKEN0.into()));
                    calls.push((1, TOKEN1.into()));
                }
                calls.extend([
                    (2, selector("getPoolState()")),
                    (3, selector("getLiquidityState()")),
                    (4, selector("swapFeeUnits()")),
                ]);
                calls
            }
            EvmPoolKind::MaverickV2 => {
                let mut calls = Vec::with_capacity(5);
                if !has_db_tokens {
                    calls.push((0, selector("tokenA()")));
                    calls.push((1, selector("tokenB()")));
                }
                calls.extend([
                    (2, selector("getState()")),
                    (3, format!("{}{:064x}", selector("fee(bool)"), 0)),
                    (4, format!("{}{:064x}", selector("fee(bool)"), 1)),
                ]);
                calls
            }
            EvmPoolKind::UniswapV4 => v4_state_view(network)
                .map(|_| {
                    vec![
                        (0, bytes32_call("getSlot0(bytes32)", &pool.address)),
                        (1, bytes32_call("getLiquidity(bytes32)", &pool.address)),
                    ]
                })
                .unwrap_or_default(),
            EvmPoolKind::Weighted => vec![
                (0, selector("getVault()")),
                (1, selector("getPoolId()")),
                (2, selector("getNormalizedWeights()")),
                (3, selector("getSwapFeePercentage()")),
            ],
            EvmPoolKind::Fluid => vec![
                (
                    0,
                    format!("{}{:064x}", selector("readFromStorage(bytes32)"), 0),
                ),
                (
                    1,
                    format!("{}{:064x}", selector("readFromStorage(bytes32)"), 1),
                ),
            ],
            EvmPoolKind::Curve => {
                let mut curve = vec![(1, selector("fee()")), (2, selector("get_virtual_price()"))];
                for index in 0..8u64 {
                    curve.push((10 + index, uint_call("coins(uint256)", index)));
                    curve.push((30 + index, uint_call("balances(uint256)", index)));
                }
                curve
            }
            EvmPoolKind::LiquidityBook if pool.protocol.eq_ignore_ascii_case("lfj") => {
                let mut calls = Vec::with_capacity(9);
                if !has_db_tokens {
                    calls.push((0, selector("getTokenX()")));
                    calls.push((1, selector("getTokenY()")));
                }
                calls.extend([
                    (2, selector("getReserves()")),
                    (3, selector("getActiveId()")),
                    (4, selector("getBinStep()")),
                ]);
                if !has_db_tokens {
                    calls.push((5, selector("tokenX()")));
                    calls.push((6, selector("tokenY()")));
                }
                calls.extend([
                    (7, selector("getReservesAndId()")),
                    (8, selector("binStep()")),
                ]);
                calls
            }
            EvmPoolKind::LiquidityBook => {
                let mut calls = Vec::with_capacity(5);
                if !has_db_tokens {
                    calls.push((0, selector("getTokenX()")));
                    calls.push((1, selector("getTokenY()")));
                }
                calls.extend([
                    (2, selector("getReserves()")),
                    (3, selector("getActiveId()")),
                    (4, selector("getBinStep()")),
                ]);
                calls
            }
            EvmPoolKind::LegacyLiquidityBook => {
                let mut calls = Vec::with_capacity(3);
                if !has_db_tokens {
                    calls.push((0, selector("tokenX()")));
                    calls.push((1, selector("tokenY()")));
                }
                calls.push((2, selector("getReservesAndId()")));
                calls
            }
            EvmPoolKind::EkuboV3 => ekubo_v3_core(network)
                .map(|_| vec![(2, bytes32_call("sload()", &pool.address))])
                .unwrap_or_default(),
            EvmPoolKind::IziSwap => {
                let mut calls = Vec::with_capacity(4);
                if !has_db_tokens {
                    calls.push((0, selector("tokenX()")));
                    calls.push((1, selector("tokenY()")));
                }
                calls.extend([(2, selector("state()")), (3, selector("fee()"))]);
                calls
            }
            EvmPoolKind::Unsupported => Vec::new(),
        };
        let target = match evm_pool_kind(pool) {
            EvmPoolKind::UniswapV4 => v4_state_view(network).unwrap_or(pool.address.as_str()),
            EvmPoolKind::EkuboV3 => ekubo_v3_core(network).unwrap_or(pool.address.as_str()),
            _ => pool.address.as_str(),
        };
        for (offset, selector) in calls {
            requests.push(RpcRequest {
                jsonrpc: "2.0",
                id: base + offset,
                method: "eth_call",
                params: json!([{"to": target, "data": selector}, format!("0x{block:x}")]),
            });
        }
    }
    requests
}

fn solana_requests(pools: &[Pool], base_id: u64) -> Vec<RpcRequest<'static>> {
    pools
        .iter()
        .enumerate()
        .map(|(index, pool)| RpcRequest {
            jsonrpc: "2.0",
            id: base_id + index as u64,
            method: "getAccountInfo",
            params: json!([pool.address, {"encoding":"base64", "commitment":"confirmed"}]),
        })
        .collect()
}

fn solana_vault_requests(
    pools: &[Pool],
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
) -> Vec<RpcRequest<'static>> {
    let mut requests = Vec::new();
    for (index, pool) in pools.iter().enumerate() {
        if pool.pool_type != "solana_standard" {
            continue;
        }
        let account_data = map
            .get(&(base_id + index as u64))
            .and_then(|response| response.result.as_ref())
            .and_then(|result| result.get("value"))
            .and_then(|value| value.get("data"))
            .and_then(Value::as_array)
            .and_then(|data| data.first())
            .and_then(Value::as_str)
            .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok());
        let (Some(vault0), Some(vault1)) = account_data
            .as_deref()
            .map(|data| solana_pool_vaults(pool, data))
            .unwrap_or_default()
        else {
            continue;
        };
        let (vault0_id, vault1_id) = solana_vault_ids(index, base_id);
        for (id, vault) in [(vault0_id, vault0), (vault1_id, vault1)] {
            requests.push(RpcRequest {
                jsonrpc: "2.0",
                id,
                method: "getAccountInfo",
                params: json!([vault, {"encoding":"base64", "commitment":"confirmed"}]),
            });
        }
    }
    requests
}

fn solana_mint_requests(
    pools: &[Pool],
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
) -> Vec<RpcRequest<'static>> {
    let mut requests = Vec::new();
    for (index, pool) in pools.iter().enumerate() {
        if !matches!(pool.pool_type.as_str(), "solana_dlmm" | "solana_whirlpool") {
            continue;
        }
        let account_data = map
            .get(&(base_id + index as u64))
            .and_then(|response| response.result.as_ref())
            .and_then(|result| result.get("value"))
            .and_then(|value| value.get("data"))
            .and_then(Value::as_array)
            .and_then(|data| data.first())
            .and_then(Value::as_str)
            .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok());
        let (Some(mint0), Some(mint1)) = account_data
            .as_deref()
            .map(|data| solana_pool_mints(pool, data))
            .unwrap_or_default()
        else {
            continue;
        };
        let (mint0_id, mint1_id) = solana_mint_ids(index, base_id);
        for (id, mint) in [(mint0_id, mint0), (mint1_id, mint1)] {
            requests.push(RpcRequest {
                jsonrpc: "2.0",
                id,
                method: "getAccountInfo",
                params: json!([mint, {"encoding":"base64", "commitment":"confirmed"}]),
            });
        }
    }
    requests
}

fn solana_u8(data: &[u8], offset: usize) -> Option<u8> {
    data.get(offset).copied()
}

fn solana_u16(data: &[u8], offset: usize) -> Option<u16> {
    data.get(offset..offset + 2)
        .and_then(|value| value.try_into().ok())
        .map(u16::from_le_bytes)
}

fn solana_u32(data: &[u8], offset: usize) -> Option<u32> {
    data.get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
}

fn solana_i32(data: &[u8], offset: usize) -> Option<i32> {
    data.get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(i32::from_le_bytes)
}

fn solana_u64(data: &[u8], offset: usize) -> Option<u64> {
    data.get(offset..offset + 8)
        .and_then(|value| value.try_into().ok())
        .map(u64::from_le_bytes)
}

fn solana_u128(data: &[u8], offset: usize) -> Option<u128> {
    data.get(offset..offset + 16)
        .and_then(|value| value.try_into().ok())
        .map(u128::from_le_bytes)
}

fn solana_pubkey(data: &[u8], offset: usize) -> Option<String> {
    data.get(offset..offset + 32)
        .map(bs58::encode)
        .map(|value| value.into_string())
}

fn solana_pool_mints(pool: &Pool, data: &[u8]) -> (Option<String>, Option<String>) {
    let Some(offsets) = (match pool.pool_type.as_str() {
        // Anchor discriminator + bump/config/owner precede the two mint keys.
        "solana_clmm" => Some((73, 105)),
        // Anchor discriminator + config/bump/fee fields precede mint_a; the
        // second mint follows fee_growth_global_a.
        "solana_whirlpool" => Some((101, 181)),
        // Raydium CP-Swap stores vaults, LP mint, then the two token mints.
        "solana_standard" if data.len() == 752 => Some((400, 432)),
        // Raydium StableSwap (AMM V5) keeps the mints after the vault and LP
        // key block in the V5 liquidity state layout.
        "solana_standard" if data.len() == 1232 => Some((464, 496)),
        "solana_standard" => Some((168, 200)),
        // Meteora DLMM: discriminator + StaticParameters (32 bytes) +
        // VariableParameters (32 bytes) + pool flags, then token_x/y_mint.
        "solana_dlmm" => Some((88, 120)),
        _ => None,
    }) else {
        return (None, None);
    };
    (
        solana_pubkey(data, offsets.0),
        solana_pubkey(data, offsets.1),
    )
}

fn solana_pool_vaults(pool: &Pool, data: &[u8]) -> (Option<String>, Option<String>) {
    if pool.pool_type != "solana_standard" {
        return (None, None);
    }
    // Raydium CP-Swap PoolState starts with amm_config and pool_creator,
    // followed by token_0_vault and token_1_vault. Offsets include Anchor's
    // eight-byte account discriminator.
    if data.len() == 752 {
        // Legacy Raydium AMM V4: the vault block follows six u128/u64 swap
        // counters at byte 256 and starts at 336.
        (solana_pubkey(data, 336), solana_pubkey(data, 368))
    } else if data.len() == 1232 {
        // Raydium StableSwap (AMM V5): baseVault and quoteVault are the first
        // two public keys after the swap counters.
        (solana_pubkey(data, 368), solana_pubkey(data, 400))
    } else {
        (solana_pubkey(data, 72), solana_pubkey(data, 104))
    }
}

fn solana_vault_ids(index: usize, base_id: u64) -> (u64, u64) {
    let first = base_id + 10_000 + index as u64 * 2;
    (first, first + 1)
}

fn solana_mint_ids(index: usize, base_id: u64) -> (u64, u64) {
    let first = base_id + 20_000 + index as u64 * 2;
    (first, first + 1)
}

fn solana_token_account_amount(map: &HashMap<u64, RpcResponse>, id: u64) -> Option<String> {
    let data = map
        .get(&id)
        .and_then(|response| response.result.as_ref())
        .and_then(|result| result.get("value"))
        .and_then(|value| value.get("data"))
        .and_then(Value::as_array)
        .and_then(|data| data.first())
        .and_then(Value::as_str)
        .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())?;
    let amount = data.get(64..72)?.try_into().ok().map(u64::from_le_bytes)?;
    Some(amount.to_string())
}

fn solana_mint_decimals(map: &HashMap<u64, RpcResponse>, id: u64) -> Option<u8> {
    let data = map
        .get(&id)
        .and_then(|response| response.result.as_ref())
        .and_then(|result| result.get("value"))
        .and_then(|value| value.get("data"))
        .and_then(Value::as_array)
        .and_then(|data| data.first())
        .and_then(Value::as_str)
        .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())?;
    let decimals = *data.get(44)?;
    (decimals <= 36).then_some(decimals)
}

fn decode_solana_pool_state(pool: &Pool, data: &[u8]) -> Option<SolanaDecodedState> {
    // All four account types below are zero-copy/Anchor accounts. The first
    // eight bytes are the account discriminator; offsets therefore include
    // those eight bytes. We intentionally decode only fields needed by the
    // screener. Vault balances and Raydium config fees require follow-up
    // account reads and are kept separate from this account decoder.
    match pool.pool_type.as_str() {
        "solana_clmm" if data.len() >= 273 => {
            let mint_decimals_0 = solana_u8(data, 233)?;
            let mint_decimals_1 = solana_u8(data, 234)?;
            let tick_spacing = solana_u16(data, 235)?;
            let liquidity = solana_u128(data, 237)?;
            let sqrt_price_x64 = solana_u128(data, 253)?;
            let tick_current = solana_i32(data, 269)?;
            Some(SolanaDecodedState {
                decoder_version: "raydium-clmm-account-v1",
                fee_bps: None,
                sqrt_price: Some(sqrt_price_x64.to_string()),
                active_liquidity: Some(liquidity.to_string()),
                state_json: json!({
                    "account_present": true,
                    "mint_decimals_0": mint_decimals_0,
                    "mint_decimals_1": mint_decimals_1,
                    "tick_spacing": tick_spacing,
                    "tick_current": tick_current,
                    "sqrt_price_x64": sqrt_price_x64.to_string(),
                    "liquidity_raw": liquidity.to_string(),
                }),
            })
        }
        "solana_whirlpool" if data.len() >= 85 => {
            let tick_spacing = solana_u16(data, 41)?;
            let fee_rate = solana_u16(data, 45)?;
            let protocol_fee_rate = solana_u16(data, 47)?;
            let liquidity = solana_u128(data, 49)?;
            let sqrt_price_x64 = solana_u128(data, 65)?;
            let tick_current_index = solana_i32(data, 81)?;
            Some(SolanaDecodedState {
                decoder_version: "orca-whirlpool-account-v1",
                // Orca stores fee_rate in hundredths of a basis point. The
                // database fee_bps column is ordinary basis points.
                fee_bps: Some(fee_rate as f64 / 100.0),
                sqrt_price: Some(sqrt_price_x64.to_string()),
                active_liquidity: Some(liquidity.to_string()),
                state_json: json!({
                    "account_present": true,
                    "tick_spacing": tick_spacing,
                    "fee_rate": fee_rate,
                    "protocol_fee_rate": protocol_fee_rate,
                    "tick_current_index": tick_current_index,
                    "sqrt_price_x64": sqrt_price_x64.to_string(),
                    "liquidity_raw": liquidity.to_string(),
                }),
            })
        }
        "solana_standard" if data.len() == 752 => {
            let base_decimals = solana_u64(data, 32)?;
            let quote_decimals = solana_u64(data, 40)?;
            if base_decimals > 36 || quote_decimals > 36 {
                return None;
            }
            let trade_fee_numerator = solana_u64(data, 144)?;
            let trade_fee_denominator = solana_u64(data, 152)?;
            let fee_bps = (trade_fee_denominator > 0)
                .then(|| trade_fee_numerator as f64 / trade_fee_denominator as f64 * 10_000.0);
            Some(SolanaDecodedState {
                decoder_version: "raydium-amm-v4-account-v1",
                fee_bps,
                sqrt_price: None,
                active_liquidity: None,
                state_json: json!({
                    "account_present": true,
                    "layout": "raydium_amm_v4",
                    "mint_decimals_0": base_decimals,
                    "mint_decimals_1": quote_decimals,
                    "trade_fee_numerator": trade_fee_numerator,
                    "trade_fee_denominator": trade_fee_denominator,
                }),
            })
        }
        "solana_standard" if data.len() == 637 => {
            let auth_bump = solana_u8(data, 328)?;
            let status = solana_u8(data, 329)?;
            let lp_mint_decimals = solana_u8(data, 330)?;
            let mint_0_decimals = solana_u8(data, 331)?;
            let mint_1_decimals = solana_u8(data, 332)?;
            if lp_mint_decimals > 36 || mint_0_decimals > 36 || mint_1_decimals > 36 {
                return None;
            }
            let lp_supply = solana_u64(data, 333)?;
            let protocol_fees_token_0 = solana_u64(data, 341)?;
            let protocol_fees_token_1 = solana_u64(data, 349)?;
            let fund_fees_token_0 = solana_u64(data, 357)?;
            let fund_fees_token_1 = solana_u64(data, 365)?;
            let open_time = solana_u64(data, 373)?;
            let recent_epoch = solana_u64(data, 381)?;
            Some(SolanaDecodedState {
                decoder_version: "raydium-cpmm-account-v1",
                fee_bps: None,
                sqrt_price: None,
                active_liquidity: Some(lp_supply.to_string()),
                state_json: json!({
                    "account_present": true,
                    "auth_bump": auth_bump,
                    "status": status,
                    "lp_mint_decimals": lp_mint_decimals,
                    "mint_0_decimals": mint_0_decimals,
                    "mint_1_decimals": mint_1_decimals,
                    "lp_supply_raw": lp_supply.to_string(),
                    "protocol_fees_token_0_raw": protocol_fees_token_0.to_string(),
                    "protocol_fees_token_1_raw": protocol_fees_token_1.to_string(),
                    "fund_fees_token_0_raw": fund_fees_token_0.to_string(),
                    "fund_fees_token_1_raw": fund_fees_token_1.to_string(),
                    "open_time": open_time,
                    "recent_epoch": recent_epoch,
                }),
            })
        }
        "solana_standard" if data.len() == 1232 => {
            // Raydium AMM V5 / StableSwap. The account layout is the V5
            // liquidity state layout: decimal fields at 40/48, trade fee at
            // 176/184, vaults at 368/400 and mints at 464/496.
            let base_decimals = solana_u64(data, 40)?;
            let quote_decimals = solana_u64(data, 48)?;
            if base_decimals > 36 || quote_decimals > 36 {
                return None;
            }
            let trade_fee_numerator = solana_u64(data, 176)?;
            let trade_fee_denominator = solana_u64(data, 184)?;
            let fee_bps = (trade_fee_denominator > 0)
                .then(|| trade_fee_numerator as f64 / trade_fee_denominator as f64 * 10_000.0);
            Some(SolanaDecodedState {
                decoder_version: "raydium-stableswap-account-v1",
                fee_bps,
                sqrt_price: None,
                active_liquidity: None,
                state_json: json!({
                    "account_present": true,
                    "layout": "raydium_stableswap_v5",
                    "mint_decimals_0": base_decimals,
                    "mint_decimals_1": quote_decimals,
                    "trade_fee_numerator": trade_fee_numerator,
                    "trade_fee_denominator": trade_fee_denominator,
                }),
            })
        }
        "solana_dlmm" if data.len() >= 232 => {
            let base_factor = solana_u16(data, 8)?;
            let variable_fee_control = solana_u32(data, 16)?;
            let max_volatility_accumulator = solana_u32(data, 20)?;
            let protocol_share = solana_u16(data, 32)?;
            let active_id = solana_i32(data, 76)?;
            let bin_step = solana_u16(data, 80)?;
            let pair_type = solana_u8(data, 75)?;
            let status = solana_u8(data, 82)?;
            let base_fee_bps = base_factor as f64 * bin_step as f64 / 10_000.0;
            Some(SolanaDecodedState {
                decoder_version: "meteora-dlmm-lbpair-account-v1",
                // DLMM's account stores the static base-fee parameters. The
                // dynamic fee component depends on the variable parameters;
                // expose the conservative base fee here rather than label it
                // as the complete current fee.
                fee_bps: Some(base_fee_bps),
                sqrt_price: None,
                active_liquidity: None,
                state_json: json!({
                    "account_present": true,
                    "base_factor": base_factor,
                    "bin_step": bin_step,
                    "active_id": active_id,
                    "pair_type": pair_type,
                    "status": status,
                    "protocol_share": protocol_share,
                    "variable_fee_control": variable_fee_control,
                    "max_volatility_accumulator": max_volatility_accumulator,
                    "base_fee_bps": base_fee_bps,
                }),
            })
        }
        _ => None,
    }
}

fn parse_curve_state(
    pool: &Pool,
    endpoint: &Endpoint,
    block: i64,
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
    checked_at: i64,
) -> StateRow {
    let mut assets = Vec::new();
    for index in 0..8u64 {
        let address = result_hex(map, base_id + 10 + index).and_then(|data| address_word(&data));
        let balance = result_hex(map, base_id + 30 + index)
            .and_then(|data| word(&data, 0))
            .map(|value| value.to_string());
        let (Some(address), Some(balance)) = (address, balance) else {
            break;
        };
        assets.push(json!({
            "asset_index": index,
            "token_address": address,
            "balance_raw": balance,
        }));
    }
    let fee_bps = result_hex(map, base_id + 1)
        .and_then(|data| word(&data, 0))
        .map(|value| value as f64 / 1_000_000.0);
    let virtual_price = result_hex(map, base_id + 2)
        .and_then(|data| word(&data, 0))
        .map(|value| value.to_string());
    let complete = assets.len() >= 2;
    let token0 = assets
        .first()
        .and_then(|asset| asset.get("token_address"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let token1 = assets
        .get(1)
        .and_then(|asset| asset.get("token_address"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let reserve0 = assets
        .first()
        .and_then(|asset| asset.get("balance_raw"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let reserve1 = assets
        .get(1)
        .and_then(|asset| asset.get("balance_raw"))
        .and_then(Value::as_str)
        .map(str::to_string);
    StateRow {
        pool_address: pool.address.clone(),
        chain: pool.chain.clone(),
        network: endpoint.network.clone(),
        protocol: endpoint.protocol.clone(),
        checked_at,
        state_block: Some(block),
        account_exists: None,
        state_status: if complete {
            "live".into()
        } else {
            "rpc_error".into()
        },
        pool_family: "stable_swap".into(),
        decoder_version: "curve-pool-v1".into(),
        state_error: if complete {
            None
        } else {
            Some("Curve returned fewer than two assets".into())
        },
        state_json: Some(
            json!({
                "fee_bps": fee_bps,
                "virtual_price_raw": virtual_price,
                "asset_count": assets.len(),
            })
            .to_string(),
        ),
        assets_json: Some(Value::Array(assets).to_string()),
        quote_supported: false,
        token0_address: token0,
        token1_address: token1,
        fee_bps,
        reserve0_raw: reserve0,
        reserve1_raw: reserve1,
        sqrt_price_x96: None,
        active_liquidity_raw: None,
        spot_price_usd: None,
        account_owner: None,
        account_data_bytes: None,
    }
}

fn parse_liquidity_book_state(
    pool: &Pool,
    endpoint: &Endpoint,
    block: i64,
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
    checked_at: i64,
) -> StateRow {
    let is_lfj = pool.protocol.eq_ignore_ascii_case("lfj")
        || pool.protocol.to_ascii_lowercase().starts_with("lfj ")
        || pool.protocol.to_ascii_lowercase().contains("merchant moe");
    let token0 = result_hex(map, base_id)
        .or_else(|| result_hex(map, base_id + 5))
        .and_then(|data| address_word(&data))
        .or_else(|| is_lfj.then(|| pool.token_a_address.clone()).flatten());
    let token1 = result_hex(map, base_id + 1)
        .or_else(|| result_hex(map, base_id + 6))
        .and_then(|data| address_word(&data))
        .or_else(|| is_lfj.then(|| pool.token_b_address.clone()).flatten());
    let reserves = result_hex(map, base_id + 2).or_else(|| result_hex(map, base_id + 7));
    let reserve0 = reserves
        .as_deref()
        .and_then(|data| word(data, 0))
        .map(|value| value.to_string());
    let reserve1 = reserves
        .as_deref()
        .and_then(|data| word(data, 1))
        .map(|value| value.to_string());
    let active_id_candidate = result_hex(map, base_id + 3)
        .and_then(|data| word(&data, 0))
        .or_else(|| result_hex(map, base_id + 2).and_then(|data| word(&data, 2)))
        .or_else(|| result_hex(map, base_id + 7).and_then(|data| word(&data, 2)))
        .map(|value| value.to_string());
    // A legacy LFJ/Joe V1 pair can answer a reserve-style call whose third
    // word is a uint32 timestamp, not a Liquidity Book uint24 activeId. Do
    // not persist that timestamp as an active bin; it would later be
    // interpreted as a price exponent.
    let active_id = active_id_candidate.and_then(|value| {
        value
            .parse::<u64>()
            .ok()
            .filter(|value| *value <= 0x00ff_ffff)
            .map(|value| value.to_string())
    });
    let bin_step = result_hex(map, base_id + 4)
        .and_then(|data| word(&data, 0))
        .or_else(|| result_hex(map, base_id + 8).and_then(|data| word(&data, 0)))
        .map(|value| value as f64);
    let complete = reserve0.is_some()
        && reserve1.is_some()
        && ((active_id.is_some() && bin_step.is_some()) || (is_lfj && bin_step.is_none()));
    let legacy_reserve_pair = is_lfj && bin_step.is_none();
    StateRow {
        pool_address: pool.address.clone(),
        chain: pool.chain.clone(),
        network: endpoint.network.clone(),
        protocol: endpoint.protocol.clone(),
        checked_at,
        state_block: Some(block),
        account_exists: None,
        state_status: if complete { "live".into() } else { "rpc_error".into() },
        pool_family: if legacy_reserve_pair {
            "constant_product_v2".into()
        } else {
            "liquidity_book".into()
        },
        decoder_version: if legacy_reserve_pair {
            "lfj-v1-reserve-pair-v1"
        } else {
            "lbpair-v2-raw-v1"
        }
        .into(),
        state_error: if complete {
            None
        } else {
            error_text(map, &[base_id, base_id + 1, base_id + 2, base_id + 3, base_id + 4, base_id + 5, base_id + 6, base_id + 7, base_id + 8])
                .or_else(|| Some("required Liquidity Book state call returned no result".into()))
        },
        state_json: Some(
            json!({
                "active_id": active_id,
                "bin_step": bin_step,
                "spot_price_source": if legacy_reserve_pair {
                    "reserve_ratio"
                } else {
                    "active_bin"
                },
                "reserve0_raw": reserve0,
                "reserve1_raw": reserve1,
            })
            .to_string(),
        ),
        assets_json: Some(
            json!([
                {"asset_index": 0, "token_address": token0.clone(), "balance_raw": reserve0.clone()},
                {"asset_index": 1, "token_address": token1.clone(), "balance_raw": reserve1.clone()},
            ])
            .to_string(),
        ),
        quote_supported: false,
        token0_address: token0,
        token1_address: token1,
        fee_bps: None,
        reserve0_raw: reserve0,
        reserve1_raw: reserve1,
        sqrt_price_x96: None,
        active_liquidity_raw: None,
        spot_price_usd: None,
        account_owner: None,
        account_data_bytes: None,
    }
}

fn parse_legacy_liquidity_book_state(
    pool: &Pool,
    endpoint: &Endpoint,
    block: i64,
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
    checked_at: i64,
) -> StateRow {
    let token0 = result_hex(map, base_id)
        .and_then(|data| address_word(&data))
        .or_else(|| pool.token_a_address.clone());
    let token1 = result_hex(map, base_id + 1)
        .and_then(|data| address_word(&data))
        .or_else(|| pool.token_b_address.clone());
    let reserves = result_hex(map, base_id + 2);
    let reserve0 = reserves
        .as_deref()
        .and_then(|data| word(data, 0))
        .map(|value| value.to_string());
    let reserve1 = reserves
        .as_deref()
        .and_then(|data| word(data, 1))
        .map(|value| value.to_string());
    let active_id = reserves
        .as_deref()
        .and_then(|data| word(data, 2))
        .map(|value| value.to_string());
    let complete = token0.is_some()
        && token1.is_some()
        && reserve0.is_some()
        && reserve1.is_some()
        && active_id.is_some();
    StateRow {
        pool_address: pool.address.clone(),
        chain: pool.chain.clone(),
        network: endpoint.network.clone(),
        protocol: endpoint.protocol.clone(),
        checked_at,
        state_block: Some(block),
        account_exists: None,
        state_status: if complete { "live".into() } else { "rpc_error".into() },
        pool_family: "liquidity_book".into(),
        decoder_version: "lbpair-legacy-v1".into(),
        state_error: if complete {
            None
        } else {
            error_text(map, &[base_id, base_id + 1, base_id + 2])
                .or_else(|| Some("required legacy Liquidity Book state call returned no result".into()))
        },
        state_json: Some(
            json!({
                "active_id": active_id,
                "reserve0_raw": reserve0,
                "reserve1_raw": reserve1,
            })
            .to_string(),
        ),
        assets_json: Some(
            json!([
                {"asset_index": 0, "token_address": token0.clone(), "balance_raw": reserve0.clone()},
                {"asset_index": 1, "token_address": token1.clone(), "balance_raw": reserve1.clone()},
            ])
            .to_string(),
        ),
        quote_supported: false,
        token0_address: token0,
        token1_address: token1,
        fee_bps: None,
        reserve0_raw: reserve0,
        reserve1_raw: reserve1,
        sqrt_price_x96: None,
        active_liquidity_raw: None,
        spot_price_usd: None,
        account_owner: None,
        account_data_bytes: None,
    }
}

fn parse_algebra_state(
    pool: &Pool,
    endpoint: &Endpoint,
    block: i64,
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
    checked_at: i64,
) -> StateRow {
    let token0 = result_hex(map, base_id)
        .and_then(|data| address_word(&data))
        .or_else(|| pool.token_a_address.clone());
    let token1 = result_hex(map, base_id + 1)
        .and_then(|data| address_word(&data))
        .or_else(|| pool.token_b_address.clone());
    let global_state = result_hex(map, base_id + 2);
    let liquidity = result_hex(map, base_id + 3)
        .and_then(|data| word(&data, 0))
        .map(|value| value.to_string());
    let sqrt_price_x96 = global_state
        .as_deref()
        .and_then(|data| word_f64(data, 0))
        .map(|value| value.to_string());
    let tick = global_state
        .as_deref()
        .and_then(|data| signed_word(data, 1, 24));
    let fee_bps = global_state
        .as_deref()
        .and_then(|data| word(data, 2))
        .map(|value| value as f64 / 100.0);
    let plugin_config = global_state
        .as_deref()
        .and_then(|data| word(data, 3))
        .map(|value| value.to_string());
    let community_fee = global_state
        .as_deref()
        .and_then(|data| word(data, 4))
        .map(|value| value.to_string());
    let unlocked = global_state
        .as_deref()
        .and_then(|data| word(data, 5))
        .map(|value| value != 0);
    let complete =
        token0.is_some() && token1.is_some() && global_state.is_some() && liquidity.is_some();
    StateRow {
        pool_address: pool.address.clone(),
        chain: pool.chain.clone(),
        network: endpoint.network.clone(),
        protocol: endpoint.protocol.clone(),
        checked_at,
        state_block: Some(block),
        account_exists: Some(complete),
        state_status: if complete { "live".into() } else { "rpc_error".into() },
        pool_family: "algebra_v3".into(),
        decoder_version: "algebra-global-state-v1".into(),
        state_error: if complete {
            None
        } else {
            error_text(map, &[base_id, base_id + 1, base_id + 2, base_id + 3, base_id + 4])
                .or_else(|| Some("required Algebra pool state call returned no result".into()))
        },
        state_json: Some(
            json!({
                "token0": token0,
                "token1": token1,
                "sqrt_price_x96": sqrt_price_x96,
                "tick": tick,
                "last_fee_raw": global_state.as_deref().and_then(|data| word(data, 2)).map(|value| value.to_string()),
                "plugin_config": plugin_config,
                "community_fee": community_fee,
                "unlocked": unlocked,
                "active_liquidity_raw": liquidity,
            })
            .to_string(),
        ),
        assets_json: Some(
            json!([
                {"asset_index": 0, "token_address": pool.token_a_address},
                {"asset_index": 1, "token_address": pool.token_b_address}
            ])
            .to_string(),
        ),
        quote_supported: false,
        token0_address: token0.clone(),
        token1_address: token1.clone(),
        fee_bps,
        reserve0_raw: None,
        reserve1_raw: None,
        sqrt_price_x96,
        active_liquidity_raw: liquidity,
        spot_price_usd: match (token0, token1, global_state) {
            (Some(a), Some(b), Some(slot)) => spot_price(pool, &a, &b, &slot),
            _ => None,
        },
        account_owner: None,
        account_data_bytes: None,
    }
}

fn parse_kyber_elastic_state(
    pool: &Pool,
    endpoint: &Endpoint,
    block: i64,
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
    checked_at: i64,
) -> StateRow {
    let token0 = result_hex(map, base_id)
        .and_then(|data| address_word(&data))
        .or_else(|| pool.token_a_address.clone());
    let token1 = result_hex(map, base_id + 1)
        .and_then(|data| address_word(&data))
        .or_else(|| pool.token_b_address.clone());
    let pool_state = result_hex(map, base_id + 2);
    let liquidity_state = result_hex(map, base_id + 3);
    let fee_bps = result_hex(map, base_id + 4)
        .and_then(|data| word(&data, 0))
        .map(|value| value as f64);
    let sqrt_price_x96 = pool_state
        .as_deref()
        .and_then(|data| word_f64(data, 0))
        .map(|value| value.to_string());
    let current_tick = pool_state
        .as_deref()
        .and_then(|data| signed_word(data, 1, 24));
    let nearest_tick = pool_state
        .as_deref()
        .and_then(|data| signed_word(data, 2, 24));
    let locked = pool_state
        .as_deref()
        .and_then(|data| word(data, 3))
        .map(|value| value != 0);
    let base_liquidity = liquidity_state
        .as_deref()
        .and_then(|data| word(data, 0))
        .map(|value| value.to_string());
    let reinvest_liquidity = liquidity_state
        .as_deref()
        .and_then(|data| word(data, 1))
        .map(|value| value.to_string());
    let complete = token0.is_some()
        && token1.is_some()
        && pool_state.is_some()
        && liquidity_state.is_some()
        && fee_bps.is_some();
    StateRow {
        pool_address: pool.address.clone(),
        chain: pool.chain.clone(),
        network: endpoint.network.clone(),
        protocol: endpoint.protocol.clone(),
        checked_at,
        state_block: Some(block),
        account_exists: Some(complete),
        state_status: if complete {
            "live".into()
        } else {
            "rpc_error".into()
        },
        pool_family: "concentrated_v3".into(),
        decoder_version: "kyberswap-elastic-pool-state-v1".into(),
        state_error: if complete {
            None
        } else {
            error_text(
                map,
                &[base_id, base_id + 1, base_id + 2, base_id + 3, base_id + 4],
            )
            .or_else(|| Some("required KyberSwap Elastic state call returned no result".into()))
        },
        state_json: Some(
            json!({
                "token0": token0,
                "token1": token1,
                "sqrt_price_x96": sqrt_price_x96,
                "current_tick": current_tick,
                "nearest_current_tick": nearest_tick,
                "locked": locked,
                "base_liquidity_raw": base_liquidity,
                "reinvest_liquidity_raw": reinvest_liquidity,
                "swap_fee_units": fee_bps,
            })
            .to_string(),
        ),
        assets_json: Some(
            json!([
                {"asset_index": 0, "token_address": token0.clone()},
                {"asset_index": 1, "token_address": token1.clone()}
            ])
            .to_string(),
        ),
        quote_supported: false,
        token0_address: token0,
        token1_address: token1,
        fee_bps,
        reserve0_raw: None,
        reserve1_raw: None,
        sqrt_price_x96,
        active_liquidity_raw: base_liquidity,
        spot_price_usd: None,
        account_owner: None,
        account_data_bytes: None,
    }
}

fn parse_maverick_v2_state(
    pool: &Pool,
    endpoint: &Endpoint,
    block: i64,
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
    checked_at: i64,
) -> StateRow {
    let token0 = result_hex(map, base_id)
        .and_then(|data| address_word(&data))
        .or_else(|| pool.token_a_address.clone());
    let token1 = result_hex(map, base_id + 1)
        .and_then(|data| address_word(&data))
        .or_else(|| pool.token_b_address.clone());
    let state = result_hex(map, base_id + 2);
    let fee_a_raw = result_hex(map, base_id + 3)
        .and_then(|data| word(&data, 0))
        .map(|value| value.to_string());
    let fee_b_raw = result_hex(map, base_id + 4)
        .and_then(|data| word(&data, 0))
        .map(|value| value.to_string());
    let active_tick = state.as_deref().and_then(|data| signed_word(data, 5, 32));
    let is_locked = state
        .as_deref()
        .and_then(|data| word(data, 6))
        .map(|value| value != 0);
    let bin_counter = state
        .as_deref()
        .and_then(|data| word(data, 7))
        .map(|value| value.to_string());
    let protocol_fee_ratio = state
        .as_deref()
        .and_then(|data| word(data, 8))
        .map(|value| value.to_string());
    let reserve0 = state
        .as_deref()
        .and_then(|data| word(data, 0))
        .map(|value| value.to_string());
    let reserve1 = state
        .as_deref()
        .and_then(|data| word(data, 1))
        .map(|value| value.to_string());
    let fee_bps = fee_a_raw
        .as_deref()
        .and_then(|value| value.parse::<f64>().ok())
        .map(|value| value / 1.0e14);
    let complete = token0.is_some()
        && token1.is_some()
        && state.is_some()
        && fee_a_raw.is_some()
        && fee_b_raw.is_some();
    StateRow {
        pool_address: pool.address.clone(),
        chain: pool.chain.clone(),
        network: endpoint.network.clone(),
        protocol: endpoint.protocol.clone(),
        checked_at,
        state_block: Some(block),
        account_exists: Some(complete),
        state_status: if complete { "live".into() } else { "rpc_error".into() },
        pool_family: "maverick_v2".into(),
        decoder_version: "maverick-v2-state-v1".into(),
        state_error: if complete {
            None
        } else {
            error_text(map, &[base_id, base_id + 1, base_id + 2, base_id + 3, base_id + 4])
                .or_else(|| Some("required Maverick V2 state call returned no result".into()))
        },
        state_json: Some(
            json!({
                "active_tick": active_tick,
                "is_locked": is_locked,
                "last_twa_d8": state.as_deref().and_then(|data| signed_word(data, 2, 64)),
                "last_log_price_d8": state.as_deref().and_then(|data| signed_word(data, 3, 64)),
                "last_timestamp": state.as_deref().and_then(|data| word(data, 4)).map(|value| value.to_string()),
                "bin_counter": bin_counter,
                "protocol_fee_ratio": protocol_fee_ratio,
                "fee_a_raw": fee_a_raw,
                "fee_b_raw": fee_b_raw,
                "reserve0_raw": reserve0,
                "reserve1_raw": reserve1,
                "state_raw": state,
            })
            .to_string(),
        ),
        assets_json: Some(
            json!([
                {"asset_index": 0, "token_address": pool.token_a_address, "balance_raw": reserve0},
                {"asset_index": 1, "token_address": pool.token_b_address, "balance_raw": reserve1}
            ])
            .to_string(),
        ),
        quote_supported: false,
        token0_address: token0,
        token1_address: token1,
        fee_bps,
        reserve0_raw: reserve0,
        reserve1_raw: reserve1,
        sqrt_price_x96: None,
        active_liquidity_raw: None,
        spot_price_usd: None,
        account_owner: None,
        account_data_bytes: None,
    }
}

fn signed_word(data: &str, index: usize, bits: u32) -> Option<i64> {
    let value = word(data, index)?;
    let mask = (1u128 << bits) - 1;
    let value = value & mask;
    let sign = 1u128 << (bits - 1);
    if value & sign != 0 {
        Some((value as i128 - (1i128 << bits)) as i64)
    } else {
        Some(value as i64)
    }
}

fn parse_uniswap_v4_state(
    pool: &Pool,
    endpoint: &Endpoint,
    block: i64,
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
    checked_at: i64,
) -> StateRow {
    let slot0 = result_hex(map, base_id);
    let liquidity = result_hex(map, base_id + 1)
        .and_then(|data| word(&data, 0))
        .map(|value| value.to_string());
    let sqrt_price_x96 = slot0
        .as_deref()
        .and_then(|data| word_f64(data, 0))
        .map(|value| value.to_string());
    let tick = slot0.as_deref().and_then(|data| signed_word(data, 1, 24));
    let protocol_fee = slot0
        .as_deref()
        .and_then(|data| word(data, 2))
        .map(|value| value.to_string());
    let lp_fee_raw = slot0
        .as_deref()
        .and_then(|data| word(data, 3))
        .map(|value| value.to_string());
    // Uniswap V4 stores LP fees in hundredths of a bip (1_000_000 = 100%).
    // 0x800000 is the dynamic-fee marker, not a 83886.08 bps fee. Keep the
    // raw value for diagnosis, but expose no static fee for that marker.
    let fee_bps = slot0
        .as_deref()
        .and_then(|data| word(data, 3))
        .and_then(uniswap_v4_fee_bps);
    // Fee is metadata; a missing fee must not make a valid V4 price state
    // look like an RPC decode failure.
    let complete = slot0.is_some() && liquidity.is_some();
    let token0 = pool.token_a_address.clone();
    let token1 = pool.token_b_address.clone();
    StateRow {
        pool_address: pool.address.clone(),
        chain: pool.chain.clone(),
        network: endpoint.network.clone(),
        protocol: endpoint.protocol.clone(),
        checked_at,
        state_block: Some(block),
        account_exists: Some(complete),
        state_status: if complete {
            "live".into()
        } else {
            "rpc_error".into()
        },
        pool_family: "uniswap_v4".into(),
        decoder_version: "uniswap-v4-state-view-v1".into(),
        state_error: if complete {
            None
        } else {
            error_text(map, &[base_id, base_id + 1])
                .or_else(|| Some("Uniswap V4 StateView returned incomplete state".into()))
        },
        state_json: Some(
            json!({
                "pool_id": pool.address,
                "state_view": v4_state_view(&endpoint.network),
                "sqrt_price_x96": sqrt_price_x96,
                "tick": tick,
                "protocol_fee_raw": protocol_fee,
                "lp_fee_raw": lp_fee_raw,
                "active_liquidity_raw": liquidity,
            })
            .to_string(),
        ),
        assets_json: Some(
            json!([
                {"asset_index": 0, "token_address": token0.clone()},
                {"asset_index": 1, "token_address": token1.clone()}
            ])
            .to_string(),
        ),
        quote_supported: false,
        token0_address: token0.clone(),
        token1_address: token1.clone(),
        fee_bps,
        reserve0_raw: None,
        reserve1_raw: None,
        sqrt_price_x96: sqrt_price_x96.clone(),
        active_liquidity_raw: liquidity.clone(),
        spot_price_usd: match (token0, token1, slot0) {
            (Some(a), Some(b), Some(slot)) => spot_price(pool, &a, &b, &slot),
            _ => None,
        },
        account_owner: v4_state_view(&endpoint.network).map(str::to_string),
        account_data_bytes: None,
    }
}

fn uniswap_v4_fee_bps(raw: u128) -> Option<f64> {
    (raw <= 1_000_000).then_some(raw as f64 / 100.0)
}

fn dynamic_uints(data: &str) -> Option<Vec<String>> {
    let offset_words = (word(data, 0)? / 32) as usize;
    let count = word(data, offset_words)? as usize;
    (0..count)
        .map(|index| word(data, offset_words + index + 1).map(|value| value.to_string()))
        .collect()
}

fn parse_weighted_state(
    pool: &Pool,
    endpoint: &Endpoint,
    block: i64,
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
    checked_at: i64,
) -> StateRow {
    let vault = result_hex(map, base_id).and_then(|data| address_word(&data));
    let pool_id = result_hex(map, base_id + 1);
    let weights = result_hex(map, base_id + 2).and_then(|data| dynamic_uints(&data));
    let fee_raw = result_hex(map, base_id + 3)
        .and_then(|data| word(&data, 0))
        .map(|value| value.to_string());
    let fee_bps = fee_raw
        .as_deref()
        .and_then(|value| value.parse::<f64>().ok())
        .map(|value| value / 1.0e14);
    // WeightedPool exposes normalized weights; StablePool and
    // ComposableStablePool intentionally do not. Vault/pool id/fee are the
    // common Balancer state contract and are sufficient to mark the pool
    // reachable without pretending a stable invariant has weights.
    let complete = vault.is_some() && pool_id.is_some();
    let stable_variant = weights.is_none();
    StateRow {
        pool_address: pool.address.clone(),
        chain: pool.chain.clone(),
        network: endpoint.network.clone(),
        protocol: endpoint.protocol.clone(),
        checked_at,
        state_block: Some(block),
        account_exists: Some(complete),
        state_status: if complete {
            "live".into()
        } else {
            "rpc_error".into()
        },
        pool_family: if stable_variant {
            "balancer_stable_or_composable".into()
        } else {
            "weighted".into()
        },
        decoder_version: "balancer-vault-pool-state-v2".into(),
        state_error: if complete {
            None
        } else {
            error_text(
                map,
                &[base_id, base_id + 1, base_id + 2, base_id + 3, base_id + 4],
            )
            .or_else(|| Some("Balancer WeightedPool returned incomplete state".into()))
        },
        state_json: Some(
            json!({
                "vault": vault,
                "pool_id": pool_id,
                "normalized_weights_raw": weights,
                "stable_variant": stable_variant,
                "swap_fee_percentage_raw": fee_raw,
            })
            .to_string(),
        ),
        assets_json: Some(
            json!([
                {"asset_index": 0, "token_address": pool.token_a_address},
                {"asset_index": 1, "token_address": pool.token_b_address}
            ])
            .to_string(),
        ),
        quote_supported: false,
        token0_address: pool.token_a_address.clone(),
        token1_address: pool.token_b_address.clone(),
        fee_bps,
        reserve0_raw: None,
        reserve1_raw: None,
        sqrt_price_x96: None,
        active_liquidity_raw: None,
        spot_price_usd: None,
        account_owner: vault,
        account_data_bytes: None,
    }
}

fn parse_fluid_state(
    pool: &Pool,
    endpoint: &Endpoint,
    block: i64,
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
    checked_at: i64,
) -> StateRow {
    let dex_variables = result_hex(map, base_id);
    let dex_variables2 = result_hex(map, base_id + 1);
    // Fluid's packed slots are uint256 values. The fields used here live in
    // the low 128 bits; parsing the whole ABI word as u128 silently dropped
    // valid states whenever any high bits were set.
    let variables = dex_variables.as_deref().and_then(low_word);
    let variables2 = dex_variables2.as_deref().and_then(low_word);
    let last_stored_price = variables.map(|value| (value >> 40) & ((1u128 << 40) - 1));
    // Fluid stores poolFee in bits [2, 18] and expresses it in hundredths
    // of a basis point: 5000 raw = 50 bps = 0.5%.
    let fee_raw = variables2.map(|value| (value >> 2) & ((1u128 << 17) - 1));
    let complete = dex_variables.is_some() && dex_variables2.is_some();
    StateRow {
        pool_address: pool.address.clone(),
        chain: pool.chain.clone(),
        network: endpoint.network.clone(),
        protocol: endpoint.protocol.clone(),
        checked_at,
        state_block: Some(block),
        account_exists: Some(complete),
        state_status: if complete {
            "live".into()
        } else {
            "rpc_error".into()
        },
        pool_family: "fluid_dex_t1".into(),
        decoder_version: "fluid-dex-storage-v1".into(),
        state_error: if complete {
            None
        } else {
            error_text(map, &[base_id, base_id + 1])
                .or_else(|| Some("Fluid DEX storage state returned incomplete data".into()))
        },
        state_json: Some(
            json!({
                "dex_variables_raw": dex_variables,
                "dex_variables2_raw": dex_variables2,
                "last_stored_price_packed": last_stored_price.map(|value| value.to_string()),
                "fee_raw": fee_raw.map(|value| value.to_string()),
            })
            .to_string(),
        ),
        assets_json: Some(
            json!([
                {"asset_index": 0, "token_address": pool.token_a_address},
                {"asset_index": 1, "token_address": pool.token_b_address}
            ])
            .to_string(),
        ),
        quote_supported: false,
        token0_address: pool.token_a_address.clone(),
        token1_address: pool.token_b_address.clone(),
        fee_bps: fee_raw.map(|value| value as f64 / 100.0),
        reserve0_raw: None,
        reserve1_raw: None,
        sqrt_price_x96: None,
        active_liquidity_raw: None,
        spot_price_usd: None,
        account_owner: Some(pool.address.clone()),
        account_data_bytes: None,
    }
}

fn parse_ekubo_v3_state(
    pool: &Pool,
    endpoint: &Endpoint,
    block: i64,
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
    checked_at: i64,
) -> StateRow {
    let packed = result_hex(map, base_id + 2);
    let fields = packed
        .as_deref()
        .and_then(|value| value.strip_prefix("0x"))
        .filter(|value| value.len() == 64)
        .and_then(|value| {
            let sqrt_ratio = u128::from_str_radix(&value[..24], 16).ok()?;
            let tick = u32::from_str_radix(&value[24..32], 16).ok()? as i32;
            let liquidity = u128::from_str_radix(&value[32..], 16).ok()?;
            Some((sqrt_ratio, tick, liquidity))
        });
    let complete = fields.is_some_and(|(sqrt_ratio, _, liquidity)| sqrt_ratio > 0 && liquidity > 0);
    let (sqrt_ratio, tick, liquidity) = fields
        .map(|(sqrt_ratio, tick, liquidity)| {
            (
                Some(sqrt_ratio.to_string()),
                Some(tick),
                Some(liquidity.to_string()),
            )
        })
        .unwrap_or((None, None, None));
    let core = ekubo_v3_core(&endpoint.network).map(str::to_string);
    StateRow {
        pool_address: pool.address.clone(),
        chain: pool.chain.clone(),
        network: endpoint.network.clone(),
        protocol: endpoint.protocol.clone(),
        checked_at,
        state_block: Some(block),
        account_exists: Some(complete),
        state_status: if complete {
            "live".into()
        } else {
            "rpc_error".into()
        },
        pool_family: "concentrated_v3".into(),
        decoder_version: "ekubo-v3-storage-v1".into(),
        state_error: if complete {
            None
        } else {
            error_text(map, &[base_id + 2])
                .or_else(|| Some("Ekubo V3 pool storage is missing or uninitialized".into()))
        },
        state_json: Some(
            json!({
                "packed_pool_state": packed,
                "sqrt_ratio_dynamic_raw": sqrt_ratio,
                "tick": tick,
                "liquidity_raw": liquidity,
            })
            .to_string(),
        ),
        assets_json: Some(
            json!([
                {"asset_index": 0, "token_address": pool.token_a_address},
                {"asset_index": 1, "token_address": pool.token_b_address}
            ])
            .to_string(),
        ),
        quote_supported: false,
        token0_address: pool.token_a_address.clone(),
        token1_address: pool.token_b_address.clone(),
        fee_bps: None,
        reserve0_raw: None,
        reserve1_raw: None,
        sqrt_price_x96: None,
        active_liquidity_raw: liquidity,
        spot_price_usd: None,
        account_owner: core,
        account_data_bytes: None,
    }
}

fn parse_iziswap_state(
    pool: &Pool,
    endpoint: &Endpoint,
    block: i64,
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
    checked_at: i64,
) -> StateRow {
    let token0 = pool
        .token_a_address
        .clone()
        .or_else(|| result_hex(map, base_id).and_then(|data| address_word(&data)));
    let token1 = pool
        .token_b_address
        .clone()
        .or_else(|| result_hex(map, base_id + 1).and_then(|data| address_word(&data)));
    let state = result_hex(map, base_id + 2);
    let fee_raw = result_hex(map, base_id + 3).and_then(|data| word(&data, 0));
    let sqrt_price_x96 = state
        .as_deref()
        .and_then(|data| word_f64(data, 0))
        .map(|value| value.to_string());
    let current_point = state.as_deref().and_then(|data| signed_word(data, 1, 24));
    let liquidity = state
        .as_deref()
        .and_then(|data| word(data, 5))
        .map(|value| value.to_string());
    let liquidity_x = state
        .as_deref()
        .and_then(|data| word(data, 6))
        .map(|value| value.to_string());
    let fee_bps = fee_raw.map(|value| value as f64 / 100.0);
    let complete = token0.is_some()
        && token1.is_some()
        && sqrt_price_x96.is_some()
        && liquidity.is_some()
        && fee_bps.is_some();
    StateRow {
        pool_address: pool.address.clone(),
        chain: pool.chain.clone(),
        network: endpoint.network.clone(),
        protocol: endpoint.protocol.clone(),
        checked_at,
        state_block: Some(block),
        account_exists: Some(complete),
        state_status: if complete {
            "live".into()
        } else {
            "rpc_error".into()
        },
        pool_family: "concentrated_v3".into(),
        decoder_version: "iziswap-state-v1".into(),
        state_error: if complete {
            None
        } else {
            error_text(map, &[base_id, base_id + 1, base_id + 2, base_id + 3])
                .or_else(|| Some("iZiSwap state() returned incomplete data".into()))
        },
        state_json: Some(
            json!({
                "sqrt_price_x96": sqrt_price_x96,
                "current_point": current_point,
                "liquidity_raw": liquidity,
                "liquidity_x_raw": liquidity_x,
                "fee_raw": fee_raw.map(|value| value.to_string()),
            })
            .to_string(),
        ),
        assets_json: Some(
            json!([
                {"asset_index": 0, "token_address": token0},
                {"asset_index": 1, "token_address": token1}
            ])
            .to_string(),
        ),
        quote_supported: false,
        token0_address: token0.clone(),
        token1_address: token1.clone(),
        fee_bps,
        reserve0_raw: None,
        reserve1_raw: None,
        sqrt_price_x96: sqrt_price_x96.clone(),
        active_liquidity_raw: liquidity,
        spot_price_usd: match (token0, token1, state) {
            (Some(a), Some(b), Some(state)) => spot_price(pool, &a, &b, &state),
            _ => None,
        },
        account_owner: Some(pool.address.clone()),
        account_data_bytes: None,
    }
}

fn parse_evm_state(
    pool: &Pool,
    endpoint: &Endpoint,
    block: i64,
    map: &HashMap<u64, RpcResponse>,
    base_id: u64,
    checked_at: i64,
) -> StateRow {
    let kind = evm_pool_kind(pool);
    if kind == EvmPoolKind::UniswapV4 {
        return parse_uniswap_v4_state(pool, endpoint, block, map, base_id, checked_at);
    }
    if kind == EvmPoolKind::Weighted {
        return parse_weighted_state(pool, endpoint, block, map, base_id, checked_at);
    }
    if kind == EvmPoolKind::Fluid {
        return parse_fluid_state(pool, endpoint, block, map, base_id, checked_at);
    }
    if kind == EvmPoolKind::Algebra {
        return parse_algebra_state(pool, endpoint, block, map, base_id, checked_at);
    }
    if kind == EvmPoolKind::KyberElastic {
        return parse_kyber_elastic_state(pool, endpoint, block, map, base_id, checked_at);
    }
    if kind == EvmPoolKind::MaverickV2 {
        return parse_maverick_v2_state(pool, endpoint, block, map, base_id, checked_at);
    }
    if kind == EvmPoolKind::Curve {
        return parse_curve_state(pool, endpoint, block, map, base_id, checked_at);
    }
    if kind == EvmPoolKind::LiquidityBook {
        return parse_liquidity_book_state(pool, endpoint, block, map, base_id, checked_at);
    }
    if kind == EvmPoolKind::LegacyLiquidityBook {
        return parse_legacy_liquidity_book_state(pool, endpoint, block, map, base_id, checked_at);
    }
    if kind == EvmPoolKind::EkuboV3 {
        return parse_ekubo_v3_state(pool, endpoint, block, map, base_id, checked_at);
    }
    if kind == EvmPoolKind::IziSwap {
        return parse_iziswap_state(pool, endpoint, block, map, base_id, checked_at);
    }
    if kind == EvmPoolKind::Unsupported {
        return StateRow {
            pool_address: pool.address.clone(),
            chain: pool.chain.clone(),
            network: endpoint.network.clone(),
            protocol: endpoint.protocol.clone(),
            checked_at,
            state_block: Some(block),
            account_exists: None,
            state_status: "unsupported_protocol".into(),
            pool_family: pool.pool_type.clone(),
            decoder_version: "none".into(),
            state_error: Some(format!(
                "no RPC decoder for protocol family: {}",
                pool.pool_type
            )),
            token0_address: None,
            token1_address: None,
            fee_bps: None,
            reserve0_raw: None,
            reserve1_raw: None,
            sqrt_price_x96: None,
            active_liquidity_raw: None,
            spot_price_usd: None,
            account_owner: None,
            account_data_bytes: None,
            state_json: None,
            assets_json: None,
            quote_supported: false,
        };
    }
    let v2 = kind == EvmPoolKind::ConstantProduct;
    let ids: Vec<u64> = if v2 {
        vec![base_id, base_id + 1, base_id + 5]
    } else {
        (0..6).map(|offset| base_id + offset).collect()
    };
    let token0 = result_hex(map, base_id)
        .and_then(|data| address_word(&data))
        .or_else(|| pool.token_a_address.clone());
    let token1 = result_hex(map, base_id + 1)
        .and_then(|data| address_word(&data))
        .or_else(|| pool.token_b_address.clone());
    let slot0 = if v2 {
        None
    } else {
        result_hex(map, base_id + 2)
    };
    let liquidity = if v2 {
        None
    } else {
        result_hex(map, base_id + 3)
            .and_then(|data| word(&data, 0))
            .map(|value| value.to_string())
    };
    let fee_bps = if v2 {
        None
    } else {
        result_hex(map, base_id + 4)
            .and_then(|data| word(&data, 0))
            .map(|value| value as f64 / 100.0)
    };
    let reserves = result_hex(map, base_id + 5);
    let reserve0 = reserves
        .as_deref()
        .and_then(|data| word(data, 0))
        .map(|value| value.to_string());
    let reserve1 = reserves
        .as_deref()
        .and_then(|data| word(data, 1))
        .map(|value| value.to_string());
    let complete = if v2 {
        token0.is_some() && token1.is_some() && reserve0.is_some() && reserve1.is_some()
    } else {
        token0.is_some()
            && token1.is_some()
            && slot0.is_some()
            && liquidity.is_some()
            && fee_bps.is_some()
    };
    let contract_present_without_pool_state = v2
        && pool.protocol.eq_ignore_ascii_case("velodrome v2")
        && token0.is_some()
        && token1.is_some()
        && (reserve0.is_none() || reserve1.is_none());
    let malformed_velodrome_index = contract_present_without_pool_state
        && !pool.token_a_symbol.trim().is_empty()
        && pool
            .token_a_symbol
            .trim()
            .eq_ignore_ascii_case(pool.token_b_symbol.trim());
    StateRow {
        pool_address: pool.address.clone(),
        chain: pool.chain.clone(),
        network: endpoint.network.clone(),
        protocol: endpoint.protocol.clone(),
        checked_at,
        state_block: Some(block),
        account_exists: None,
        state_status: if complete {
            "live".into()
        } else if malformed_velodrome_index {
            "invalid_index".into()
        } else if contract_present_without_pool_state {
            "account_present".into()
        } else {
            "rpc_error".into()
        },
        pool_family: pool.pool_type.clone(),
        decoder_version: if malformed_velodrome_index {
            "velodrome-index-validation-v1"
        } else if v2 {
            "constant-product-v2-raw-v1"
        } else {
            "concentrated-v3-raw-v1"
        }
        .into(),
        state_error: if complete {
            None
        } else if malformed_velodrome_index {
            Some("Velodrome index row has identical token symbols but no pool reserve API; exclude until re-indexed".into())
        } else if contract_present_without_pool_state {
            Some("contract has token metadata but no Velodrome pool reserve API; verify indexed pool address".into())
        } else {
            error_text(map, &ids)
                .or_else(|| Some("required protocol state call returned no result".into()))
        },
        token0_address: token0.clone(),
        token1_address: token1.clone(),
        fee_bps,
        reserve0_raw: reserve0.clone(),
        reserve1_raw: reserve1.clone(),
        sqrt_price_x96: slot0
            .clone()
            .and_then(|data| word_f64(&data, 0))
            .map(|value| value.to_string()),
        active_liquidity_raw: liquidity.clone(),
        spot_price_usd: match (token0.clone(), token1.clone(), slot0.clone()) {
            (Some(a), Some(b), Some(slot)) => spot_price(pool, &a, &b, &slot),
            _ => None,
        },
        account_owner: None,
        account_data_bytes: None,
        state_json: Some(json!({
            "token0": token0.clone(),
            "token1": token1.clone(),
            "sqrt_price_x96": slot0.as_ref().and_then(|data| word_f64(data, 0)).map(|value| value.to_string()),
            "active_liquidity_raw": liquidity,
            "reserve0_raw": reserve0,
            "reserve1_raw": reserve1,
            "index_validation": if malformed_velodrome_index {
                Some("same_token_symbol_without_pool_state")
            } else {
                None
            },
        }).to_string()),
        assets_json: None,
        quote_supported: false,
    }
}

fn parse_solana_state(
    pool: &Pool,
    endpoint: &Endpoint,
    slot: i64,
    map: &HashMap<u64, RpcResponse>,
    id: u64,
    vault_ids: Option<(u64, u64)>,
    mint_ids: Option<(u64, u64)>,
    checked_at: i64,
) -> StateRow {
    let value = map
        .get(&id)
        .and_then(|response| response.result.as_ref())
        .and_then(|result| result.get("value"));
    let exists = value.is_some_and(|value| !value.is_null());
    let owner = value
        .and_then(|value| value.get("owner"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let bytes = value
        .and_then(|value| value.get("data"))
        .and_then(Value::as_array)
        .and_then(|data| data.first())
        .and_then(Value::as_str)
        .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
        .map(|data| data.len() as i64);
    let account_data = value
        .and_then(|value| value.get("data"))
        .and_then(Value::as_array)
        .and_then(|data| data.first())
        .and_then(Value::as_str)
        .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok());
    let decoded = account_data
        .as_deref()
        .and_then(|data| decode_solana_pool_state(pool, data));
    let (decoded_token0, decoded_token1) = account_data
        .as_deref()
        .map(|data| solana_pool_mints(pool, data))
        .unwrap_or_default();
    let (vault0, vault1) = account_data
        .as_deref()
        .map(|data| solana_pool_vaults(pool, data))
        .unwrap_or_default();
    let reserve0 = vault_ids.and_then(|(vault0_id, _)| solana_token_account_amount(map, vault0_id));
    let reserve1 = vault_ids.and_then(|(_, vault1_id)| solana_token_account_amount(map, vault1_id));
    let mint_decimals0 = mint_ids.and_then(|(mint0_id, _)| solana_mint_decimals(map, mint0_id));
    let mint_decimals1 = mint_ids.and_then(|(_, mint1_id)| solana_mint_decimals(map, mint1_id));
    let decoded_live = exists && decoded.is_some();
    let token0 = decoded_token0.or_else(|| pool.token_a_address.clone());
    let token1 = decoded_token1.or_else(|| pool.token_b_address.clone());
    let mut state_json = decoded
        .as_ref()
        .map(|value| value.state_json.clone())
        .unwrap_or_else(|| json!({"account_present": exists}));
    if let Some(object) = state_json.as_object_mut() {
        object.insert("token0_address".into(), json!(token0));
        object.insert("token1_address".into(), json!(token1));
        object.insert("token0_vault".into(), json!(vault0));
        object.insert("token1_vault".into(), json!(vault1));
        object.insert("reserve0_raw".into(), json!(reserve0));
        object.insert("reserve1_raw".into(), json!(reserve1));
        if let Some(decimals) = mint_decimals0 {
            object.insert("mint_decimals_0".into(), json!(decimals));
        }
        if let Some(decimals) = mint_decimals1 {
            object.insert("mint_decimals_1".into(), json!(decimals));
        }
    }
    StateRow {
        pool_address: pool.address.clone(),
        chain: pool.chain.clone(),
        network: endpoint.network.clone(),
        protocol: endpoint.protocol.clone(),
        checked_at,
        state_block: Some(slot),
        account_exists: Some(exists),
        state_status: if decoded_live {
            "live".into()
        } else if exists {
            "account_present".into()
        } else {
            "account_missing".into()
        },
        pool_family: pool.pool_type.clone(),
        decoder_version: decoded
            .as_ref()
            .map(|value| value.decoder_version)
            .unwrap_or("solana-account-health-v1")
            .into(),
        state_error: if decoded_live {
            None
        } else if exists {
            Some(format!(
                "account exists but no decoder matched pool_type={} and data_bytes={}",
                pool.pool_type,
                bytes.unwrap_or_default()
            ))
        } else {
            Some("getAccountInfo returned null".into())
        },
        token0_address: token0,
        token1_address: token1,
        fee_bps: decoded.as_ref().and_then(|value| value.fee_bps),
        reserve0_raw: reserve0,
        reserve1_raw: reserve1,
        sqrt_price_x96: decoded.as_ref().and_then(|value| value.sqrt_price.clone()),
        active_liquidity_raw: decoded
            .as_ref()
            .and_then(|value| value.active_liquidity.clone()),
        spot_price_usd: None,
        account_owner: owner,
        account_data_bytes: bytes,
        state_json: Some(state_json.to_string()),
        assets_json: None,
        quote_supported: false,
    }
}

fn transport_error_state(
    pool: Pool,
    network: &str,
    protocol: &str,
    checked_at: i64,
    error: String,
) -> StateRow {
    StateRow {
        pool_address: pool.address,
        chain: pool.chain,
        network: network.to_string(),
        protocol: protocol.to_string(),
        checked_at,
        state_block: None,
        account_exists: None,
        state_status: "transport_error".into(),
        pool_family: pool.pool_type,
        decoder_version: "none".into(),
        state_error: Some(error),
        state_json: None,
        assets_json: None,
        quote_supported: false,
        token0_address: None,
        token1_address: None,
        fee_bps: None,
        reserve0_raw: None,
        reserve1_raw: None,
        sqrt_price_x96: None,
        active_liquidity_raw: None,
        spot_price_usd: None,
        account_owner: None,
        account_data_bytes: None,
    }
}

async fn latest_block(client: &Client, endpoint: &Endpoint, solana: bool) -> Result<i64> {
    let request = if solana {
        RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "getSlot",
            params: json!([{"commitment":"confirmed"}]),
        }
    } else {
        RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "eth_blockNumber",
            params: json!([]),
        }
    };
    let response = rpc_batch(client, &endpoint.url, vec![request])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("empty block response"))?;
    if solana {
        let value = response
            .result
            .ok_or_else(|| anyhow!("block RPC error: {:?}", response.error))?;
        if let Some(slot) = value.as_i64() {
            return Ok(slot);
        }
        if let Some(slot) = value.as_str() {
            return slot.parse::<i64>().context("parse Solana slot");
        }
        return Err(anyhow!("invalid Solana slot response: {value}"));
    }
    let value = response
        .result
        .and_then(|value| value.as_str().map(str::to_string))
        .ok_or_else(|| anyhow!("block RPC error: {:?}", response.error))?;
    i64::from_str_radix(value.trim_start_matches("0x"), 16).context("parse EVM block")
}

async fn block_timestamp(client: &Client, endpoint: &Endpoint, block: i64) -> Result<i64> {
    let request = RpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method: "eth_getBlockByNumber",
        params: json!([format!("0x{block:x}"), false]),
    };
    let response = rpc_batch(client, &endpoint.url, vec![request])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("empty historical block response"))?;
    let timestamp = response
        .result
        .and_then(|value| value.get("timestamp").cloned())
        .and_then(|value| value.as_str().map(str::to_string))
        .ok_or_else(|| anyhow!("historical block has no timestamp: {:?}", response.error))?;
    i64::from_str_radix(timestamp.trim_start_matches("0x"), 16)
        .context("parse historical block timestamp")
}

/// Select one archive-capable EVM block for each hour. The block spacing is
/// estimated from a 1,000-block probe, then each historical call uses an
/// explicit block tag. We store an exact hourly grid in SQLite, while the RPC
/// state comes from the nearest block to that hour.
async fn historical_blocks(
    client: &Client,
    endpoint: &Endpoint,
    latest: i64,
    hours: usize,
) -> Result<Vec<(i64, i64)>> {
    if hours == 0 || latest <= 0 {
        return Ok(Vec::new());
    }
    let latest_timestamp = block_timestamp(client, endpoint, latest).await?;
    let probe_block = latest.saturating_sub(1_000);
    let probe_timestamp = block_timestamp(client, endpoint, probe_block).await?;
    let seconds_per_block = ((latest_timestamp - probe_timestamp) as f64 / 1_000.0).max(0.1);
    let anchor = latest_timestamp.div_euclid(3_600) * 3_600;
    let mut result = Vec::with_capacity(hours);
    for offset in (0..hours).rev() {
        let timestamp = anchor - offset as i64 * 3_600;
        let blocks_back =
            ((latest_timestamp - timestamp).max(0) as f64 / seconds_per_block).round() as i64;
        result.push((timestamp, latest.saturating_sub(blocks_back)));
    }
    Ok(result)
}

fn metadata_decimals(connection: &Connection) -> Result<HashMap<(String, String), u32>> {
    let mut result = HashMap::new();
    let mut statement = connection.prepare(
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
            result.insert((chain, address.to_ascii_lowercase()), decimals as u32);
        }
    }
    Ok(result)
}

fn state_number(value: Option<&str>) -> Option<f64> {
    value?.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn history_ratio(
    pool: &Pool,
    state: &StateRow,
    decimals: &HashMap<(String, String), u32>,
) -> Option<(String, String, f64)> {
    if state.state_status != "live" {
        return None;
    }
    let token0 = state
        .token0_address
        .clone()
        .or_else(|| pool.token_a_address.clone())?;
    let token1 = state
        .token1_address
        .clone()
        .or_else(|| pool.token_b_address.clone())?;
    let key0 = (pool.chain.clone(), token0.to_ascii_lowercase());
    let key1 = (pool.chain.clone(), token1.to_ascii_lowercase());
    let state_json = state
        .state_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    let decimals0 = decimals
        .get(&key0)
        .copied()
        .or_else(|| {
            state_json
                .as_ref()
                .and_then(|value| value.get("mint_decimals_0"))
                .and_then(Value::as_u64)
                .map(|value| value as u32)
        })
        .or_else(|| {
            (token0.eq_ignore_ascii_case("0x0000000000000000000000000000000000000000")
                || token0.eq_ignore_ascii_case("0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"))
            .then_some(18)
        })?;
    let decimals1 = decimals
        .get(&key1)
        .copied()
        .or_else(|| {
            state_json
                .as_ref()
                .and_then(|value| value.get("mint_decimals_1"))
                .and_then(Value::as_u64)
                .map(|value| value as u32)
        })
        .or_else(|| {
            (token1.eq_ignore_ascii_case("0x0000000000000000000000000000000000000000")
                || token1.eq_ignore_ascii_case("0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"))
            .then_some(18)
        })?;
    let decimal_scale = 10f64.powi(decimals0 as i32 - decimals1 as i32);
    let raw_ratio = if let (Some(reserve0), Some(reserve1)) = (
        state_number(state.reserve0_raw.as_deref()),
        state_number(state.reserve1_raw.as_deref()),
    ) {
        (reserve1 > 0.0 && reserve0 > 0.0).then_some((reserve1 / reserve0) * decimal_scale)?
    } else if let Some(sqrt) = state_number(state.sqrt_price_x96.as_deref()) {
        (sqrt > 0.0).then_some((sqrt * sqrt / 2f64.powi(192)) * decimal_scale)?
    } else {
        let active_id = state_json
            .as_ref()
            .and_then(|value| value.get("active_id"))
            .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))?;
        let bin_step = state_json
            .as_ref()
            .and_then(|value| value.get("bin_step"))
            .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))?;
        (1.0 + bin_step / 10_000.0).powf(active_id) * decimal_scale
    };
    let token1_per_token0 = 1.0 / raw_ratio;
    token1_per_token0
        .is_finite()
        .then_some((token0, token1, token1_per_token0))
        .filter(|(_, _, ratio)| *ratio > 0.0)
}

fn write_historical_ratios(
    connection: &mut Connection,
    timestamp: i64,
    pools: &[Pool],
    states: &[StateRow],
    decimals: &HashMap<(String, String), u32>,
    observed_at: i64,
) -> Result<usize> {
    let by_address = pools
        .iter()
        .map(|pool| (pool.address.as_str(), pool))
        .collect::<HashMap<_, _>>();
    let transaction = connection.transaction()?;
    let mut written = 0;
    for state in states {
        let Some(pool) = by_address.get(state.pool_address.as_str()) else {
            continue;
        };
        let Some((token0, token1, ratio)) = history_ratio(pool, state, decimals) else {
            continue;
        };
        written += transaction.execute(
            "INSERT OR IGNORE INTO pool_price_history(chain,pool_address,timestamp,token0_address,token1_address,token1_per_token0,token0_price_usd,token1_price_usd,source,price_status,status_reason,observed_at) VALUES(?1,?2,?3,?4,?5,?6,NULL,NULL,'rpc_pool_historical','priced',NULL,?7)",
            params![state.chain, state.pool_address, timestamp, token0, token1, ratio, observed_at],
        )?;
    }
    transaction.commit()?;
    Ok(written)
}

async fn refresh_batch(
    client: Arc<Client>,
    semaphore: Arc<Semaphore>,
    endpoint: Endpoint,
    pools: Vec<Pool>,
    block: i64,
    checked_at: i64,
) -> Vec<StateRow> {
    let _permit = match semaphore.acquire().await {
        Ok(permit) => permit,
        Err(_) => return Vec::new(),
    };
    let solana = endpoint.protocol.eq_ignore_ascii_case("solana");
    let base_id = 1000;
    let requests = if solana {
        solana_requests(&pools, base_id)
    } else {
        evm_requests(&pools, base_id, &endpoint.network, block)
    };
    let mut responses = if requests.is_empty() {
        HashMap::new()
    } else {
        match rpc_batch(&client, &endpoint.url, requests).await {
            Ok(responses) => response_map(responses),
            Err(error) => {
                return pools
                    .into_iter()
                    .map(|pool| StateRow {
                        pool_address: pool.address,
                        chain: pool.chain,
                        network: endpoint.network.clone(),
                        protocol: endpoint.protocol.clone(),
                        checked_at,
                        state_block: Some(block),
                        account_exists: None,
                        state_status: "transport_error".into(),
                        pool_family: pool.pool_type,
                        decoder_version: "none".into(),
                        state_error: Some(redact_rpc_error(&error.to_string())),
                        token0_address: None,
                        token1_address: None,
                        fee_bps: None,
                        reserve0_raw: None,
                        reserve1_raw: None,
                        sqrt_price_x96: None,
                        active_liquidity_raw: None,
                        spot_price_usd: None,
                        account_owner: None,
                        account_data_bytes: None,
                        state_json: None,
                        assets_json: None,
                        quote_supported: false,
                    })
                    .collect()
            }
        }
    };
    if solana {
        let mint_requests = solana_mint_requests(&pools, &responses, base_id);
        for chunk in mint_requests.chunks(40) {
            if let Ok(mint_responses) = rpc_batch(&client, &endpoint.url, chunk.to_vec()).await {
                for (id, response) in response_map(mint_responses) {
                    responses.insert(id, response);
                }
            }
        }
        let vault_requests = solana_vault_requests(&pools, &responses, base_id);
        // Keep the follow-up account batches below the free-tier gateway
        // burst limit. A pool batch of 100 can expand to roughly 200 vault
        // reads; sending that as one request causes partial responses even
        // when the first-stage pool accounts were returned successfully.
        for chunk in vault_requests.chunks(40) {
            if let Ok(vault_responses) = rpc_batch(&client, &endpoint.url, chunk.to_vec()).await {
                for (id, response) in response_map(vault_responses) {
                    responses.insert(id, response);
                }
            }
        }
    }
    pools
        .iter()
        .enumerate()
        .map(|(index, pool)| {
            let pool_base_id = if solana {
                base_id + index as u64
            } else {
                base_id + index as u64 * 100
            };
            let mut state = if solana {
                let vault_ids =
                    (pool.pool_type == "solana_standard").then(|| solana_vault_ids(index, base_id));
                let mint_ids =
                    matches!(pool.pool_type.as_str(), "solana_dlmm" | "solana_whirlpool")
                        .then(|| solana_mint_ids(index, base_id));
                parse_solana_state(
                    pool,
                    &endpoint,
                    block,
                    &responses,
                    pool_base_id,
                    vault_ids,
                    mint_ids,
                    checked_at,
                )
            } else {
                parse_evm_state(pool, &endpoint, block, &responses, pool_base_id, checked_at)
            };
            if let Some(error) = pool_transport_error(&responses, pool_base_id, solana) {
                state.state_status = "transport_error".into();
                state.state_error = Some(error);
            }
            state
        })
        .collect()
}

fn write_states(connection: &mut Connection, states: &[StateRow]) -> Result<()> {
    let transaction = connection.transaction()?;
    for state in states {
        transaction.execute(
            r#"INSERT INTO pool_rpc_state (pool_address, chain, rpc_network, rpc_protocol, checked_at, state_block, account_exists, state_status, pool_family, decoder_version, state_error, state_json, assets_json, quote_supported, token0_address, token1_address, fee_bps, reserve0_raw, reserve1_raw, sqrt_price_x96, active_liquidity_raw, spot_price_usd, account_owner, account_data_bytes)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(pool_address) DO UPDATE SET chain=excluded.chain, rpc_network=excluded.rpc_network, rpc_protocol=excluded.rpc_protocol, checked_at=excluded.checked_at, state_block=excluded.state_block, account_exists=excluded.account_exists, state_status=excluded.state_status, pool_family=excluded.pool_family, decoder_version=excluded.decoder_version, state_error=excluded.state_error, state_json=excluded.state_json, assets_json=excluded.assets_json, quote_supported=excluded.quote_supported, token0_address=excluded.token0_address, token1_address=excluded.token1_address, fee_bps=excluded.fee_bps, reserve0_raw=excluded.reserve0_raw, reserve1_raw=excluded.reserve1_raw, sqrt_price_x96=excluded.sqrt_price_x96, active_liquidity_raw=excluded.active_liquidity_raw, spot_price_usd=excluded.spot_price_usd, account_owner=excluded.account_owner, account_data_bytes=excluded.account_data_bytes
               WHERE excluded.state_status NOT IN ('transport_error', 'account_missing')
                  OR pool_rpc_state.state_status NOT IN ('live', 'account_present')"#,
            params![state.pool_address, state.chain, state.network, state.protocol, state.checked_at, state.state_block, state.account_exists.map(|value| value as i64), state.state_status, state.pool_family, state.decoder_version, state.state_error, state.state_json, state.assets_json, state.quote_supported as i64, state.token0_address, state.token1_address, state.fee_bps, state.reserve0_raw, state.reserve1_raw, state.sqrt_price_x96, state.active_liquidity_raw, state.spot_price_usd, state.account_owner, state.account_data_bytes],
        )?;
        transaction.execute(
            "UPDATE pools
             SET token_a_address=coalesce(?1, token_a_address),
                 token_b_address=coalesce(?2, token_b_address)
             WHERE pool_address=?3",
            params![
                state.token0_address,
                state.token1_address,
                state.pool_address
            ],
        )?;
        if state.chain.eq_ignore_ascii_case("solana") {
            if let Some(value) = state
                .state_json
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            {
                for (index, address) in [
                    (0usize, state.token0_address.as_deref()),
                    (1usize, state.token1_address.as_deref()),
                ] {
                    let Some(address) = address.filter(|value| !value.trim().is_empty()) else {
                        continue;
                    };
                    let decimals = [
                        value
                            .get("mint_decimals_0")
                            .or_else(|| value.get("mint_0_decimals")),
                        value
                            .get("mint_decimals_1")
                            .or_else(|| value.get("mint_1_decimals")),
                    ][index]
                        .and_then(Value::as_u64)
                        .filter(|value| *value <= 36);
                    let Some(decimals) = decimals else { continue };
                    transaction.execute(
                        "INSERT INTO token_metadata (chain,address,symbol,name,decimals,status,source,checked_at,error,code_present,metadata_issue)
                         VALUES (?1,?2,NULL,NULL,?3,'complete','rpc_mint_account',?4,NULL,1,NULL)
                         ON CONFLICT(chain,address) DO UPDATE SET
                           decimals=excluded.decimals,
                           status='complete',
                           source='rpc_mint_account',
                           checked_at=excluded.checked_at,
                           error=NULL",
                        params![state.chain, address, decimals as i64, state.checked_at],
                    )?;
                }
            }
        }
    }
    transaction.commit()?;
    Ok(())
}

async fn refresh_cycle(
    client: Arc<Client>,
    sqlite_path: &Path,
    chain_filter: Option<&str>,
    state_status: Option<&str>,
    batch_size: usize,
    concurrency: usize,
    limit: usize,
    maintenance_only: bool,
    interval_secs: u64,
    priority_only: bool,
) -> Result<RefreshReport> {
    let cycle_started_at = now();
    let started = Instant::now();
    let endpoints = load_endpoints()?;
    let mut connection = Connection::open(sqlite_path)?;
    connection.execute_batch(
        "PRAGMA busy_timeout=5000; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;",
    )?;
    ensure_state_table(&connection)?;
    let pools = if priority_only {
        load_priority_pools(sqlite_path, chain_filter, limit)?
    } else {
        load_pools(
            sqlite_path,
            chain_filter,
            state_status,
            limit,
            maintenance_only,
            true,
        )?
    };
    if pools.is_empty() {
        return Err(anyhow!("no pools selected"));
    }
    let checked_at = now();
    let mut grouped: HashMap<String, Vec<Pool>> = HashMap::new();
    for pool in pools {
        grouped.entry(pool.chain.clone()).or_default().push(pool);
    }
    let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut tasks = Vec::new();
    let mut pre_states = Vec::new();
    let mut selected = 0usize;
    let mut batches = 0usize;
    for (chain, chain_pools) in grouped {
        let Some(network) = chain_network(&chain) else {
            selected += chain_pools.len();
            batches += 1;
            pre_states.extend(chain_pools.into_iter().map(|pool| {
                transport_error_state(
                    pool,
                    "unknown",
                    "unknown",
                    checked_at,
                    "chain is not mapped to an RPC network".into(),
                )
            }));
            continue;
        };
        let Some(endpoint_candidates) = endpoints.get(network).cloned() else {
            eprintln!("skip {chain}: no healthy endpoint for {network}");
            selected += chain_pools.len();
            batches += 1;
            pre_states.extend(chain_pools.into_iter().map(|pool| {
                transport_error_state(
                    pool,
                    network,
                    if chain.eq_ignore_ascii_case("solana") {
                        "solana"
                    } else {
                        "evm"
                    },
                    checked_at,
                    format!("no healthy RPC endpoint for {network}"),
                )
            }));
            continue;
        };
        let solana = chain.eq_ignore_ascii_case("solana");
        let (endpoint, block) = match select_endpoint(&client, &endpoint_candidates, solana).await {
            Ok(block) => block,
            Err(error) => {
                eprintln!("skip {chain}: {}", redact_rpc_error(&error.to_string()));
                selected += chain_pools.len();
                batches += 1;
                let error = redact_rpc_error(&error.to_string());
                pre_states.extend(chain_pools.into_iter().map(|pool| {
                    transport_error_state(
                        pool,
                        network,
                        if solana { "solana" } else { "evm" },
                        checked_at,
                        error.clone(),
                    )
                }));
                continue;
            }
        };
        println!(
            "refresh {chain}: {} pools at {} {}",
            chain_pools.len(),
            if solana { "slot" } else { "block" },
            block
        );
        let chain_batch_size = effective_batch_size(&chain, batch_size);
        if chain_batch_size != batch_size {
            eprintln!(
                "refresh {chain}: reducing JSON-RPC batch size {batch_size} -> {chain_batch_size} for free-tier CU safety"
            );
        }
        for chunk in chain_pools.chunks(chain_batch_size) {
            selected += chunk.len();
            batches += 1;
            tasks.push(tokio::spawn(refresh_batch(
                client.clone(),
                semaphore.clone(),
                endpoint.clone(),
                chunk.to_vec(),
                block,
                checked_at,
            )));
        }
    }
    if selected == 0 {
        return Err(anyhow!("no pools selected"));
    }

    let mut states_written = 0usize;
    let mut live_or_present = 0usize;
    let mut account_missing = 0usize;
    let mut rpc_error = 0usize;
    let mut transport_error = 0usize;
    let mut unsupported_protocol = 0usize;
    let mut pending_states = Vec::with_capacity(STATE_WRITE_BATCH_SIZE);
    if !pre_states.is_empty() {
        live_or_present += pre_states
            .iter()
            .filter(|state| state.state_status == "live" || state.state_status == "account_present")
            .count();
        account_missing += pre_states
            .iter()
            .filter(|state| state.state_status == "account_missing")
            .count();
        rpc_error += pre_states
            .iter()
            .filter(|state| state.state_status == "rpc_error")
            .count();
        transport_error += pre_states
            .iter()
            .filter(|state| state.state_status == "transport_error")
            .count();
        unsupported_protocol += pre_states
            .iter()
            .filter(|state| state.state_status == "unsupported_protocol")
            .count();
        pending_states.extend(pre_states);
        if !priority_only && pending_states.len() >= STATE_WRITE_BATCH_SIZE {
            write_states(&mut connection, &pending_states)?;
            states_written += pending_states.len();
            pending_states.clear();
        }
    }
    for task in tasks {
        let batch_states = task
            .await
            .map_err(|error| anyhow!("refresh task: {error}"))?;
        live_or_present += batch_states
            .iter()
            .filter(|state| state.state_status == "live" || state.state_status == "account_present")
            .count();
        account_missing += batch_states
            .iter()
            .filter(|state| state.state_status == "account_missing")
            .count();
        rpc_error += batch_states
            .iter()
            .filter(|state| state.state_status == "rpc_error")
            .count();
        transport_error += batch_states
            .iter()
            .filter(|state| state.state_status == "transport_error")
            .count();
        unsupported_protocol += batch_states
            .iter()
            .filter(|state| state.state_status == "unsupported_protocol")
            .count();
        pending_states.extend(batch_states);
        if !priority_only && pending_states.len() >= STATE_WRITE_BATCH_SIZE {
            write_states(&mut connection, &pending_states)?;
            states_written += pending_states.len();
            pending_states.clear();
        }
    }
    if !pending_states.is_empty() {
        write_states(&mut connection, &pending_states)?;
        states_written += pending_states.len();
    }
    let duration_ms = started.elapsed().as_millis() as u64;
    let duration_secs = (duration_ms as f64 / 1000.0).max(0.001);
    Ok(RefreshReport {
        status: "ok".into(),
        cycle_started_at,
        cycle_finished_at: now(),
        duration_ms,
        selected,
        states_written,
        batches,
        live_or_present,
        account_missing,
        rpc_error,
        transport_error,
        unsupported_protocol,
        pools_per_second: states_written as f64 / duration_secs,
        batch_size,
        concurrency,
        interval_secs,
        error: None,
    })
}

async fn historical_cycle(
    client: Arc<Client>,
    sqlite_path: &Path,
    chain_filter: Option<&str>,
    hours: usize,
    batch_size: usize,
    concurrency: usize,
    limit: usize,
) -> Result<Value> {
    let required_points = hours.max(1);
    let price_path = price_db_path(sqlite_path);
    let catalog_connection = Connection::open(sqlite_path)?;
    catalog_connection.execute_batch("PRAGMA busy_timeout=5000;")?;
    let mut price_connection = Connection::open(&price_path)?;
    price_connection.execute_batch("PRAGMA busy_timeout=5000; PRAGMA journal_mode=WAL;")?;
    ensure_price_history_table(&price_connection)?;
    let pools = load_history_pools(
        sqlite_path,
        &price_path,
        chain_filter,
        required_points,
        limit,
    )?;
    if pools.is_empty() {
        return Ok(json!({
            "hours": hours,
            "selected_pools": 0,
            "historical_blocks": 0,
            "written_points": 0,
            "status": "complete"
        }));
    }
    let decimals = metadata_decimals(&catalog_connection)?;
    let endpoints = load_endpoints()?;
    let mut grouped = HashMap::<String, Vec<Pool>>::new();
    for pool in pools {
        grouped.entry(pool.chain.clone()).or_default().push(pool);
    }
    let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut selected_pools = 0usize;
    let mut block_count = 0usize;
    let mut written_points = 0usize;
    let mut failed_chains = Vec::new();
    for (chain, chain_pools) in grouped {
        let Some(network) = chain_network(&chain) else {
            failed_chains.push(chain);
            continue;
        };
        let Some(endpoint_candidates) = endpoints.get(network).cloned() else {
            failed_chains.push(chain);
            continue;
        };
        let (endpoint, latest) = match select_endpoint(&client, &endpoint_candidates, false).await {
            Ok(value) => value,
            Err(error) => {
                failed_chains.push(format!("{chain}: {}", redact_rpc_error(&error.to_string())));
                continue;
            }
        };
        let blocks = match historical_blocks(&client, &endpoint, latest, hours).await {
            Ok(value) => value,
            Err(error) => {
                failed_chains.push(format!("{chain}: {}", redact_rpc_error(&error.to_string())));
                continue;
            }
        };
        selected_pools += chain_pools.len();
        block_count += blocks.len();
        // Schedule the time dimension as well as the pool-batch dimension.
        // The old loop waited for all pools at hour N before sending hour
        // N+1, which made even one pool perform 168 serialized HTTP round
        // trips. The semaphore still caps upstream pressure, while all
        // hourly reads can now make progress concurrently.
        let chain_pools = Arc::new(chain_pools);
        let mut block_tasks = Vec::with_capacity(blocks.len());
        for (timestamp, block) in blocks {
            let pools = Arc::clone(&chain_pools);
            let client = client.clone();
            let semaphore = semaphore.clone();
            let endpoint = endpoint.clone();
            block_tasks.push(tokio::spawn(async move {
                let mut tasks = Vec::new();
                for chunk in pools.chunks(batch_size.max(1)) {
                    tasks.push(tokio::spawn(refresh_batch(
                        client.clone(),
                        semaphore.clone(),
                        endpoint.clone(),
                        chunk.to_vec(),
                        block,
                        timestamp,
                    )));
                }
                let mut states = Vec::new();
                for task in tasks {
                    states.extend(
                        task.await
                            .map_err(|error| anyhow!("historical RPC task: {error}"))?,
                    );
                }
                Ok::<_, anyhow::Error>((timestamp, states))
            }));
        }
        for task in block_tasks {
            let (timestamp, states) = task
                .await
                .map_err(|error| anyhow!("historical block task: {error}"))??;
            written_points += write_historical_ratios(
                &mut price_connection,
                timestamp,
                &chain_pools,
                &states,
                &decimals,
                now(),
            )?;
        }
    }
    Ok(json!({
        "hours": hours,
        "required_points_per_pool": required_points,
        "selected_pools": selected_pools,
        "historical_blocks": block_count,
        "written_points": written_points,
        "failed_chains": failed_chains,
        "status": if failed_chains.is_empty() { "complete" } else { "partial" }
    }))
}

fn write_report(path: &Path, report: &RefreshReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(report)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let sqlite_path = PathBuf::from(arg("--sqlite", "data/lp_screener.db"));
    let chain_filter = env::args()
        .position(|value| value == "--chain")
        .and_then(|index| env::args().nth(index + 1));
    let state_status = env::args()
        .position(|value| value == "--state-status")
        .and_then(|index| env::args().nth(index + 1));
    if state_status.as_deref().is_some_and(|status| {
        !matches!(
            status,
            "live"
                | "account_present"
                | "account_missing"
                | "rpc_error"
                | "transport_error"
                | "unsupported_protocol"
        )
    }) {
        return Err(anyhow!("unsupported --state-status value"));
    }
    let batch_size: usize = arg("--batch-size", "40").parse().context("--batch-size")?;
    let concurrency: usize = arg("--concurrency", "4").parse().context("--concurrency")?;
    let limit: usize = arg("--limit", "0").parse().context("--limit")?;
    let maintenance_only = has_flag("--maintenance-only");
    let duration_secs: u64 = arg("--duration-secs", "0")
        .parse()
        .context("--duration-secs")?;
    let interval_secs: u64 = arg("--interval-secs", "900")
        .parse()
        .context("--interval-secs")?;
    let metrics_path = PathBuf::from(arg("--metrics", "lp_refresh_latest.json"));
    let priority_only = has_flag("--priority-only");
    let history_hours: usize = arg("--history-hours", "0")
        .parse()
        .context("--history-hours")?;
    let history_only = has_flag("--history-only");
    if priority_only && maintenance_only {
        return Err(anyhow!(
            "--priority-only and --maintenance-only cannot be combined"
        ));
    }
    let client = Arc::new(
        Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(20))
            .build()?,
    );
    if history_only {
        if history_hours == 0 {
            return Err(anyhow!("--history-only requires --history-hours > 0"));
        }
        let report = historical_cycle(
            client,
            &sqlite_path,
            chain_filter.as_deref(),
            history_hours,
            batch_size,
            concurrency,
            limit,
        )
        .await?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let run_started = Instant::now();
    let deadline = (duration_secs > 0).then(|| run_started + Duration::from_secs(duration_secs));
    let mut next_run = run_started;
    loop {
        let report = match refresh_cycle(
            client.clone(),
            &sqlite_path,
            chain_filter.as_deref(),
            state_status.as_deref(),
            batch_size,
            concurrency,
            limit,
            maintenance_only,
            interval_secs,
            priority_only,
        )
        .await
        {
            Ok(report) => report,
            Err(error) => {
                let report = RefreshReport {
                    status: "error".into(),
                    cycle_started_at: now(),
                    cycle_finished_at: now(),
                    duration_ms: 0,
                    selected: 0,
                    states_written: 0,
                    batches: 0,
                    live_or_present: 0,
                    account_missing: 0,
                    rpc_error: 0,
                    transport_error: 0,
                    unsupported_protocol: 0,
                    pools_per_second: 0.0,
                    batch_size,
                    concurrency,
                    interval_secs,
                    error: Some(redact_rpc_error(&error.to_string())),
                };
                write_report(&metrics_path, &report)?;
                if duration_secs == 0 {
                    return Err(error);
                }
                eprintln!(
                    "refresh cycle failed: {}",
                    redact_rpc_error(&error.to_string())
                );
                report
            }
        };
        write_report(&metrics_path, &report)?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        if history_hours > 0 {
            let history = historical_cycle(
                client.clone(),
                &sqlite_path,
                chain_filter.as_deref(),
                history_hours,
                batch_size,
                concurrency,
                limit,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&history)?);
        }
        if duration_secs == 0 {
            break;
        }
        next_run += Duration::from_secs(interval_secs.max(1));
        let Some(deadline) = deadline else { break };
        if Instant::now() >= deadline {
            break;
        }
        let sleep_until = next_run.min(deadline);
        if sleep_until > Instant::now() {
            tokio::time::sleep_until(tokio::time::Instant::from_std(sleep_until)).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(protocol: &str, pool_type: &str, address: &str) -> Pool {
        Pool {
            address: address.into(),
            chain: "ethereum".into(),
            protocol: protocol.into(),
            pool_type: pool_type.into(),
            token_a_symbol: "USDC".into(),
            token_b_symbol: "WETH".into(),
            token_a_address: Some("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".into()),
            token_b_address: Some("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2".into()),
        }
    }

    #[test]
    fn classifies_newly_supported_protocols() {
        assert_eq!(
            evm_pool_kind(&pool(
                "PancakeSwap StableSwap",
                "stable_swap",
                "0x0000000000000000000000000000000000000001"
            )),
            EvmPoolKind::Curve
        );
        assert_eq!(
            evm_pool_kind(&pool(
                "SyncSwap V2.1",
                "unsupported_protocol",
                "0x0000000000000000000000000000000000000001"
            )),
            EvmPoolKind::ConstantProduct
        );
        assert_eq!(
            evm_pool_kind(&pool(
                "Ekubo V3",
                "concentrated_v3",
                "0x2412d98ba8200050ba63355f9473f7f3f31c273f1b2ed56a013ad49b90b9260d"
            )),
            EvmPoolKind::EkuboV3
        );
        assert_eq!(
            evm_pool_kind(&pool(
                "KyberSwap",
                "concentrated_v3",
                "0x0000000000000000000000000000000000000001"
            )),
            EvmPoolKind::ConstantProduct
        );
        assert_eq!(
            evm_pool_kind(&pool(
                "KyberSwap Elastic",
                "concentrated_v3",
                "0x0000000000000000000000000000000000000001"
            )),
            EvmPoolKind::KyberElastic
        );
        assert_eq!(
            evm_pool_kind(&pool(
                "THENA FUSION",
                "liquidity_book",
                "0x0000000000000000000000000000000000000001"
            )),
            EvmPoolKind::Algebra
        );
    }

    #[test]
    fn only_all_transport_failures_mark_a_pool_transport_error() {
        let mut responses = HashMap::new();
        responses.insert(1000, transport_response(1000, "gateway unavailable"));
        assert!(pool_transport_error(&responses, 1000, false).is_some());

        responses.insert(
            1001,
            RpcResponse {
                id: 1001,
                result: Some(json!("0x01")),
                error: None,
            },
        );
        assert!(pool_transport_error(&responses, 1000, false).is_none());
    }

    #[test]
    fn decodes_uniswap_v4_fee_units_and_dynamic_marker() {
        assert_eq!(uniswap_v4_fee_bps(10_000), Some(100.0));
        assert_eq!(uniswap_v4_fee_bps(1_000_000), Some(10_000.0));
        assert_eq!(uniswap_v4_fee_bps(0x800000), None);
    }

    #[test]
    fn decodes_fluid_packed_fee_from_full_uint256_word() {
        let packed = "0x00fa3e8d8d7231813f449000000000000000ffffff000004cbbc5b8d80004e21";
        let value = low_word(packed).expect("Fluid uint256 low word");
        let raw_fee = (value >> 2) & ((1u128 << 17) - 1);
        assert_eq!(raw_fee, 5_000);
        assert_eq!(raw_fee as f64 / 100.0, 50.0);
    }

    #[test]
    fn decodes_solana_mints_from_protocol_account_layouts() {
        for (pool_type, first, second, length) in [
            ("solana_clmm", 73, 105, 273),
            ("solana_whirlpool", 101, 181, 245),
            ("solana_standard", 168, 200, 389),
            ("solana_dlmm", 88, 120, 232),
        ] {
            let mut data = vec![0u8; length];
            let mint_a = (1u8..=32).collect::<Vec<_>>();
            let mint_b = (33u8..=64).collect::<Vec<_>>();
            data[first..first + 32].copy_from_slice(&mint_a);
            data[second..second + 32].copy_from_slice(&mint_b);
            let mut candidate = pool("Raydium", pool_type, "pool");
            candidate.pool_type = pool_type.into();
            let (decoded_a, decoded_b) = solana_pool_mints(&candidate, &data);
            assert_eq!(decoded_a, Some(bs58::encode(mint_a).into_string()));
            assert_eq!(decoded_b, Some(bs58::encode(mint_b).into_string()));
        }

        let mut legacy = vec![0u8; 752];
        let mint_a = (65u8..=96).collect::<Vec<_>>();
        let mint_b = (97u8..=128).collect::<Vec<_>>();
        let vault_a = (129u8..=160).collect::<Vec<_>>();
        let vault_b = (161u8..=192).collect::<Vec<_>>();
        legacy[400..432].copy_from_slice(&mint_a);
        legacy[432..464].copy_from_slice(&mint_b);
        legacy[336..368].copy_from_slice(&vault_a);
        legacy[368..400].copy_from_slice(&vault_b);
        let candidate = pool("Raydium", "solana_standard", "legacy");
        assert_eq!(
            solana_pool_mints(&candidate, &legacy),
            (
                Some(bs58::encode(mint_a).into_string()),
                Some(bs58::encode(mint_b).into_string())
            )
        );
        assert_eq!(
            solana_pool_vaults(&candidate, &legacy),
            (
                Some(bs58::encode(vault_a).into_string()),
                Some(bs58::encode(vault_b).into_string())
            )
        );

        let mut stable = vec![0u8; 1232];
        let stable_mint_a = (193u8..=224).collect::<Vec<_>>();
        let stable_mint_b = (0u8..32)
            .map(|offset| 225u8.wrapping_add(offset))
            .collect::<Vec<_>>();
        let stable_vault_a = (1u8..=32).collect::<Vec<_>>();
        let stable_vault_b = (33u8..=64).collect::<Vec<_>>();
        stable[464..496].copy_from_slice(&stable_mint_a);
        stable[496..528].copy_from_slice(&stable_mint_b);
        stable[368..400].copy_from_slice(&stable_vault_a);
        stable[400..432].copy_from_slice(&stable_vault_b);
        assert_eq!(
            solana_pool_mints(&candidate, &stable),
            (
                Some(bs58::encode(stable_mint_a).into_string()),
                Some(bs58::encode(stable_mint_b).into_string())
            )
        );
        assert_eq!(
            solana_pool_vaults(&candidate, &stable),
            (
                Some(bs58::encode(stable_vault_a).into_string()),
                Some(bs58::encode(stable_vault_b).into_string())
            )
        );
    }

    #[test]
    fn decodes_ekubo_v3_packed_state() {
        let pool = pool(
            "Ekubo V3",
            "concentrated_v3",
            "0x2412d98ba8200050ba63355f9473f7f3f31c273f1b2ed56a013ad49b90b9260d",
        );
        let endpoint = Endpoint {
            network: "eth-mainnet".into(),
            protocol: "EVM".into(),
            url: "http://127.0.0.1".into(),
        };
        let mut responses = HashMap::new();
        responses.insert(
            1002,
            RpcResponse {
                id: 1002,
                result: Some(json!(
                    "0x7fff471649fab8a762c84d76ffffffa70000000000000000001c9d43218682bf"
                )),
                error: None,
            },
        );
        let state = parse_ekubo_v3_state(&pool, &endpoint, 1, &responses, 1000, 2);
        assert_eq!(state.state_status, "live");
        assert_eq!(
            state.active_liquidity_raw.as_deref(),
            Some("8054210998731455")
        );
        assert!(state
            .state_json
            .as_deref()
            .is_some_and(|json| json.contains("\"tick\":-89")));
    }
}
