//! Price-history metrics for the backend layer.
//!
//! This crate deliberately owns no HTTP code and does not know about RPC
//! providers. The RPC workspace writes raw observations; this crate reads the
//! shared SQLite snapshot and derives prices, averages, and realized daily
//! volatility for the API layer.

use std::{collections::HashMap, env, path::Path};

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};

#[derive(Clone, Copy)]
struct PricePoint {
    timestamp: i64,
    price: f64,
}

#[derive(Clone)]
struct RelativePoint {
    timestamp: i64,
    token0_address: String,
    token1_address: String,
    token1_per_token0: f64,
    token0_price_usd: Option<f64>,
    token1_price_usd: Option<f64>,
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "''")
}

fn open_connection(sqlite_path: &Path) -> Result<Connection> {
    let connection = Connection::open(sqlite_path)?;
    let Some(price_path) = env::var_os("LP_PRICE_DB").map(std::path::PathBuf::from) else {
        return Ok(connection);
    };
    if price_path == sqlite_path {
        return Ok(connection);
    }
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
         );
         ",
    )?;
    Ok(connection)
}

fn db_query(sqlite_path: &Path, sql: &str) -> Result<Vec<Value>> {
    let connection = open_connection(sqlite_path)?;
    let mut statement = connection.prepare(sql)?;
    let names = statement
        .column_names()
        .iter()
        .map(|name| (*name).to_string())
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
                rusqlite::types::ValueRef::Text(value) => {
                    json!(String::from_utf8_lossy(value))
                }
                rusqlite::types::ValueRef::Blob(_) => Value::Null,
            };
            object.insert(name.clone(), value);
        }
        result.push(Value::Object(object));
    }
    Ok(result)
}

fn table_has_column(sqlite_path: &Path, table: &str, column: &str) -> Result<bool> {
    let rows = db_query(sqlite_path, &format!("PRAGMA table_info({table})"))?;
    Ok(rows
        .iter()
        .any(|row| row.get("name").and_then(Value::as_str) == Some(column)))
}

fn price_key(chain: &str, address: &str) -> String {
    let address = if address.starts_with("0x") {
        address.to_ascii_lowercase()
    } else {
        address.to_string()
    };
    format!("{chain}\u{0}{address}")
}

