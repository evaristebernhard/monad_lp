//! Public option quote ingestion for the RPC/data layer.
//!
//! This binary only reads public Deribit instruments and tickers and stores
//! raw quote observations in SQLite. Hedge math belongs to
//! `backend/lp-options`.

use std::{
    env,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use reqwest::Client;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

const DEFAULT_DB: &str = "data/lp_screener.db";
const DERIBIT_API: &str = "https://www.deribit.com/api/v2/public";

fn arg(name: &str, default: &str) -> String {
    let args = env::args().collect::<Vec<_>>();
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .unwrap_or_else(|| default.to_string())
}

fn has_flag(name: &str) -> bool {
    env::args().any(|value| value == name)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default()
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
}

fn ensure_schema(db: &Connection) -> Result<()> {
    db.execute_batch(include_str!("../schema.sql"))?;
    Ok(())
}

async fn public_call(client: &Client, method: &str, params: Value) -> Result<Value> {
    let query = params
        .as_object()
        .into_iter()
        .flat_map(|object| object.iter())
        .map(|(key, value)| (key.clone(), value.to_string()))
        .collect::<Vec<_>>();
    let payload: Value = client
        .get(format!("{DERIBIT_API}/{method}"))
        .query(&query)
        .header("accept", "application/json")
        .header("user-agent", "lp-option-rpc/0.1")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if let Some(error) = payload.get("error") {
        anyhow::bail!("Deribit {method}: {error}");
    }
    Ok(payload.get("result").cloned().unwrap_or(Value::Null))
}

fn underlying_params(underlying: &str) -> (&'static str, &'static str) {
    match underlying {
        "SOL" => ("USDC", "sol_usdc"),
        "ETH" => ("ETH", "eth_usd"),
        "BTC" => ("BTC", "btc_usd"),
        _ => ("", ""),
    }
}

async fn refresh_snapshot(
    db: &mut Connection,
    client: &Client,
    underlying: &str,
    tenor: &str,
    target_days: f64,
) -> Result<usize> {
    let (currency, index_name) = underlying_params(underlying);
    if currency.is_empty() {
        anyhow::bail!("unsupported option underlying {underlying}; use BTC, ETH, or SOL")
    }
    let index = number(
        public_call(client, "get_index_price", json!({"index_name": index_name}))
            .await?
            .get("index_price"),
    )
    .context("missing Deribit index price")?;
    let instruments = public_call(
        client,
        "get_instruments",
        json!({"currency": currency, "kind": "option", "expired": false}),
    )
    .await?
    .as_array()
    .cloned()
    .unwrap_or_default();
    let prefix = format!("{underlying}_USDC-");
    let instruments = instruments
        .into_iter()
        .filter(|item| {
            underlying != "SOL"
                || item
                    .get("instrument_name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect::<Vec<_>>();
    let target_ms = (now() as f64 + target_days * 86_400.0) * 1000.0;
    let expiry = instruments
        .iter()
        .filter_map(|item| number(item.get("expiration_timestamp")))
        .filter(|value| *value >= target_ms)
        .min_by(|left, right| {
            (left - target_ms)
                .abs()
                .partial_cmp(&(right - target_ms).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .context("no future Deribit option expiry")?;
    let selected = instruments
        .into_iter()
        .filter(|item| {
            item.get("is_active")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter(|item| number(item.get("expiration_timestamp")) == Some(expiry))
        .filter(|item| {
            number(item.get("strike"))
                .map(|strike| (0.70..=1.30).contains(&(strike / index)))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    let mut quotes = Vec::new();
    for instrument in selected {
        let Some(name) = instrument.get("instrument_name").and_then(Value::as_str) else {
            continue;
        };
        let ticker = public_call(client, "ticker", json!({"instrument_name": name})).await?;
        let settlement = instrument
            .get("settlement_currency")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_uppercase();
        let multiplier = if matches!(settlement.as_str(), "USDC" | "USDT" | "USD") {
            1.0
        } else if settlement == underlying {
            index
        } else {
            0.0
        };
        quotes.push((name.to_string(), instrument, ticker, multiplier));
    }
    let generated_at = now();
    let transaction = db.transaction()?;
    transaction.execute(
        "DELETE FROM option_snapshots WHERE venue='deribit' AND underlying=?1 AND tenor=?2",
        params![underlying, tenor],
    )?;
    transaction.execute(
        "INSERT INTO option_snapshots(venue,underlying,tenor,generated_at,expiry_timestamp,days_to_expiry,index_price,status) VALUES('deribit',?1,?2,?3,?4,?5,?6,'ok')",
        params![underlying, tenor, generated_at, expiry as i64, (expiry - generated_at as f64 * 1000.0) / 86_400_000.0, index],
    )?;
    let snapshot_id = transaction.last_insert_rowid();
    let quote_count = quotes.len();
    for (name, instrument, ticker, multiplier) in quotes {
        transaction.execute(
            "INSERT INTO option_quotes(snapshot_id,instrument_name,option_type,strike,expiration_timestamp,contract_size,settlement_currency,best_bid_usd,best_ask_usd,mark_price_usd,mark_iv,open_interest,raw_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                snapshot_id,
                name,
                instrument.get("option_type").and_then(Value::as_str),
                number(instrument.get("strike")),
                number(instrument.get("expiration_timestamp")),
                number(instrument.get("contract_size")),
                instrument.get("settlement_currency").and_then(Value::as_str),
                number(ticker.get("best_bid_price")).map(|value| value * multiplier),
                number(ticker.get("best_ask_price")).map(|value| value * multiplier),
                number(ticker.get("mark_price")).map(|value| value * multiplier),
                number(ticker.get("mark_iv")),
                number(ticker.get("open_interest")),
                serde_json::to_string(&json!({"instrument": instrument, "ticker": ticker}))?,
            ],
        )?;
    }
    transaction.commit()?;
    Ok(quote_count)
}

#[tokio::main]
async fn main() -> Result<()> {
    let db_path = arg("--db", DEFAULT_DB);
    let underlying = arg("--underlying", "SOL").to_ascii_uppercase();
    let tenor = arg("--tenor", "nearest");
    let days = arg("--days", "7").parse::<f64>().context("parse --days")?;
    let mut db = Connection::open(&db_path).with_context(|| format!("open {db_path}"))?;
    ensure_schema(&db)?;
    let snapshots: i64 = db.query_row(
        "SELECT count(*) FROM option_snapshots WHERE venue='deribit' AND underlying=?1 AND tenor=?2",
        params![underlying, tenor],
        |row| row.get(0),
    )?;
    if !has_flag("--refresh") {
        println!("{{\"underlying\":\"{underlying}\",\"tenor\":\"{tenor}\",\"snapshots\":{snapshots},\"refreshed\":false}}");
        return Ok(());
    }
    let client = Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let quotes = refresh_snapshot(&mut db, &client, &underlying, &tenor, days).await?;
    println!("{{\"underlying\":\"{underlying}\",\"tenor\":\"{tenor}\",\"quotes\":{quotes},\"refreshed\":true}}");
    Ok(())
}
