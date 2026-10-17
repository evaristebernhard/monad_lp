//! Backend option calculations.
//!
//! `rpc/option` only stores public quote observations. This binary reads those
//! observations and the LP snapshot, then calculates and persists hedge plans.

use std::env;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

const DEFAULT_DB: &str = "data/lp_screener.db";

fn arg(name: &str, default: &str) -> String {
    let args = env::args().collect::<Vec<_>>();
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .unwrap_or_else(|| default.to_string())
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
}

fn stable_symbol(symbol: &str) -> bool {
    matches!(
        symbol.trim().to_ascii_uppercase().as_str(),
        "USDC" | "USDT" | "USDS" | "DAI" | "BUSD" | "PYUSD" | "USD1" | "FDUSD"
    )
}

fn aliases(underlying: &str) -> [&'static str; 2] {
    match underlying {
        "BTC" => ["BTC", "WBTC"],
        "ETH" => ["ETH", "WETH"],
        "SOL" => ["SOL", "WSOL"],
        _ => ["", ""],
    }
}

#[derive(Debug)]
struct PoolCandidate {
    address: String,
    chain: String,
    asset_symbol: String,
    token_a_symbol: String,
    token_b_symbol: String,
    price_usd: f64,
    liquidity_usd: f64,
    fee_apr: f64,
}

fn load_pools(
    db: &Connection,
    underlying: &str,
    chain: Option<&str>,
    min_tvl: Option<f64>,
    limit: usize,
) -> Result<Vec<PoolCandidate>> {
    let mut statement = db.prepare(
        "SELECT pool_address, chain, protocol, pair, coalesce(asset_symbol,''), coalesce(token_a_symbol,''), coalesce(token_b_symbol,''), coalesce(price_usd,0), coalesce(liquidity_usd,0), coalesce(lp_fee_apr_estimate,0) FROM pools WHERE pair_category='stable_quote' AND coalesce(price_usd,0)>0 AND coalesce(liquidity_usd,0)>0 ORDER BY coalesce(liquidity_usd,0) DESC, coalesce(volume_24h_usd,0) DESC",
    )?;
    let aliases = aliases(underlying);
    let mut result = Vec::new();
    for row in statement.query_map([], |row| {
        Ok(PoolCandidate {
            address: row.get(0)?,
            chain: row.get(1)?,
            asset_symbol: row.get(4)?,
            token_a_symbol: row.get(5)?,
            token_b_symbol: row.get(6)?,
            price_usd: row.get(7)?,
            liquidity_usd: row.get(8)?,
            fee_apr: row.get(9)?,
        })
    })? {
        let pool = row?;
        let symbols = [
            pool.asset_symbol.as_str(),
            pool.token_a_symbol.as_str(),
            pool.token_b_symbol.as_str(),
        ];
        if !symbols.iter().any(|symbol| {
            aliases
                .iter()
                .any(|alias| !alias.is_empty() && symbol.eq_ignore_ascii_case(alias))
        }) || !symbols.iter().any(|symbol| stable_symbol(symbol))
        {
            continue;
        }
        if chain.is_some_and(|value| !pool.chain.eq_ignore_ascii_case(value))
            || min_tvl.is_some_and(|value| pool.liquidity_usd < value)
        {
            continue;
        }
        result.push(pool);
        if limit > 0 && result.len() >= limit {
            break;
        }
    }
    Ok(result)
}