fn same_address(left: &str, right: &str) -> bool {
    if left.starts_with("0x") && right.starts_with("0x") {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn window_points(points: &[PricePoint], days: i64) -> Vec<PricePoint> {
    let Some(latest) = points.iter().map(|point| point.timestamp).max() else {
        return Vec::new();
    };
    let cutoff = latest - days * 86_400;
    points
        .iter()
        .copied()
        .filter(|point| point.timestamp >= cutoff && point.price.is_finite() && point.price > 0.0)
        .collect()
}

fn hourly_points(points: &[PricePoint], days: i64) -> Vec<PricePoint> {
    let Some(latest) = points.iter().map(|point| point.timestamp).max() else {
        return Vec::new();
    };
    // An hourly window of N days contains N*24 hourly prices, not N*24+1
    // boundary points. The latter would make an otherwise complete 7d series
    // fail the completeness check and unnecessarily fall back to a shorter
    // window.
    let cutoff = latest - (days.saturating_mul(24).saturating_sub(1)) * 3_600;
    let mut buckets = HashMap::<i64, PricePoint>::new();
    for point in points
        .iter()
        .copied()
        .filter(|point| point.timestamp >= cutoff && point.price.is_finite() && point.price > 0.0)
    {
        buckets.insert(point.timestamp.div_euclid(3_600), point);
    }
    let mut selected = buckets
        .into_iter()
        .map(|(hour, point)| PricePoint {
            timestamp: hour * 3_600,
            price: point.price,
        })
        .collect::<Vec<_>>();
    selected.sort_by_key(|point| point.timestamp);
    selected
}

/// Remove obvious legacy decode spikes before deriving averages or volatility.
///
/// A token can genuinely move by a large amount, so this is deliberately not
/// a hard per-return cap.  We only remove an old cluster when the latest two
/// or more observations agree within 10x and an older observation is more
/// than 100x away from that current cluster.  This catches the historical
/// sqrt-price direction/decimal bugs without hiding a single new market move.
fn sanitize_points(mut points: Vec<PricePoint>) -> Vec<PricePoint> {
    points.sort_by_key(|point| point.timestamp);
    let Some(latest) = points.last().copied() else {
        return points;
    };
    if points.len() < 3 || latest.price <= 0.0 {
        return points;
    }
    let near_latest = points
        .iter()
        .filter(|point| {
            point.price > 0.0 && (point.price / latest.price).ln().abs() <= 10.0_f64.ln()
        })
        .count();
    if near_latest < 2 {
        return points;
    }
    points.retain(|point| {
        point.price > 0.0 && (point.price / latest.price).ln().abs() <= 100.0_f64.ln()
    });
    points
}

fn realized_returns_volatility(returns: &[(i64, f64)]) -> Option<f64> {
    if returns.is_empty() {
        return None;
    }
    if returns.len() == 1 {
        // With only two hourly observations there is no unbiased sample
        // standard deviation yet. Still expose the observed move as a
        // dailyized realized-move estimate; the caller publishes the sample
        // count through iv_status so this is not mistaken for a mature series.
        let (interval, value) = returns[0];
        return Some(value.abs() * (86_400.0 / interval.max(3_600) as f64).sqrt());
    }
    let mean = returns.iter().map(|(_, value)| *value).sum::<f64>() / returns.len() as f64;
    let variance = returns
        .iter()
        .map(|(_, value)| (value - mean).powi(2))
        .sum::<f64>()
        / (returns.len() - 1) as f64;
    let mut intervals = returns
        .iter()
        .map(|(interval, _)| (*interval).max(3_600))
        .collect::<Vec<_>>();
    intervals.sort_unstable();
    let interval = intervals[intervals.len() / 2] as f64;
    Some(variance.max(0.0).sqrt() * (86_400.0 / interval).sqrt())
}

/// Strict per-pool daily volatility from exactly 24 consecutive hourly prices.
/// Input must be chronological. No token-history fallback or database work.
pub fn consecutive_hourly_daily_volatility(points: &[(i64, f64)]) -> Option<f64> {
    if points.len() != 24
        || points.iter().any(|(_, p)| !p.is_finite() || *p <= 0.)
        || points.windows(2).any(|w| w[1].0 - w[0].0 != 3600)
    {
        return None;
    }
    let returns = points
        .windows(2)
        .map(|w| (3600, (w[1].1 / w[0].1).ln()))
        .collect::<Vec<_>>();
    realized_returns_volatility(&returns).filter(|v| v.is_finite())
}

fn volatility(points: &[PricePoint], days: i64) -> (Option<f64>, usize) {
    let selected = hourly_points(points, days);
    let returns = selected
        .windows(2)
        .filter_map(|window| {
            let previous = window[0];
            let current = window[1];
            if current.price <= 0.0 || previous.price <= 0.0 {
                return None;
            }
            Some((
                (current.timestamp - previous.timestamp).max(1),
                (current.price / previous.price).ln(),
            ))
        })
        .collect::<Vec<_>>();
    // A few identical snapshots collected within a short interval do not
    // prove that volatility is zero. Treat that state as insufficient history
    // until the series spans at least one full day; a genuinely flat series
    // with a longer observation window may still report a real 0.0.
    if selected.len() >= 2
        && selected
            .last()
            .zip(selected.first())
            .is_some_and(|(last, first)| last.timestamp - first.timestamp < 86_400)
        && returns.iter().all(|(_, value)| value.abs() < 1e-12)
    {
        return (None, returns.len());
    }
    (realized_returns_volatility(&returns), selected.len())
}

/// A 7-day volatility value is valid only for a complete hourly window.
/// Returning NULL for a short or gapped series prevents a few RPC snapshots
/// from being presented as a 7-day estimate.
fn complete_window_volatility(points: &[PricePoint], days: i64) -> (Option<f64>, usize) {
    let selected = hourly_points(points, days);
    let required = days.saturating_mul(24).max(2) as usize;
    if selected.len() != required
        || selected
            .windows(2)
            .any(|window| window[1].timestamp - window[0].timestamp != 3_600)
    {
        return (None, selected.len());
    }
    let returns = selected
        .windows(2)
        .filter_map(|window| {
            let previous = window[0];
            let current = window[1];
            (previous.price > 0.0 && current.price > 0.0).then_some((
                current.timestamp - previous.timestamp,
                (current.price / previous.price).ln(),
            ))
        })
        .collect::<Vec<_>>();
    (realized_returns_volatility(&returns), selected.len())
}

/// Prefer the requested observation window, but use a wider stored window
/// when the token has too few recent points. The returned volatility is still
/// dailyized by the observed sampling interval, so a 7d/30d fallback remains
/// a daily volatility estimate and is reported with a low-sample status.
fn volatility_with_fallback(
    points: &[PricePoint],
    preferred_days: i64,
) -> (Option<f64>, usize, i64) {
    let mut windows = vec![preferred_days];
    for days in [1_i64, 3, 7, 30] {
        if !windows.contains(&days) {
            windows.push(days);
        }
    }
    let mut last_samples = 0;
    let mut last_window = preferred_days;
    for days in windows {
        let (value, samples) = volatility(points, days);
        if value.is_some() {
            return (value, samples, days);
        }
        if samples > 0 {
            last_samples = samples;
            last_window = days;
        }
    }
    (None, last_samples, last_window)
}

fn relative_volatility_with_fallback(
    points: &[PricePoint],
    preferred_days: i64,
) -> (Option<f64>, usize, i64) {
    let (value, samples, window) = volatility_with_fallback(points, preferred_days);
    if samples < 3 && value.is_some_and(|value| value.abs() < 1e-12) {
        (None, samples, window)
    } else {
        (value, samples, window)
    }
}

fn average_price(points: &[PricePoint], days: i64) -> Option<f64> {
    let selected = window_points(points, days);
    if selected.is_empty() {
        return None;
    }
    Some(selected.iter().map(|point| point.price).sum::<f64>() / selected.len() as f64)
}

fn json_f64(value: Option<f64>) -> Value {
    value.map_or(Value::Null, |value| json!(value))
}

fn history_points(sqlite_path: &Path, chain: &str, address: &str) -> Result<Vec<PricePoint>> {
    let normalized = if address.starts_with("0x") {
        address.to_ascii_lowercase()
    } else {
        address.to_string()
    };
    let rows = db_query(
        sqlite_path,
        &format!(
            "SELECT timestamp, price_usd FROM token_price_history WHERE chain = '{}' AND address = '{}' ORDER BY timestamp",
            escape(chain),
            escape(&normalized)
        ),
    )?;
    let points = rows
        .into_iter()
        .filter_map(|row| {
            Some(PricePoint {
                timestamp: row.get("timestamp")?.as_i64()?,
                price: row.get("price_usd")?.as_f64()?,
            })
        })
        .filter(|point| point.timestamp > 0 && point.price.is_finite() && point.price > 0.0)
        .collect::<Vec<_>>();
    Ok(sanitize_points(points))
}

/// Build a token-market pair series from two USD histories. Both sides are
/// bucketed to UTC hours and only common buckets are used. This is an
/// external-market proxy, not a claim about any particular pool's executable
/// price.
fn relative_token_history(left: &[PricePoint], right: &[PricePoint]) -> Vec<PricePoint> {
    let left_by_hour = left
        .iter()
        .filter(|point| point.price.is_finite() && point.price > 0.0)
        .map(|point| (point.timestamp.div_euclid(3_600), point.price))
        .collect::<HashMap<_, _>>();
    let mut points = right
        .iter()
        .filter_map(|point| {
            if !point.price.is_finite() || point.price <= 0.0 {
                return None;
            }
            let hour = point.timestamp.div_euclid(3_600);
            let left_price = left_by_hour.get(&hour).copied()?;
            let price = point.price / left_price;
            (price.is_finite() && price > 0.0).then_some(PricePoint {
                timestamp: hour * 3_600,
                price,
            })
        })
        .collect::<Vec<_>>();
    points.sort_by_key(|point| point.timestamp);
    points.dedup_by_key(|point| point.timestamp);
    points
}

fn choose_relative_history<'a>(
    pool_local: &'a [PricePoint],
    token_market: &'a [PricePoint],
) -> (&'a [PricePoint], &'static str) {
    let local_is_usable = complete_window_volatility(pool_local, 1).0.is_some()
        || complete_window_volatility(pool_local, 7).0.is_some();
    let market_is_usable = complete_window_volatility(token_market, 1).0.is_some()
        || complete_window_volatility(token_market, 7).0.is_some();
    if pool_local.is_empty() && !token_market.is_empty() {
        (token_market, "token_price_history_market_proxy")
    } else if local_is_usable
        || token_market.is_empty()
        || (!market_is_usable && token_market.len() <= pool_local.len())
    {
        (pool_local, "pool_price_history")
    } else {
        (token_market, "token_price_history_market_proxy")
    }
}

fn token_report_from_points(chain: &str, address: &str, points: &[PricePoint]) -> Value {
    let latest = points.last().copied();
    let (vol_1d, samples_1d) = complete_window_volatility(points, 1);
    let (vol_3d, samples_3d, window_3d) = volatility_with_fallback(points, 3);
    let (vol_7d, samples_7d) = complete_window_volatility(points, 7);
    let (vol_30d, samples_30d, window_30d) = volatility_with_fallback(points, 30);
    let suspicious = [vol_1d, vol_3d, vol_7d, vol_30d]
        .into_iter()
        .flatten()
        .any(|value| value > 1.0);
    json!({
        "chain": chain,
        "address": address,
        "latest": {
            "timestamp": latest.map(|point| point.timestamp),
            "price_usd": latest.map(|point| point.price),
        },
        "averages": {
            "1d": average_price(points, 1),
            "3d": average_price(points, 3),
            "7d": average_price(points, 7),
            "30d": average_price(points, 30),
        },
        "daily_vol": {
            "1d": vol_1d,
            "3d": vol_3d,
            "7d": vol_7d,
            "30d": vol_30d,
        },
        "samples": {
            "1d": samples_1d,
            "3d": samples_3d,
            "7d": samples_7d,
            "30d": samples_30d,
        },
        "windows_used_days": {
            "1d": 1,
            "3d": window_3d,
            "7d": 7,
            "30d": window_30d,
        },
        "status": if latest.is_none() {
            "missing_price_history"
        } else if suspicious {
            "suspicious_daily_volatility"
        } else if samples_1d < 2 && samples_3d < 2 && samples_7d < 2 {
            "low_sample"
        } else {
            "ok"
        },
    })
}

/// Return one token's backend-owned history metrics for the HTTP API.
pub fn token_volatility_report(sqlite_path: &Path, chain: &str, address: &str) -> Result<Value> {
    let points = history_points(sqlite_path, chain, address)?;
    Ok(token_report_from_points(chain, address, &points))
}

/// Return the two token reports and pair-relative volatility for one pool.
pub fn pool_volatility_report(
    sqlite_path: &Path,
    chain: &str,
    pool_address: &str,
) -> Result<Option<Value>> {
    let pool_rows = db_query(
        sqlite_path,
        &format!(
            "SELECT pair, protocol, token_a_address, token_a_symbol, token_b_address, token_b_symbol FROM pools WHERE chain = '{}' AND lower(pool_address) = lower('{}') LIMIT 1",
            escape(chain),
            escape(pool_address)
        ),
    )?;
    let Some(pool) = pool_rows.first() else {
        return Ok(None);
    };
    let token_a_address = pool
        .get("token_a_address")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let token_b_address = pool
        .get("token_b_address")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let token_a_points = history_points(sqlite_path, chain, token_a_address)?;
    let token_b_points = history_points(sqlite_path, chain, token_b_address)?;
    let pool_status_filter = if table_has_column(sqlite_path, "pool_price_history", "price_status")?
    {
        " AND coalesce(price_status, 'priced') = 'priced'"
    } else {
        ""
    };
    let relative_rows = db_query(
        sqlite_path,
        &format!(
            "SELECT timestamp, token1_per_token0 FROM pool_price_history WHERE chain = '{}' AND lower(pool_address) = lower('{}'){} ORDER BY timestamp",
            escape(chain),
            escape(pool_address),
            pool_status_filter
        ),
    )?;
    let relative_points = sanitize_points(
        relative_rows
            .into_iter()
            .filter_map(|row| {
                Some(PricePoint {
                    timestamp: row.get("timestamp")?.as_i64()?,
                    price: row.get("token1_per_token0")?.as_f64()?,
                })
            })
            .filter(|point| point.timestamp > 0 && point.price.is_finite() && point.price > 0.0)
            .collect(),
    );
    let market_relative_points = relative_token_history(&token_a_points, &token_b_points);
    let (selected_relative_points, relative_source) =
        choose_relative_history(&relative_points, &market_relative_points);
    let (relative_1d, relative_samples_1d) =
        complete_window_volatility(selected_relative_points, 1);
    let (relative_3d, relative_samples_3d, relative_window_3d) =
        relative_volatility_with_fallback(selected_relative_points, 3);
    let (relative_7d, relative_samples_7d) =
        complete_window_volatility(selected_relative_points, 7);
    let (relative_30d, relative_samples_30d, relative_window_30d) =
        relative_volatility_with_fallback(selected_relative_points, 30);
    let has_relative_history = !selected_relative_points.is_empty();
    let pair_1d = relative_1d;
    let pair_3d = relative_3d;
    let suspicious = [pair_1d, pair_3d, relative_7d, relative_30d]
        .into_iter()
        .flatten()
        .any(|value| value > 1.0);
    Ok(Some(json!({
        "chain": chain,
        "pool_address": pool_address,
        "pair": pool.get("pair").cloned().unwrap_or(Value::Null),
        "protocol": pool.get("protocol").cloned().unwrap_or(Value::Null),
        "token_a": {
            "address": token_a_address,
            "symbol": pool.get("token_a_symbol").cloned().unwrap_or(Value::Null),
            "metrics": token_report_from_points(chain, token_a_address, &token_a_points),
        },
        "token_b": {
            "address": token_b_address,
            "symbol": pool.get("token_b_symbol").cloned().unwrap_or(Value::Null),
            "metrics": token_report_from_points(chain, token_b_address, &token_b_points),
        },
        "pair_vol": {
            "1d": pair_1d,
            "3d": pair_3d,
            "7d": relative_7d,
            "30d": relative_30d,
            "annualized": relative_7d.or(relative_1d),
            "volatility_window_days": if relative_7d.is_some() {
                7
            } else if relative_1d.is_some() {
                1
            } else {
                0
            },
            "samples_1d": relative_samples_1d,
            "samples_3d": relative_samples_3d,
            "samples_7d": relative_samples_7d,
            "samples_30d": relative_samples_30d,
            "source": if has_relative_history { relative_source } else { "missing" },
            "price_semantics": if relative_source == "pool_price_history" {
                "pool_local"
            } else if has_relative_history {
                "external_market_pair_proxy"
            } else {
                "missing"
            },
            "windows_used_days": {
                "1d": 1,
                "3d": relative_window_3d,
                "7d": 7,
                "30d": relative_window_30d,
            },
        },
        "status": if pair_1d.is_none()
            && pair_3d.is_none()
            && relative_7d.is_none()
            && relative_30d.is_none()
        {
            if has_relative_history {
                "insufficient_pair_history"
            } else {
                "missing_pair_history"
            }
        } else if suspicious { "suspicious_daily_volatility" } else { "ok" },
    })))
}

fn is_stable_symbol(symbol: &str) -> bool {
    matches!(
        symbol.trim().to_ascii_uppercase().as_str(),
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
            | "USD"
            | "LUSD"
    )
}

/// Attach backend-derived price and volatility fields to API pool rows.
pub fn attach_runtime_volatility(sqlite_path: &Path, mut items: Vec<Value>) -> Result<Vec<Value>> {
    if items.is_empty() {
        return Ok(items);
    }
    let mut tokens = HashMap::<String, (String, String)>::new();
    let mut pool_keys = HashMap::<String, (String, String)>::new();
    for item in &items {
        let Some(object) = item.as_object() else {
            continue;
        };
        let chain = object.get("chain").and_then(Value::as_str).unwrap_or("");
        if let Some(address) = object
            .get("pool_address")
            .and_then(Value::as_str)
            .filter(|address| !address.is_empty())
        {
            pool_keys.insert(
                format!("{chain}\u{0}{address}"),
                (chain.to_string(), address.to_string()),
            );
        }
        for field in ["token_a_address", "token_b_address"] {
            let Some(address) = object
                .get(field)
                .and_then(Value::as_str)
                .filter(|address| !address.is_empty())
            else {
                continue;
            };
            let key = price_key(chain, address);
            tokens
                .entry(key)
                .or_insert_with(|| (chain.to_string(), address.to_string()));
        }
    }
    if tokens.is_empty() && pool_keys.is_empty() {
        return Ok(items);
    }
    let rows = if tokens.is_empty() {
        Vec::new()
    } else {
        let conditions = tokens
            .values()
            .map(|(chain, address)| {
                let normalized = if address.starts_with("0x") {
                    address.to_ascii_lowercase()
                } else {
                    address.clone()
                };
                format!(
                    "(chain = '{}' AND address = '{}')",
                    escape(chain),
                    escape(&normalized)
                )
            })
            .collect::<Vec<_>>();
        let sql = format!(
            "SELECT chain, address, timestamp, price_usd FROM token_price_history WHERE {} ORDER BY chain, address, timestamp",
            conditions.join(" OR ")
        );
        db_query(sqlite_path, &sql)?
    };
    let mut history = HashMap::<String, Vec<PricePoint>>::new();
    for row in rows {
        let Some(chain) = row.get("chain").and_then(Value::as_str) else {
            continue;
        };
        let Some(address) = row.get("address").and_then(Value::as_str) else {
            continue;
        };
        let Some(timestamp) = row.get("timestamp").and_then(Value::as_i64) else {
            continue;
        };
        let Some(price) = row.get("price_usd").and_then(Value::as_f64) else {
            continue;
        };
        history
            .entry(price_key(chain, address))
            .or_default()
            .push(PricePoint { timestamp, price });
    }
    for points in history.values_mut() {
        let sanitized = sanitize_points(std::mem::take(points));
        *points = sanitized;
    }
    let mut relative_history = HashMap::<String, Vec<PricePoint>>::new();
    let mut pool_token0_history = HashMap::<String, Vec<PricePoint>>::new();
    let mut pool_token1_history = HashMap::<String, Vec<PricePoint>>::new();
    let mut relative_latest = HashMap::<String, RelativePoint>::new();
    if !pool_keys.is_empty() {
        let pool_conditions = pool_keys
            .values()
            .map(|(chain, address)| {
                format!(
                    "(chain = '{}' AND pool_address = '{}')",
                    escape(chain),
                    escape(address)
                )
            })
            .collect::<Vec<_>>();
        let pool_usd_columns =
            if table_has_column(sqlite_path, "pool_price_history", "token0_price_usd")?
                && table_has_column(sqlite_path, "pool_price_history", "token1_price_usd")?
            {
                "token0_price_usd, token1_price_usd"
            } else {
                "NULL AS token0_price_usd, NULL AS token1_price_usd"
            };
        let pool_status_filter =
            if table_has_column(sqlite_path, "pool_price_history", "price_status")? {
                " AND coalesce(price_status, 'priced') = 'priced'"
            } else {
                ""
            };
        let relative_sql = format!(
            "SELECT chain, pool_address, timestamp, token0_address, token1_address, token1_per_token0, {pool_usd_columns} FROM pool_price_history WHERE ({}){} ORDER BY chain, pool_address, timestamp",
            pool_conditions.join(" OR "),
            pool_status_filter
        );
        for row in db_query(sqlite_path, &relative_sql)? {
            let Some(chain) = row.get("chain").and_then(Value::as_str) else {
                continue;
            };
            let Some(address) = row.get("pool_address").and_then(Value::as_str) else {
                continue;
            };
            let Some(timestamp) = row.get("timestamp").and_then(Value::as_i64) else {
                continue;
            };
            let Some(token0_address) = row.get("token0_address").and_then(Value::as_str) else {
                continue;
            };
            let Some(token1_address) = row.get("token1_address").and_then(Value::as_str) else {
                continue;
            };
            let Some(price) = row.get("token1_per_token0").and_then(Value::as_f64) else {
                continue;
            };
            let token0_price_usd = row.get("token0_price_usd").and_then(Value::as_f64);
            let token1_price_usd = row.get("token1_price_usd").and_then(Value::as_f64);
            let key = format!("{chain}\u{0}{address}");
            relative_history
                .entry(key.clone())
                .or_default()
                .push(PricePoint { timestamp, price });
            if let Some(price) = token0_price_usd.filter(|value| value.is_finite() && *value > 0.0)
            {
                pool_token0_history
                    .entry(key.clone())
                    .or_default()
                    .push(PricePoint { timestamp, price });
            }
            if let Some(price) = token1_price_usd.filter(|value| value.is_finite() && *value > 0.0)
            {
                pool_token1_history
                    .entry(key.clone())
                    .or_default()
                    .push(PricePoint { timestamp, price });
            }
            if relative_latest
                .get(&key)
                .is_none_or(|point| timestamp >= point.timestamp)
            {
                relative_latest.insert(
                    key,
                    RelativePoint {
                        timestamp,
                        token0_address: token0_address.to_string(),
                        token1_address: token1_address.to_string(),
                        token1_per_token0: price,
                        token0_price_usd,
                        token1_price_usd,
                    },
                );
            }
        }
    }
    for points in pool_token0_history.values_mut() {
        *points = sanitize_points(std::mem::take(points));
    }
    for points in pool_token1_history.values_mut() {
        *points = sanitize_points(std::mem::take(points));
    }
    for item in &mut items {
        let Some(object) = item.as_object_mut() else {
            continue;
        };
        let pool_chain = object.get("chain").and_then(Value::as_str).unwrap_or("");
        let pool_address = object
            .get("pool_address")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let token_a_address = object
            .get("token_a_address")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let token_b_address = object
            .get("token_b_address")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let token_a = object
            .get("token_a_address")
            .and_then(Value::as_str)
            .map(|address| {
                history
                    .get(&price_key(pool_chain, address))
                    .cloned()
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let token_b = object
            .get("token_b_address")
            .and_then(Value::as_str)
            .map(|address| {
                history
                    .get(&price_key(pool_chain, address))
                    .cloned()
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let pool_local_relative = relative_history
            .get(&format!("{pool_chain}\u{0}{pool_address}"))
            .cloned()
            .unwrap_or_default();
        let pool_key = format!("{pool_chain}\u{0}{pool_address}");
        let latest_relative = relative_latest.get(&pool_key);
        let pool_token0 = pool_token0_history
            .get(&pool_key)
            .cloned()
            .unwrap_or_default();
        let pool_token1 = pool_token1_history
            .get(&pool_key)
            .cloned()
            .unwrap_or_default();
        let (pool_token_a, pool_token_b) = if let Some(latest) = latest_relative {
            if same_address(&token_a_address, &latest.token0_address)
                && same_address(&token_b_address, &latest.token1_address)
            {
                (pool_token0.clone(), pool_token1.clone())
            } else if same_address(&token_a_address, &latest.token1_address)
                && same_address(&token_b_address, &latest.token0_address)
            {
                (pool_token1.clone(), pool_token0.clone())
            } else {
                (Vec::new(), Vec::new())
            }
        } else {
            (Vec::new(), Vec::new())
        };
        let token_a_market = history
            .get(&price_key(pool_chain, &token_a_address))
            .cloned()
            .unwrap_or_default();
        let token_b_market = history
            .get(&price_key(pool_chain, &token_b_address))
            .cloned()
            .unwrap_or_default();
        let market_relative = relative_token_history(&token_a_market, &token_b_market);
        let (relative, relative_source) =
            choose_relative_history(&pool_local_relative, &market_relative);
        let (relative_one_day, relative_one_day_samples) = complete_window_volatility(relative, 1);
        let (relative_three_day, relative_three_day_samples, relative_three_day_window) =
            relative_volatility_with_fallback(relative, 3);
        let (relative_seven_day, relative_seven_day_samples) =
            complete_window_volatility(relative, 7);
        let (relative_thirty_day, relative_thirty_day_samples, relative_thirty_day_window) =
            relative_volatility_with_fallback(relative, 30);
        let has_relative_history = !relative.is_empty();
        let one_day = relative_one_day;
        let three_day = relative_three_day;
        let one_day_samples = relative_one_day_samples;
        let three_day_samples = relative_three_day_samples;
        let token_a_symbol = object
            .get("token_a_symbol")
            .and_then(Value::as_str)
            .unwrap_or("");
        let token_b_symbol = object
            .get("token_b_symbol")
            .and_then(Value::as_str)
            .unwrap_or("");
        // Stable assets are normally quiet, but hard-coding their volatility
        // to zero hides depegs. Use the same observed USD history as every
        // other token; a missing/short series remains NULL.
        let token_a_daily_vol = complete_window_volatility(&token_a, 1).0;
        let token_b_daily_vol = complete_window_volatility(&token_b, 1).0;
        // Token history is a chain-wide USD anchor. The displayed current
        // quote must still be pool-local: use this pool's latest decoded
        // token1/token0 ratio and re-anchor one side (stablecoin first, then
        // token A, then token B) instead of copying the same global pair
        // price into every pool.
        let mut current_a = pool_token_a
            .last()
            .map(|point| point.price)
            .or_else(|| token_a.last().map(|point| point.price));
        let mut current_b = pool_token_b
            .last()
            .map(|point| point.price)
            .or_else(|| token_b.last().map(|point| point.price));
        if let Some(relative) = latest_relative {
            if same_address(
                object
                    .get("token_a_address")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                &relative.token0_address,
            ) {
                current_a = relative.token0_price_usd.or(current_a);
                current_b = relative.token1_price_usd.or(current_b);
            } else if same_address(
                object
                    .get("token_a_address")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                &relative.token1_address,
            ) {
                current_a = relative.token1_price_usd.or(current_a);
                current_b = relative.token0_price_usd.or(current_b);
            }
            let local_ratio = if same_address(
                object
                    .get("token_a_address")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                &relative.token0_address,
            ) && same_address(
                object
                    .get("token_b_address")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                &relative.token1_address,
            ) {
                Some(relative.token1_per_token0)
            } else if same_address(
                object
                    .get("token_a_address")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                &relative.token1_address,
            ) && same_address(
                object
                    .get("token_b_address")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                &relative.token0_address,
            ) {
                (relative.token1_per_token0 > 0.0).then_some(1.0 / relative.token1_per_token0)
            } else {
                None
            };
            if let Some(local_ratio) = local_ratio.filter(|ratio| ratio.is_finite() && *ratio > 0.0)
            {
                if is_stable_symbol(token_a_symbol) {
                    current_a = Some(1.0);
                    current_b = Some(local_ratio);
                } else if is_stable_symbol(token_b_symbol) {
                    current_a = Some(1.0 / local_ratio);
                    current_b = Some(1.0);
                } else if let Some(anchor) = current_a {
                    current_b = Some(anchor * local_ratio);
                } else if let Some(anchor) = current_b {
                    current_a = Some(anchor / local_ratio);
                }
            }
        }
        object.insert("token_a_price_usd".into(), json_f64(current_a));
        object.insert("token_b_price_usd".into(), json_f64(current_b));
        object.insert(
            "token_a_avg_price_1d_usd".into(),
            json_f64(average_price(&pool_token_a, 1)),
        );
        object.insert(
            "token_b_avg_price_1d_usd".into(),
            json_f64(average_price(&pool_token_b, 1)),
        );
        object.insert(
            "token_a_avg_price_3d_usd".into(),
            json_f64(average_price(&pool_token_a, 3)),
        );
        object.insert(
            "token_b_avg_price_3d_usd".into(),
            json_f64(average_price(&pool_token_b, 3)),
        );
        object.insert(
            "token_a_avg_price_7d_usd".into(),
            json_f64(average_price(&pool_token_a, 7)),
        );
        object.insert(
            "token_b_avg_price_7d_usd".into(),
            json_f64(average_price(&pool_token_b, 7)),
        );
        object.insert(
            "token_a_avg_price_usd".into(),
            json_f64(average_price(&pool_token_a, 30)),
        );
        object.insert(
            "token_b_avg_price_usd".into(),
            json_f64(average_price(&pool_token_b, 30)),
        );
        object.insert("iv_1d_annualized".into(), json_f64(one_day));
        object.insert("iv_3d_annualized".into(), json_f64(three_day));
        object.insert("iv_7d_annualized".into(), json_f64(relative_seven_day));
        object.insert("iv_30d_annualized".into(), json_f64(relative_thirty_day));
        object.insert("iv_90d_annualized".into(), Value::Null);
        object.insert(
            "iv_annualized".into(),
            json_f64(relative_seven_day.or(relative_one_day)),
        );
        object.insert(
            "volatility_window_days".into(),
            json!(if relative_seven_day.is_some() {
                7
            } else if relative_one_day.is_some() {
                1
            } else {
                0
            }),
        );
        object.insert("realized_vol_1d".into(), json_f64(one_day));
        object.insert("realized_vol_3d".into(), json_f64(three_day));
        object.insert("token_a_daily_vol".into(), json_f64(token_a_daily_vol));
        object.insert("token_b_daily_vol".into(), json_f64(token_b_daily_vol));
        object.insert("iv_1d_sample_count".into(), json!(one_day_samples));
        object.insert("iv_3d_sample_count".into(), json!(three_day_samples));
        object.insert("historical_pair_vol_1d".into(), json_f64(one_day));
        object.insert("historical_pair_vol_3d".into(), json_f64(three_day));
        object.insert(
            "historical_pair_vol_7d".into(),
            json_f64(relative_seven_day),
        );
        object.insert(
            "historical_pair_vol_source".into(),
            json!(if has_relative_history {
                relative_source
            } else {
                "missing"
            }),
        );
        object.insert(
            "historical_pair_price_semantics".into(),
            json!(if relative_source == "pool_price_history" {
                "pool_local"
            } else if has_relative_history {
                "external_market_pair_proxy"
            } else {
                "missing"
            }),
        );
        object.insert("historical_pair_samples_1d".into(), json!(one_day_samples));
        object.insert(
            "historical_pair_samples_3d".into(),
            json!(three_day_samples),
        );
        object.insert(
            "historical_pair_samples_7d".into(),
            json!(relative_seven_day_samples),
        );
        object.insert(
            "iv_sample_count_30d".into(),
            json!(relative_thirty_day_samples),
        );
        object.insert("iv_1d_window_days".into(), json!(1));
        object.insert(
            "iv_3d_window_days".into(),
            json!(if relative_three_day.is_some() {
                relative_three_day_window
            } else {
                3
            }),
        );
        object.insert("iv_7d_window_days".into(), json!(7));
        object.insert(
            "iv_30d_window_days".into(),
            json!(if relative_thirty_day.is_some() {
                relative_thirty_day_window
            } else {
                30
            }),
        );
        object.insert(
            "iv_status".into(),
            json!(if one_day.is_some()
                || three_day.is_some()
                || relative_seven_day.is_some()
                || relative_thirty_day.is_some()
            {
                if one_day_samples < 2
                    && three_day_samples < 2
                    && relative_seven_day_samples < 2
                    && relative_thirty_day_samples < 2
                {
                    if has_relative_history {
                        "low_sample_backend_relative_realized_volatility"
                    } else {
                        "low_sample_backend_realized_volatility"
                    }
                } else {
                    if relative_three_day_window > 3 || relative_thirty_day_window > 30 {
                        "fallback_window_backend_realized_volatility"
                    } else if relative_seven_day.is_none() {
                        "one_day_fallback_backend_realized_volatility"
                    } else {
                        "backend_pool_realized_volatility"
                    }
                }
            } else if one_day_samples > 0
                || three_day_samples > 0
                || relative_seven_day_samples > 0
                || relative_thirty_day_samples > 0
            {
                "insufficient_pair_history"
            } else {
                "missing_price_history"
            }),
        );
        object.insert(
            "iv_source".into(),
            json!(if has_relative_history {
                if relative_source == "pool_price_history" {
                    "backend_pool_price_history"
                } else {
                    "backend_token_market_history"
                }
            } else {
                "missing"
            }),
        );
        object.insert("iv_backend_pool".into(), json!(pool_address));
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::{attach_runtime_volatility, pool_volatility_report};
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn test_db() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("lp-metrics-{suffix}.db"))
    }

    #[test]
    fn pool_volatility_falls_back_to_token_market_history() {
        let path = test_db();
        let db = Connection::open(&path).expect("open test db");
        db.execute_batch(
            "CREATE TABLE pools (
               chain TEXT, pool_address TEXT, pair TEXT, protocol TEXT,
               token_a_address TEXT, token_a_symbol TEXT,
               token_b_address TEXT, token_b_symbol TEXT
             );
             CREATE TABLE token_price_history (
               chain TEXT, address TEXT, timestamp INTEGER, price_usd REAL
             );
             CREATE TABLE pool_price_history (
               chain TEXT, pool_address TEXT, timestamp INTEGER,
               token0_address TEXT, token1_address TEXT,
               token1_per_token0 REAL
             );
             INSERT INTO pools VALUES
               ('test', 'pool-1', 'A/B', 'test', 'A', 'A', 'B', 'B');
             INSERT INTO token_price_history VALUES
               ('test', 'A', 100000, 1.0), ('test', 'A', 103600, 2.0),
               ('test', 'B', 100000, 1.0), ('test', 'B', 103600, 1.0);",
        )
        .expect("seed test db");
        drop(db);

        let report = pool_volatility_report(&path, "test", "pool-1")
            .expect("report")
            .expect("pool");
        assert_eq!(
            report["pair_vol"]["source"],
            Value::String("token_price_history_market_proxy".into())
        );
        assert!(report["pair_vol"]["1d"].is_null());
        assert_eq!(
            report["status"],
            Value::String("suspicious_daily_volatility".into())
        );
        fs::remove_file(path).expect("remove test db");
    }

    #[test]
    fn pool_volatility_uses_the_pool_series_when_present() {
        let path = test_db();
        let db = Connection::open(&path).expect("open test db");
        db.execute_batch(
            "CREATE TABLE pools (
               chain TEXT, pool_address TEXT, pair TEXT, protocol TEXT,
               token_a_address TEXT, token_a_symbol TEXT,
               token_b_address TEXT, token_b_symbol TEXT
             );
             CREATE TABLE token_price_history (
               chain TEXT, address TEXT, timestamp INTEGER, price_usd REAL
             );
             CREATE TABLE pool_price_history (
               chain TEXT, pool_address TEXT, timestamp INTEGER,
               token0_address TEXT, token1_address TEXT,
               token1_per_token0 REAL
             );
             INSERT INTO pools VALUES
               ('test', 'pool-1', 'A/B', 'test', 'A', 'A', 'B', 'B');
             INSERT INTO pool_price_history VALUES
               ('test', 'pool-1', 100000, 'A', 'B', 1.0),
               ('test', 'pool-1', 103600, 'A', 'B', 2.0);",
        )
        .expect("seed test db");
        drop(db);

        let report = pool_volatility_report(&path, "test", "pool-1")
            .expect("report")
            .expect("pool");
        assert_eq!(
            report["pair_vol"]["source"],
            Value::String("pool_price_history".into())
        );
        assert!(report["pair_vol"]["1d"].is_null());
        fs::remove_file(path).expect("remove test db");
    }

    #[test]
    fn unavailable_pool_observations_do_not_become_pair_volatility() {
        let path = test_db();
        let db = Connection::open(&path).expect("open test db");
        db.execute_batch(
            "CREATE TABLE pools (
               chain TEXT, pool_address TEXT, pair TEXT, protocol TEXT,
               token_a_address TEXT, token_a_symbol TEXT,
               token_b_address TEXT, token_b_symbol TEXT
             );
             CREATE TABLE token_price_history (
               chain TEXT, address TEXT, timestamp INTEGER, price_usd REAL
             );
             CREATE TABLE pool_price_history (
               chain TEXT, pool_address TEXT, timestamp INTEGER,
               token0_address TEXT, token1_address TEXT,
               token1_per_token0 REAL, price_status TEXT
             );
             INSERT INTO pools VALUES
               ('test', 'pool-1', 'A/B', 'test', 'A', 'A', 'B', 'B');
             INSERT INTO pool_price_history VALUES
               ('test', 'pool-1', 100000, 'A', 'B', NULL, 'unavailable'),
               ('test', 'pool-1', 103600, 'A', 'B', NULL, 'unavailable');",
        )
        .expect("seed test db");
        drop(db);

        let report = pool_volatility_report(&path, "test", "pool-1")
            .expect("report")
            .expect("pool");
        assert_eq!(
            report["pair_vol"]["source"],
            Value::String("missing".into())
        );
        assert!(report["pair_vol"]["1d"].is_null());
        fs::remove_file(path).expect("remove test db");
    }

    #[test]
    fn pool_price_averages_use_pool_local_usd_snapshots() {
        let path = test_db();
        let db = Connection::open(&path).expect("open test db");
        db.execute_batch(
            "CREATE TABLE token_price_history (
               chain TEXT, address TEXT, timestamp INTEGER, price_usd REAL
             );
             CREATE TABLE pool_price_history (
               chain TEXT, pool_address TEXT, timestamp INTEGER,
               token0_address TEXT, token1_address TEXT,
               token1_per_token0 REAL,
               token0_price_usd REAL, token1_price_usd REAL
             );
             INSERT INTO token_price_history VALUES
               ('test', 'A', 100000, 1.0), ('test', 'A', 103600, 2.0),
               ('test', 'B', 100000, 1.0), ('test', 'B', 103600, 1.0);
             INSERT INTO pool_price_history VALUES
               ('test', 'pool-1', 100000, 'A', 'B', 1.0, 1.0, 1.0),
               ('test', 'pool-1', 103600, 'A', 'B', 3.0, 1.0, 3.0);",
        )
        .expect("seed test db");
        drop(db);

        let items = vec![json!({
            "chain": "test",
            "pool_address": "pool-1",
            "token_a_address": "A",
            "token_b_address": "B",
            "token_a_symbol": "A",
            "token_b_symbol": "B",
            "iv_annualized": 99.0,
            "iv_30d_annualized": 99.0,
            "iv_source": "legacy_pool_volatility"
        })];
        let attached = attach_runtime_volatility(&path, items)
            .expect("attach")
            .pop()
            .expect("item");
        assert_eq!(attached["token_a_avg_price_1d_usd"], 1.0);
        assert_eq!(attached["token_b_avg_price_1d_usd"], 2.0);
        assert_eq!(attached["iv_source"], "backend_pool_price_history");
        assert!(attached["iv_annualized"].is_null());
        assert!(attached["iv_30d_annualized"]
            .as_f64()
            .is_some_and(|value| value > 0.0));
        fs::remove_file(path).expect("remove test db");
    }

    #[test]
    fn short_flat_pool_history_is_not_reported_as_zero() {
        let path = test_db();
        let db = Connection::open(&path).expect("open test db");
        db.execute_batch(
            "CREATE TABLE pools (
               chain TEXT, pool_address TEXT, pair TEXT, protocol TEXT,
               token_a_address TEXT, token_a_symbol TEXT,
               token_b_address TEXT, token_b_symbol TEXT
             );
             CREATE TABLE token_price_history (
               chain TEXT, address TEXT, timestamp INTEGER, price_usd REAL
             );
             CREATE TABLE pool_price_history (
               chain TEXT, pool_address TEXT, timestamp INTEGER,
               token0_address TEXT, token1_address TEXT,
               token1_per_token0 REAL
             );
             INSERT INTO pools VALUES
               ('test', 'pool-1', 'A/B', 'test', 'A', 'A', 'B', 'B');
             INSERT INTO pool_price_history VALUES
               ('test', 'pool-1', 100000, 'A', 'B', 1.0),
               ('test', 'pool-1', 103600, 'A', 'B', 1.0),
               ('test', 'pool-1', 107200, 'A', 'B', 1.0),
               ('test', 'pool-1', 110800, 'A', 'B', 1.0),
               ('test', 'pool-1', 114400, 'A', 'B', 1.0),
               ('test', 'pool-1', 118000, 'A', 'B', 1.0);",
        )
        .expect("seed test db");
        drop(db);

        let report = pool_volatility_report(&path, "test", "pool-1")
            .expect("report")
            .expect("pool");
        assert!(report["pair_vol"]["1d"].is_null());
        assert_eq!(
            report["status"],
            Value::String("insufficient_pair_history".into())
        );
        fs::remove_file(path).expect("remove test db");
    }

    #[test]
    fn complete_hourly_window_uses_1d_until_7d_exists() {
        let path = test_db();
        let db = Connection::open(&path).expect("open test db");
        db.execute_batch(
            "CREATE TABLE pools (
               chain TEXT, pool_address TEXT, pair TEXT, protocol TEXT,
               token_a_address TEXT, token_a_symbol TEXT,
               token_b_address TEXT, token_b_symbol TEXT
             );
             CREATE TABLE token_price_history (
               chain TEXT, address TEXT, timestamp INTEGER, price_usd REAL
             );
             CREATE TABLE pool_price_history (
               chain TEXT, pool_address TEXT, timestamp INTEGER,
               token0_address TEXT, token1_address TEXT,
               token1_per_token0 REAL
             );
             INSERT INTO pools VALUES
               ('test', 'pool-1', 'A/B', 'test', 'A', 'A', 'B', 'B');",
        )
        .expect("seed test db");
        for hour in 0..24_i64 {
            db.execute(
                "INSERT INTO pool_price_history VALUES ('test','pool-1',?1,'A','B',?2)",
                rusqlite::params![100_000 + hour * 3_600, 1.0 + hour as f64 / 100.0],
            )
            .expect("insert hourly point");
        }
        drop(db);
        let report = pool_volatility_report(&path, "test", "pool-1")
            .expect("report")
            .expect("pool");
        assert!(report["pair_vol"]["1d"]
            .as_f64()
            .is_some_and(|value| value > 0.0));
        assert!(report["pair_vol"]["7d"].is_null());
        assert_eq!(report["pair_vol"]["volatility_window_days"], 1);
        assert!(report["pair_vol"]["annualized"].as_f64().is_some());
        fs::remove_file(path).expect("remove test db");
    }

    #[test]
    fn stable_token_volatility_uses_observed_history() {
        let path = test_db();
        let db = Connection::open(&path).expect("open test db");
        db.execute_batch(
            "CREATE TABLE token_price_history (
               chain TEXT, address TEXT, timestamp INTEGER, price_usd REAL
             );
             CREATE TABLE pool_price_history (
               chain TEXT, pool_address TEXT, timestamp INTEGER,
               token0_address TEXT, token1_address TEXT,
               token1_per_token0 REAL
             );
             INSERT INTO token_price_history VALUES
               ('test', 'USDC', 100000, 1.0),
               ('test', 'USDC', 103600, 1.01),
               ('test', 'B', 100000, 1.0),
               ('test', 'B', 103600, 1.0);
             INSERT INTO pool_price_history VALUES
               ('test', 'pool-1', 103600, 'USDC', 'B', 1.0);",
        )
        .expect("seed test db");
        drop(db);

        let item = json!({
            "chain": "test",
            "pool_address": "pool-1",
            "token_a_address": "USDC",
            "token_b_address": "B",
            "token_a_symbol": "USDC",
            "token_b_symbol": "B"
        });
        let attached = attach_runtime_volatility(&path, vec![item])
            .expect("attach")
            .pop()
            .expect("item");
        assert!(attached["token_a_daily_vol"].is_null());
        fs::remove_file(path).expect("remove test db");
    }
}
