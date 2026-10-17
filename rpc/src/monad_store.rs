//! Persistent contract shared by the API and RPC listener. No network work here.
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::PathBuf;

pub fn path() -> PathBuf {
    std::env::var_os("MONAD_LISTENER_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if std::path::Path::new("rpc/Cargo.toml").exists() {
                "rpc/data/monad_events.db".into()
            } else if std::path::Path::new("../rpc/Cargo.toml").exists() {
                "../rpc/data/monad_events.db".into()
            } else {
                "data/monad_events.db".into()
            }
        })
}
pub fn open() -> Result<Connection> {
    let p = path();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let c = Connection::open(p)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    c.execute_batch("PRAGMA journal_mode=WAL;
      CREATE TABLE IF NOT EXISTS display_config(id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL, body TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS cursors(pool TEXT PRIMARY KEY, block INTEGER NOT NULL, hash TEXT NOT NULL, started_at INTEGER NOT NULL, error TEXT);
      CREATE TABLE IF NOT EXISTS events(pool TEXT NOT NULL, block INTEGER NOT NULL, block_hash TEXT NOT NULL, tx TEXT NOT NULL, log_index INTEGER NOT NULL, timestamp INTEGER NOT NULL, whale INTEGER, body TEXT NOT NULL, PRIMARY KEY(pool,tx,log_index));
      CREATE INDEX IF NOT EXISTS event_pool_time ON events(pool,timestamp);
      CREATE TABLE IF NOT EXISTS snapshots(pool TEXT PRIMARY KEY, body TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS live_market(pool TEXT PRIMARY KEY, body TEXT NOT NULL);")?;
    Ok(c)
}
/// Read without schema creation or any write, for isolated metric endpoints.
pub fn read_connection() -> Result<Option<Connection>> {
    let p = path();
    if !p.exists() {
        return Ok(None);
    }
    let c = Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(Some(c))
}
pub fn display(c: &Connection) -> Result<Value> {
    let row: Option<String> = c
        .query_row("SELECT body FROM display_config WHERE id=1", [], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(row
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or(json!({"token":null,"pools":[],"pool_ids":[],"version":0})))
}
#[allow(dead_code)] // Writer lives in lp-api; the RPC binary shares this module read-only.
pub fn save(c: &mut Connection, token: &str, pools: &[Value]) -> Result<Value> {
    let tx = c.transaction()?;
    let old = display(&tx)?;
    let ids = |rows: &Value| {
        rows.as_array()
            .into_iter()
            .flatten()
            .map(|p| p["pool_address"].clone())
            .collect::<Vec<_>>()
    };
    if old["token"] == token && ids(&old["pools"]) == ids(&json!(pools)) {
        return Ok(old);
    }
    let v = json!({"token":token,"pools":pools,"pool_ids":pools.iter().map(|p| &p["pool_address"]).collect::<Vec<_>>(),"version":old["version"].as_u64().unwrap_or(0)+1});
    tx.execute("INSERT INTO display_config VALUES(1,?1,?2) ON CONFLICT(id) DO UPDATE SET version=excluded.version,body=excluded.body", rusqlite::params![v["version"].as_i64(),v.to_string()])?;
    tx.commit()?;
    Ok(v)
}
pub fn fixed_pools() -> Result<Vec<Value>> {
    let config: Value = serde_json::from_str(include_str!("../../config/monad-demo-pools.json"))?;
    Ok(config["pools"].as_array().cloned().unwrap_or_default())
}

pub fn poll_ms() -> u64 {
    std::env::var("MONAD_LISTENER_POLL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000)
        .clamp(2000, 60000)
}

pub fn price_poll_ms() -> u64 {
    std::env::var("MONAD_PRICE_POLL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000)
        .clamp(500, 5000)
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    #[test]
    fn twenty_fixed_pools_have_unique_names_and_explicit_base_addresses() {
        let pools = fixed_pools().unwrap();
        assert_eq!(pools.len(), 20);
        let mut names = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        for pool in pools {
            let mut pair = pool["pair"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .split('/')
                .map(str::to_owned)
                .collect::<Vec<_>>();
            pair.sort();
            assert!(names.insert(pair));
            assert!(ids.insert(pool["pool_address"].as_str().unwrap().to_string()));
            assert_eq!(pool["base_token"], pool["token_a_address"]);
        }
    }
}