fn finite_replication(
    p0: f64,
    wealth: f64,
    range_pct: f64,
    n_down: usize,
    n_up: usize,
) -> Result<Value> {
    let low = p0 * (1.0 - range_pct);
    let high = p0 * (1.0 + range_pct);
    if !(0.0 < low && low < p0 && p0 < high && wealth > 0.0 && n_down > 0 && n_up > 0) {
        anyhow::bail!("invalid hedge range or wealth")
    }
    let denominator = 2.0 * p0.sqrt() - p0 / high.sqrt() - low.sqrt();
    if denominator <= 0.0 {
        anyhow::bail!("invalid hedge range denominator")
    }
    let liquidity = wealth / denominator;
    let mut legs = Vec::new();
    let mut add_side = |start: f64, end: f64, segments: usize, option_type: &str| {
        let step = (end.powf(0.25) - start.powf(0.25)) / segments as f64;
        let error = liquidity * step.powi(2);
        for index in 1..=segments {
            let previous = (start.powf(0.25) + (index - 1) as f64 * step).powi(4);
            let current = (start.powf(0.25) + index as f64 * step).powi(4);
            legs.push(json!({
                "option_type": option_type,
                "segment_low": previous,
                "segment_high": current,
                "target_strike": (previous * current).sqrt(),
                "target_quantity_underlying": liquidity * (1.0 / previous.sqrt() - 1.0 / current.sqrt()),
            }));
        }
        error
    };
    let down_error = add_side(low, p0, n_down, "put");
    let up_error = add_side(p0, high, n_up, "call");
    Ok(json!({
        "initial_wealth_usd": wealth,
        "p0": p0,
        "range_low": low,
        "range_high": high,
        "n_down": n_down,
        "n_up": n_up,
        "liquidity_parameter": liquidity,
        "replication_error_usd": down_error.max(up_error),
        "hedge_legs": legs,
    }))
}

fn price_replication(
    db: &Connection,
    snapshot_id: i64,
    plan: &mut Value,
) -> Result<(Option<f64>, String)> {
    let mut statement = db.prepare(
        "SELECT option_type, strike, contract_size, best_ask_usd, instrument_name, mark_iv FROM option_quotes WHERE snapshot_id=?1 AND strike IS NOT NULL",
    )?;
    let quotes = statement
        .query_map(params![snapshot_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<f64>>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<f64>>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let Some(legs) = plan.get_mut("hedge_legs").and_then(Value::as_array_mut) else {
        anyhow::bail!("hedge plan has no legs")
    };
    let mut premium = 0.0;
    let mut complete = true;
    for leg in legs {
        let option_type = leg.get("option_type").and_then(Value::as_str).unwrap_or("");
        let target = number(leg.get("target_strike")).unwrap_or_default();
        let quote = quotes
            .iter()
            .filter(|quote| quote.0.as_deref() == Some(option_type) && quote.1.is_some())
            .min_by(|left, right| {
                let left_distance = (left.1.unwrap_or_default() / target).ln().abs();
                let right_distance = (right.1.unwrap_or_default() / target).ln().abs();
                left_distance
                    .partial_cmp(&right_distance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        let Some((_, Some(strike), Some(contract_size), Some(ask), instrument, mark_iv)) = quote
        else {
            leg["quote_status"] = json!("missing");
            leg["premium_usd"] = Value::Null;
            complete = false;
            continue;
        };
        if *contract_size <= 0.0 || !ask.is_finite() {
            leg["quote_status"] = json!("missing");
            leg["premium_usd"] = Value::Null;
            complete = false;
            continue;
        }
        let target_quantity = number(leg.get("target_quantity_underlying")).unwrap_or_default();
        let contracts = (target_quantity / contract_size - 1e-12).ceil();
        let quantity = contracts * contract_size;
        let cost = quantity * ask;
        leg["quote_status"] = json!("matched");
        leg["instrument_name"] = instrument.clone().map_or(Value::Null, |value| json!(value));
        leg["matched_strike"] = json!(strike);
        leg["contracts"] = json!(contracts);
        leg["contract_size_underlying"] = json!(contract_size);
        leg["hedge_quantity_underlying"] = json!(quantity);
        leg["premium_usd"] = json!(cost);
        leg["mark_iv"] = mark_iv.map_or(Value::Null, |value| json!(value));
        premium += cost;
    }
    plan["option_premium_usd"] = if complete {
        json!(premium)
    } else {
        Value::Null
    };
    plan["option_quote_status"] = json!(if complete { "complete" } else { "incomplete" });
    Ok((
        complete.then_some(premium),
        if complete { "complete" } else { "incomplete" }.into(),
    ))
}

fn latest_snapshot(db: &Connection, underlying: &str, tenor: &str) -> Result<Option<i64>> {
    db.query_row(
        "SELECT id FROM option_snapshots WHERE venue='deribit' AND underlying=?1 AND tenor=?2 AND status='ok' ORDER BY generated_at DESC, id DESC LIMIT 1",
        params![underlying, tenor],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn write_plans(
    db: &mut Connection,
    plans: &[(PoolCandidate, Value, Option<f64>, f64, f64, f64, String)],
) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS hedge_analysis (pool_address TEXT PRIMARY KEY, p0 REAL, range_low REAL, range_high REAL, period_fee_usd REAL, option_status TEXT, option_premium_usd REAL, replication_error_usd REAL, net_pnl_lower_usd REAL, net_pnl_upper_usd REAL, decision TEXT, hedge_json TEXT)")?;
    let transaction = db.transaction()?;
    for (pool, hedge, premium, period_fee, lower, upper, status) in plans {
        let decision = if *lower > 0.0 && premium.is_some() {
            "candidate"
        } else if premium.is_none() {
            "needs_quote"
        } else {
            "not_profitable"
        };
        transaction.execute(
            "INSERT OR REPLACE INTO hedge_analysis(pool_address,p0,range_low,range_high,period_fee_usd,option_status,option_premium_usd,replication_error_usd,net_pnl_lower_usd,net_pnl_upper_usd,decision,hedge_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                &pool.address,
                hedge["p0"].as_f64(),
                hedge["range_low"].as_f64(),
                hedge["range_high"].as_f64(),
                period_fee,
                status,
                premium,
                hedge["replication_error_usd"].as_f64(),
                if premium.is_some() { Some(*lower) } else { None::<f64> },
                if premium.is_some() { Some(*upper) } else { None::<f64> },
                decision,
                serde_json::to_string(hedge)?,
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn main() -> Result<()> {
    let db_path = arg("--db", DEFAULT_DB);
    let underlying = arg("--underlying", "SOL").to_ascii_uppercase();
    let tenor = arg("--tenor", "nearest");
    let chain = env::args()
        .position(|value| value == "--chain")
        .and_then(|index| env::args().nth(index + 1));
    let days = arg("--days", "7").parse::<f64>().context("parse --days")?;
    let min_tvl = arg("--min-tvl", "0")
        .parse::<f64>()
        .context("parse --min-tvl")?;
    let limit = arg("--limit", "100")
        .parse::<usize>()
        .context("parse --limit")?;
    let wealth = arg("--wealth", "10000")
        .parse::<f64>()
        .context("parse --wealth")?;
    let range_pct = arg("--range-pct", "0.20")
        .parse::<f64>()
        .context("parse --range-pct")?;
    let n_down = arg("--n-down", "2")
        .parse::<usize>()
        .context("parse --n-down")?;
    let n_up = arg("--n-up", "2")
        .parse::<usize>()
        .context("parse --n-up")?;
    let mut db = Connection::open(&db_path).with_context(|| format!("open {db_path}"))?;
    let snapshot_id = latest_snapshot(&db, &underlying, &tenor)?;
    let pools = load_pools(
        &db,
        &underlying,
        chain.as_deref(),
        (min_tvl > 0.0).then_some(min_tvl),
        limit,
    )?;
    let aliases = aliases(&underlying);
    let mut plans = Vec::new();
    if let Some(snapshot_id) = snapshot_id {
        for pool in pools {
            let token_a = pool.token_a_symbol.to_ascii_uppercase();
            let token_b = pool.token_b_symbol.to_ascii_uppercase();
            let mut p0 = pool.price_usd;
            if stable_symbol(&token_a) && aliases.iter().any(|alias| token_b == *alias) {
                p0 = 1.0 / p0;
            }
            let mut hedge = finite_replication(p0, wealth, range_pct, n_down, n_up)?;
            let (premium, status) = price_replication(&db, snapshot_id, &mut hedge)?;
            let period_fee = pool.fee_apr * wealth * days / 365.0;
            let error = hedge["replication_error_usd"].as_f64().unwrap_or_default();
            let lower = premium
                .map(|value| period_fee - value - error)
                .unwrap_or_default();
            let upper = premium.map(|value| period_fee - value).unwrap_or_default();
            plans.push((pool, hedge, premium, period_fee, lower, upper, status));
        }
    }
    if env::args().any(|value| value == "--write-db") {
        write_plans(&mut db, &plans)?;
    }
    let complete = plans.iter().filter(|plan| plan.6 == "complete").count();
    println!("{{\"underlying\":\"{underlying}\",\"tenor\":\"{tenor}\",\"pools\":{},\"complete\":{},\"write_db\":{}}}", plans.len(), complete, env::args().any(|value| value == "--write-db"));
    Ok(())
}
