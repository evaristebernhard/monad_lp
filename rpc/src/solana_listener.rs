//! Optional Solana WebSocket log listener.
//!
//! The HTTP RPC proxy and this listener intentionally share the same catalog,
//! but they have different transport behavior. JSON-RPC reads can fail over
//! per request; a WebSocket subscription is kept on one provider until it
//! closes, then the next catalog endpoint is tried.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    env,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use axum::{
    extract::{Query, State},
    Json,
};
use futures_util::{SinkExt, StreamExt};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    fs::{create_dir_all, File, OpenOptions},
    io::AsyncWriteExt,
    sync::Semaphore,
    task::JoinHandle,
    time::sleep,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::state::AppState;

#[path = "solana_decoders/mod.rs"]
mod solana_decoders;
#[path = "solana_sdk_decoder.rs"]
mod solana_sdk_decoder;

const SOLANA_ROUTE: &str = "solana-mainnet";
const DEFAULT_OUTPUT: &str = "rpc/data/solana_logs.jsonl";
const DEFAULT_EVENT_DB: &str = "rpc/data/solana_events.db";
const DEFAULT_WHALE_DB: &str = "rpc/data/solana_whales.db";
const DEFAULT_REALTIME_DB: &str = "rpc/data/solana_realtime.db";
const DEFAULT_COMMITMENT: &str = "confirmed";
const MAX_BACKOFF_SECS: u64 = 30;
const DEFAULT_POOLS_PER_PROTOCOL: usize = 2;
const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";
const DEFAULT_QUOTE_MINTS: [&str; 3] = [
    "So11111111111111111111111111111111111111112",
    "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",
];

fn protocol_descriptors() -> Value {
    let protocols = solana_decoders::registry::ALL
        .iter()
        .map(|descriptor| {
            json!({
                "protocol": descriptor.protocol,
                "pool_types": [descriptor.slug],
                "program_ids": [descriptor.program_id],
                "category": descriptor.category,
                "decoder": if matches!(descriptor.slug, "solana_dlmm" | "solana_whirlpool" | "solana_standard" | "solana_clmm") {
                    "sol-parser-sdk + native raw discriminator fallback"
                } else {
                    "registry instruction/log fallback"
                }
            })
        })
        .collect::<Vec<_>>();
    json!({
        "listener_model": "logsSubscribe immediate persist + bounded async getTransaction(jsonParsed/base64) enrichment + sol-parser-sdk + balance-delta attribution",
        "protocol_count": protocols.len(),
        "protocols": protocols
    })
}

pub(crate) async fn protocols() -> Json<Value> {
    Json(protocol_descriptors())
}

fn protocol_program_ids(pool_type: Option<&str>) -> Vec<&'static str> {
    match pool_type {
        Some("solana_standard") => solana_decoders::registry::ALL
            .iter()
            .filter(|descriptor| {
                descriptor.slug == "solana_standard" || descriptor.slug == "raydium_cpmm"
            })
            .map(|descriptor| descriptor.program_id)
            .collect(),
        Some(pool_type) => solana_decoders::registry::by_slug(pool_type)
            .map(|descriptor| vec![descriptor.program_id])
            .unwrap_or_else(solana_decoders::registry::all_program_ids),
        None => solana_decoders::registry::all_program_ids(),
    }
}

fn annotate_protocol_decoder(enrichment: &mut Value, watched_filter: Option<&WatchFilter>) {
    let Some(filter) = watched_filter else {
        return;
    };
    let expected = protocol_program_ids(filter.pool_type.as_deref());
    let transaction = enrichment
        .get_mut("transaction")
        .and_then(Value::as_object_mut);
    let Some(transaction) = transaction else {
        return;
    };
    let observed = transaction
        .get("program_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let matched_program = expected
        .iter()
        .find(|program| observed.iter().any(|item| item == *program))
        .copied();
    let logs = transaction
        .get("log_messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let raw_decoded = transaction
        .get("raw_protocol_events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|event| {
            event.get("pool_type").and_then(Value::as_str) == filter.pool_type.as_deref()
                && matched_program.is_some_and(|program| {
                    event.get("program_id").and_then(Value::as_str) == Some(program)
                })
        })
        .cloned();
    let sdk_decoded = transaction
        .get("sdk_protocol_events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|event| {
            event.get("pool_type").and_then(Value::as_str) == filter.pool_type.as_deref()
                && event
                    .get("pool_address")
                    .and_then(Value::as_str)
                    .is_some_and(|pool| {
                        filter
                            .address
                            .as_deref()
                            .is_some_and(|address| pool.eq_ignore_ascii_case(address))
                    })
        })
        .cloned();
    let exact_route = apply_exact_route_trade(transaction, filter);
    let exact_sdk_event = !exact_route
        && sdk_decoded
            .as_ref()
            .is_some_and(|decoded| apply_meteora_dlmm_event_trade(transaction, filter, decoded));
    suppress_unreliable_route_trade(transaction, filter, exact_route || exact_sdk_event);
    let decoded = solana_decoders::decode(filter.pool_type.as_deref(), &observed, &logs);
    let decoder_status = if sdk_decoded.is_some() {
        "sol_parser_sdk"
    } else if exact_route {
        "sol_parser_sdk_route"
    } else if raw_decoded.is_some() {
        "native_raw_decoder"
    } else if decoded.is_some() {
        "native_log_decoder_fallback"
    } else if matched_program.is_some() {
        "native_program_unresolved"
    } else {
        "no_watched_program_match"
    };
    transaction.insert("watched_protocol".to_string(), json!(filter.protocol));
    transaction.insert("watched_pool_type".to_string(), json!(filter.pool_type));
    transaction.insert("protocol_decoder".to_string(), json!(decoder_status));
    transaction.insert("matched_program_id".to_string(), json!(matched_program));
    if let Some(decoded) = sdk_decoded {
        transaction.insert(
            "event_kind".to_string(),
            decoded
                .get("event_kind")
                .cloned()
                .unwrap_or_else(|| json!("protocol_event")),
        );
        transaction.insert(
            "protocol_action".to_string(),
            json!(
                if decoded.get("event_kind").and_then(Value::as_str) == Some("swap") {
                    "swap"
                } else {
                    "liquidity"
                }
            ),
        );
        transaction.insert(
            "decoded_protocol".to_string(),
            decoded.get("protocol").cloned().unwrap_or(Value::Null),
        );
        transaction.insert(
            "decoded_pool_type".to_string(),
            decoded.get("pool_type").cloned().unwrap_or(Value::Null),
        );
        transaction.insert(
            "protocol_instruction".to_string(),
            decoded.get("instruction").cloned().unwrap_or(Value::Null),
        );
        transaction.insert(
            "protocol_explanation".to_string(),
            json!(format!(
                "sol-parser-sdk decoded {} with pool accounts, token mints and protocol amounts",
                decoded
                    .get("event")
                    .and_then(Value::as_str)
                    .unwrap_or("DEX event")
            )),
        );
        transaction.insert("protocol_program_id".to_string(), Value::Null);
        transaction.insert(
            "protocol_details".to_string(),
            decoded.get("details").cloned().unwrap_or(Value::Null),
        );
    } else if let Some(decoded) = raw_decoded {
        transaction.insert(
            "event_kind".to_string(),
            decoded
                .get("event_kind")
                .cloned()
                .unwrap_or_else(|| json!("protocol_event")),
        );
        transaction.insert(
            "protocol_action".to_string(),
            decoded.get("action").cloned().unwrap_or(Value::Null),
        );
        transaction.insert(
            "decoded_protocol".to_string(),
            decoded.get("protocol").cloned().unwrap_or(Value::Null),
        );
        transaction.insert(
            "decoded_pool_type".to_string(),
            decoded.get("pool_type").cloned().unwrap_or(Value::Null),
        );
        transaction.insert(
            "protocol_instruction".to_string(),
            decoded.get("instruction").cloned().unwrap_or(Value::Null),
        );
        transaction.insert(
            "protocol_explanation".to_string(),
            decoded.get("explanation").cloned().unwrap_or(Value::Null),
        );
        transaction.insert(
            "protocol_program_id".to_string(),
            decoded.get("program_id").cloned().unwrap_or(Value::Null),
        );
        transaction.insert("protocol_details".to_string(), Value::Null);
        if let Some(trade) = transaction.get_mut("trade").and_then(Value::as_object_mut) {
            if trade.get("side").and_then(Value::as_str) == Some("unknown") {
                if let Some(explanation) = decoded.get("explanation") {
                    trade.insert("reason".to_string(), explanation.clone());
                }
            }
        }
    } else if let Some(decoded) = decoded {
        transaction.insert("event_kind".to_string(), json!(decoded.event_kind));
        transaction.insert("protocol_action".to_string(), json!(decoded.action));
        transaction.insert("decoded_protocol".to_string(), json!(decoded.protocol));
        transaction.insert("decoded_pool_type".to_string(), json!(decoded.pool_type));
        transaction.insert(
            "protocol_instruction".to_string(),
            json!(decoded.instruction),
        );
        transaction.insert(
            "protocol_explanation".to_string(),
            json!(decoded.explanation),
        );
        transaction.insert("protocol_program_id".to_string(), json!(decoded.program_id));
        transaction.insert("protocol_details".to_string(), Value::Null);
        if let Some(trade) = transaction.get_mut("trade").and_then(Value::as_object_mut) {
            if trade.get("side").and_then(Value::as_str) == Some("unknown") {
                trade.insert("reason".to_string(), json!(decoded.explanation));
            }
        }
    } else if matched_program.is_some() {
        let failed = transaction.get("err").is_some_and(|error| !error.is_null());
        transaction.insert(
            "event_kind".to_string(),
            json!(if failed {
                "failed_program_event"
            } else {
                "protocol_event_unresolved"
            }),
        );
        if let Some(trade) = transaction.get_mut("trade").and_then(Value::as_object_mut) {
            if trade.get("side").and_then(Value::as_str) == Some("unknown") {
                trade.insert(
                    "reason".to_string(),
                    json!(if failed {
                        "known pool program failed before token balances settled"
                    } else {
                        "known pool program invoked, but its instruction was not decoded"
                    }),
                );
            }
        }
    } else {
        transaction.insert("event_kind".to_string(), json!("unrelated_router_event"));
        if let Some(trade) = transaction.get_mut("trade").and_then(Value::as_object_mut) {
            if trade.get("side").and_then(Value::as_str) == Some("unknown") {
                trade.insert(
                    "reason".to_string(),
                    json!("transaction mentioned the watched pool but did not invoke its pool program"),
                );
            }
        }
    }
}

fn apply_meteora_dlmm_event_trade(
    transaction: &mut serde_json::Map<String, Value>,
    _filter: &WatchFilter,
    decoded: &Value,
) -> bool {
    if decoded.get("event").and_then(Value::as_str) != Some("MeteoraDlmmSwap") {
        return false;
    }
    let Some(details) = decoded.get("details") else {
        return false;
    };
    // Do not treat swap instruction limits (max input / minimum output) as
    // executed amounts. The DLMM event parser marks its settled amount pair.
    if details.get("amounts_executed").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    let Some(trade) = transaction.get("trade").cloned() else {
        return false;
    };
    let Some(target_mint) = trade.get("target_mint").and_then(Value::as_str) else {
        return false;
    };
    let Some(token_x) = details.get("token_x_mint").and_then(Value::as_str) else {
        return false;
    };
    let Some(token_y) = details.get("token_y_mint").and_then(Value::as_str) else {
        return false;
    };
    let (input_mint, output_mint) =
        if details.get("swap_for_y").and_then(Value::as_bool) == Some(true) {
            (token_x, token_y)
        } else {
            (token_y, token_x)
        };
    let quote_mints = DEFAULT_QUOTE_MINTS
        .iter()
        .map(|mint| mint.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let target_lower = target_mint.to_ascii_lowercase();
    let (side, quote_mint, target_raw, quote_raw) = if output_mint
        .eq_ignore_ascii_case(&target_lower)
        && quote_mints.contains(&input_mint.to_ascii_lowercase())
    {
        (
            "buy",
            input_mint,
            details.get("amount_out").and_then(Value::as_u64),
            details.get("amount_in").and_then(Value::as_u64),
        )
    } else if input_mint.eq_ignore_ascii_case(&target_lower)
        && quote_mints.contains(&output_mint.to_ascii_lowercase())
    {
        (
            "sell",
            output_mint,
            details.get("amount_in").and_then(Value::as_u64),
            details.get("amount_out").and_then(Value::as_u64),
        )
    } else {
        return false;
    };
    let (Some(target_raw), Some(quote_raw)) = (target_raw, quote_raw) else {
        return false;
    };
    let target_decimals = mint_decimals(transaction, target_mint);
    let quote_decimals = mint_decimals(transaction, quote_mint);
    let target_amount = target_raw as f64 / 10_f64.powi(target_decimals as i32);
    let quote_amount = quote_raw as f64 / 10_f64.powi(quote_decimals as i32);
    if !target_amount.is_finite()
        || !quote_amount.is_finite()
        || target_amount <= 0.0
        || quote_amount <= 0.0
    {
        return false;
    }
    let quote_lower = quote_mint.to_ascii_lowercase();
    let unit_usd = if quote_lower == WSOL_MINT.to_ascii_lowercase() {
        trade.get("sol_price_usd").and_then(Value::as_f64)
    } else if quote_lower == DEFAULT_QUOTE_MINTS[1].to_ascii_lowercase()
        || quote_lower == DEFAULT_QUOTE_MINTS[2].to_ascii_lowercase()
    {
        Some(1.0)
    } else {
        None
    };
    let notional_usd = unit_usd
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|unit_usd| quote_amount * unit_usd);
    let whale_threshold = trade
        .get("whale_threshold_usd")
        .and_then(Value::as_f64)
        .unwrap_or(f64::INFINITY);
    let is_whale = notional_usd.is_some_and(|amount| amount >= whale_threshold);
    let mut exact_trade = trade;
    if let Some(object) = exact_trade.as_object_mut() {
        object.insert("side".to_string(), json!(side));
        object.insert(
            "action".to_string(),
            json!(if side == "buy" { "bought" } else { "sold" }),
        );
        if let Some(trader) = details.get("from").and_then(Value::as_str) {
            object.insert("trader".to_string(), json!(trader));
        }
        object.insert("target_amount_ui".to_string(), json!(target_amount));
        object.insert("quote_mint".to_string(), json!(quote_mint));
        object.insert("quote_amount_ui".to_string(), json!(quote_amount));
        object.insert(
            "target_delta_ui_amount".to_string(),
            json!(if side == "buy" {
                target_amount
            } else {
                -target_amount
            }),
        );
        object.insert(
            "quote_delta_ui_amount".to_string(),
            json!(if side == "buy" {
                -quote_amount
            } else {
                quote_amount
            }),
        );
        object.insert(
            "swap_ratio_quote_per_target".to_string(),
            json!(quote_amount / target_amount),
        );
        object.insert(
            "swap_price_usd".to_string(),
            notional_usd.map_or(Value::Null, |amount| json!(amount / target_amount)),
        );
        object.insert(
            "notional_usd".to_string(),
            notional_usd.map_or(Value::Null, |amount| json!(amount)),
        );
        object.insert("is_whale".to_string(), json!(is_whale));
        object.insert(
            "alert_type".to_string(),
            if is_whale {
                json!(if side == "buy" {
                    "whale_buy"
                } else {
                    "whale_sell"
                })
            } else {
                Value::Null
            },
        );
        object.insert("attribution".to_string(), json!("sdk_meteora_dlmm_event"));
        object.insert(
            "amount_source".to_string(),
            json!("meteora_dlmm_swap_event"),
        );
    }
    transaction.insert("trade".to_string(), exact_trade);
    true
}

fn apply_exact_route_trade(
    transaction: &mut serde_json::Map<String, Value>,
    filter: &WatchFilter,
) -> bool {
    let Some(pool) = filter.address.as_deref() else {
        return false;
    };
    let Some(routes) = transaction.get("sdk_route_swaps").and_then(Value::as_array) else {
        return false;
    };
    let Some(trade) = transaction.get("trade").cloned() else {
        return false;
    };
    let Some(target_mint) = trade.get("target_mint").and_then(Value::as_str) else {
        return false;
    };
    let quote_mints = DEFAULT_QUOTE_MINTS
        .iter()
        .map(|mint| mint.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let Some(route) = routes.iter().find(|route| {
        route
            .get("pool")
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case(pool))
            && route
                .get("actual_input_amount")
                .and_then(Value::as_u64)
                .is_some_and(|amount| amount > 0)
            && route
                .get("actual_output_amount")
                .and_then(Value::as_u64)
                .is_some_and(|amount| amount > 0)
    }) else {
        return false;
    };
    let Some(input_mint) = route.get("input_mint").and_then(Value::as_str) else {
        return false;
    };
    let Some(output_mint) = route.get("output_mint").and_then(Value::as_str) else {
        return false;
    };
    let target_lower = target_mint.to_ascii_lowercase();
    let (side, quote_mint, target_raw, quote_raw) = if output_mint
        .eq_ignore_ascii_case(&target_lower)
        && quote_mints.contains(&input_mint.to_ascii_lowercase())
    {
        (
            "buy",
            input_mint,
            route.get("actual_output_amount").and_then(Value::as_u64),
            route.get("actual_input_amount").and_then(Value::as_u64),
        )
    } else if input_mint.eq_ignore_ascii_case(&target_lower)
        && quote_mints.contains(&output_mint.to_ascii_lowercase())
    {
        (
            "sell",
            output_mint,
            route.get("actual_input_amount").and_then(Value::as_u64),
            route.get("actual_output_amount").and_then(Value::as_u64),
        )
    } else {
        return false;
    };
    let (Some(target_raw), Some(quote_raw)) = (target_raw, quote_raw) else {
        return false;
    };
    let target_decimals = mint_decimals(transaction, target_mint);
    let quote_decimals = mint_decimals(transaction, quote_mint);
    let target_amount = target_raw as f64 / 10_f64.powi(target_decimals as i32);
    let quote_amount = quote_raw as f64 / 10_f64.powi(quote_decimals as i32);
    if !target_amount.is_finite()
        || !quote_amount.is_finite()
        || target_amount <= 0.0
        || quote_amount <= 0.0
    {
        return false;
    }
    let quote_lower = quote_mint.to_ascii_lowercase();
    let unit_usd = if quote_lower == WSOL_MINT.to_ascii_lowercase() {
        trade.get("sol_price_usd").and_then(Value::as_f64)
    } else if quote_lower == DEFAULT_QUOTE_MINTS[1].to_ascii_lowercase()
        || quote_lower == DEFAULT_QUOTE_MINTS[2].to_ascii_lowercase()
    {
        Some(1.0)
    } else {
        None
    };
    let notional_usd = unit_usd
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|unit_usd| quote_amount * unit_usd);
    let whale_threshold = trade
        .get("whale_threshold_usd")
        .and_then(Value::as_f64)
        .unwrap_or(f64::INFINITY);
    let is_whale = notional_usd.is_some_and(|amount| amount >= whale_threshold);
    let mut exact_trade = trade;
    if let Some(object) = exact_trade.as_object_mut() {
        object.insert("side".to_string(), json!(side));
        object.insert(
            "action".to_string(),
            json!(if side == "buy" { "bought" } else { "sold" }),
        );
        if let Some(trader) = route.get("trader").and_then(Value::as_str) {
            object.insert("trader".to_string(), json!(trader));
        }
        object.insert("target_amount_ui".to_string(), json!(target_amount));
        object.insert("quote_mint".to_string(), json!(quote_mint));
        object.insert("quote_amount_ui".to_string(), json!(quote_amount));
        object.insert(
            "target_delta_ui_amount".to_string(),
            json!(if side == "buy" {
                target_amount
            } else {
                -target_amount
            }),
        );
        object.insert(
            "quote_delta_ui_amount".to_string(),
            json!(if side == "buy" {
                -quote_amount
            } else {
                quote_amount
            }),
        );
        object.insert(
            "swap_ratio_quote_per_target".to_string(),
            json!(quote_amount / target_amount),
        );
        object.insert(
            "swap_price_usd".to_string(),
            notional_usd.map_or(Value::Null, |amount| json!(amount / target_amount)),
        );
        object.insert(
            "notional_usd".to_string(),
            notional_usd.map_or(Value::Null, |amount| json!(amount)),
        );
        object.insert("is_whale".to_string(), json!(is_whale));
        object.insert(
            "alert_type".to_string(),
            if is_whale {
                json!(if side == "buy" {
                    "whale_buy"
                } else {
                    "whale_sell"
                })
            } else {
                Value::Null
            },
        );
        object.insert("attribution".to_string(), json!("sdk_route_pool_exact"));
        object.insert(
            "route_protocol".to_string(),
            route.get("protocol").cloned().unwrap_or(Value::Null),
        );
    }
    transaction.insert("trade".to_string(), exact_trade);
    true
}

fn suppress_unreliable_route_trade(
    transaction: &mut serde_json::Map<String, Value>,
    filter: &WatchFilter,
    exact_route: bool,
) {
    if exact_route {
        return;
    }
    let Some(routes) = transaction
        .get("sdk_route_swaps")
        .and_then(Value::as_array)
        .filter(|routes| !routes.is_empty())
    else {
        return;
    };
    let has_route_for_pool = routes.iter().any(|route| {
        route
            .get("pool")
            .and_then(Value::as_str)
            .zip(filter.address.as_deref())
            .is_some_and(|(route_pool, watched_pool)| route_pool.eq_ignore_ascii_case(watched_pool))
    });
    if let Some(trade) = transaction.get_mut("trade").and_then(Value::as_object_mut) {
        trade.insert("side".to_string(), json!("unknown"));
        trade.insert("is_whale".to_string(), json!(false));
        trade.insert("alert_type".to_string(), Value::Null);
        trade.insert("notional_usd".to_string(), Value::Null);
        trade.insert("swap_price_usd".to_string(), Value::Null);
        trade.insert("quote_amount_ui".to_string(), Value::Null);
        trade.insert("target_amount_ui".to_string(), Value::Null);
        trade.insert("swap_ratio_quote_per_target".to_string(), Value::Null);
        trade.insert(
            "reason".to_string(),
            json!(if has_route_for_pool {
                "router swap had no reliable executed amount for this watched pool"
            } else {
                "transaction route executed through a different pool"
            }),
        );
    }
}

fn mint_decimals(transaction: &serde_json::Map<String, Value>, mint: &str) -> u64 {
    transaction
        .get("token_changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|change| {
            change
                .get("mint")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case(mint))
        })
        .and_then(|change| change.get("decimals"))
        .and_then(Value::as_u64)
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
struct WatchFilter {
    address: Option<String>,
    protocol: Option<String>,
    pool_type: Option<String>,
    token_a_symbol: Option<String>,
    token_b_symbol: Option<String>,
}

impl WatchFilter {
    fn subscription_filter(&self) -> Value {
        self.address
            .as_ref()
            .map(|address| json!({"mentions": [address]}))
            .unwrap_or_else(|| Value::String("all".to_string()))
    }
}

#[derive(Clone, Debug)]
struct Config {
    ws_urls: Vec<String>,
    http_urls: Vec<String>,
    filters: Vec<WatchFilter>,
    target_token: Option<String>,
    quote_mints: HashSet<String>,
    whale_usd: f64,
    sol_price_usd: Option<f64>,
    commitment: String,
    enrich: bool,
    output: PathBuf,
    event_db: PathBuf,
    whale_db: PathBuf,
    realtime_db: PathBuf,
    pool_refresh_secs: u64,
}

impl Config {
    fn from_env(state: &AppState) -> Result<Option<Self>> {
        if !env_bool("SOLANA_LISTENER_ENABLED") {
            return Ok(None);
        }
        Self::build(state, None).map(Some)
    }

    fn from_token(state: &AppState, token: &str) -> Result<Self> {
        let token = token.trim();
        if token.is_empty() {
            bail!("listener start requires a token mint");
        }
        Self::build(state, Some(token))
    }

    fn build(state: &AppState, token_override: Option<&str>) -> Result<Self> {
        let route = state.routes.get(SOLANA_ROUTE).ok_or_else(|| {
            anyhow!("Solana listener enabled but route {SOLANA_ROUTE} is missing")
        })?;

        let http_urls = env::var("SOLANA_LISTENER_HTTP_URL")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                route
                    .endpoints
                    .iter()
                    .map(|endpoint| endpoint.url.clone())
                    .collect()
            });

        if http_urls.is_empty() {
            bail!("no usable Solana HTTP RPC endpoints found in the RPC catalog");
        }

        let ws_urls = env::var("SOLANA_LISTENER_WS_URL")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(to_ws_url)
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_else(|| {
                http_urls
                    .iter()
                    .filter_map(|endpoint| to_ws_url(endpoint).ok())
                    .collect()
            });

        if ws_urls.is_empty() {
            bail!("no usable Solana WebSocket endpoints found in the RPC catalog");
        }

        let pool_db_path = pool_catalog_path();
        let mut filters = Vec::new();
        for address in csv_values("SOLANA_LISTENER_POOL_ADDRESSES") {
            filters.push(WatchFilter {
                address: Some(address),
                protocol: None,
                pool_type: None,
                token_a_symbol: None,
                token_b_symbol: None,
            });
        }
        let target_token = token_override.map(ToOwned::to_owned).or_else(|| {
            env::var("SOLANA_LISTENER_TOKEN")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        });
        if let Some(token) = target_token.as_deref() {
            filters.extend(discover_token_pools(&token)?);
        }
        if filters.is_empty() {
            if let Some(address) = env::var("SOLANA_LISTENER_MENTION")
                .or_else(|_| env::var("SOLANA_LISTENER_ADDRESS"))
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
            {
                filters.push(WatchFilter {
                    address: Some(address),
                    protocol: None,
                    pool_type: None,
                    token_a_symbol: None,
                    token_b_symbol: None,
                });
            } else if env_bool("SOLANA_LISTENER_ALL") {
                filters.push(WatchFilter {
                    address: None,
                    protocol: None,
                    pool_type: None,
                    token_a_symbol: None,
                    token_b_symbol: None,
                });
            }
        }
        deduplicate_filters(&mut filters);
        if filters.is_empty() {
            bail!(
                "listener requires a token, pool address, mention address, or SOLANA_LISTENER_ALL=true"
            );
        }

        let sol_price_usd = env::var("SOLANA_LISTENER_SOL_PRICE_USD")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|value: &f64| value.is_finite() && *value > 0.0)
            .or_else(|| discover_sol_price_usd(&pool_db_path));

        Ok(Self {
            ws_urls,
            http_urls,
            filters,
            target_token,
            quote_mints: quote_mints(),
            whale_usd: env::var("SOLANA_LISTENER_WHALE_USD")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(10_000.0),
            sol_price_usd,
            commitment: env::var("SOLANA_LISTENER_COMMITMENT")
                .unwrap_or_else(|_| DEFAULT_COMMITMENT.to_string()),
            enrich: env::var("SOLANA_LISTENER_ENRICH")
                .map(|value| !matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "no"))
                .unwrap_or(true),
            output: env::var_os("SOLANA_LISTENER_OUTPUT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT)),
            event_db: env::var_os("SOLANA_LISTENER_EVENT_DB")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_EVENT_DB)),
            whale_db: env::var_os("SOLANA_LISTENER_WHALE_DB")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WHALE_DB)),
            realtime_db: env::var_os("SOLANA_LISTENER_REALTIME_DB")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_REALTIME_DB)),
            pool_refresh_secs: env::var("SOLANA_LISTENER_POOL_REFRESH_SECS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(15)
                .max(1),
        })
    }
}

fn csv_values(name: &str) -> Vec<String> {
    env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn quote_mints() -> HashSet<String> {
    let configured = csv_values("SOLANA_LISTENER_QUOTE_MINTS");
    let values = if configured.is_empty() {
        DEFAULT_QUOTE_MINTS
            .iter()
            .map(|value| (*value).to_string())
            .collect()
    } else {
        configured
    };
    values
        .into_iter()
        .map(|value| value.to_ascii_lowercase())
        .collect()
}

fn discover_token_pools(token: &str) -> Result<Vec<WatchFilter>> {
    let path = pool_catalog_path();
    let connection = Connection::open(&path)
        .with_context(|| format!("open Solana pool catalog {}", path.display()))?;
    let limit = env::var("SOLANA_LISTENER_POOLS_PER_PROTOCOL")
        .or_else(|_| env::var("SOLANA_LISTENER_POOL_LIMIT"))
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_POOLS_PER_PROTOCOL);
    let mut pools = {
        let mut statement = connection.prepare(
            "WITH ranked AS (
           SELECT pool_address, protocol, pool_type, token_a_symbol, token_b_symbol,
                  ROW_NUMBER() OVER (
                    PARTITION BY protocol
                    ORDER BY COALESCE(liquidity_usd, 0) DESC,
                             COALESCE(volume_24h_usd, 0) DESC,
                             pool_address
                  ) AS pool_rank
           FROM pools
           WHERE lower(chain) = 'solana'
             AND (lower(token_a_address) = lower(?1)
                  OR lower(token_b_address) = lower(?1))
         )
         SELECT pool_address, protocol, pool_type, token_a_symbol, token_b_symbol
         FROM ranked
         WHERE ?2 = 0 OR pool_rank <= ?2
         ORDER BY protocol, pool_rank",
        )?;
        let pools = statement
            .query_map(params![token, limit as i64], |row| {
                Ok(WatchFilter {
                    address: Some(row.get(0)?),
                    protocol: row.get(1).ok(),
                    pool_type: row.get(2).ok(),
                    token_a_symbol: row.get(3).ok(),
                    token_b_symbol: row.get(4).ok(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        pools
    };
    if pools.is_empty() {
        let mut statement = connection.prepare(
            "WITH ranked AS (
               SELECT pool_address, protocol, pool_type, token_a_symbol, token_b_symbol,
                      ROW_NUMBER() OVER (
                        PARTITION BY protocol
                        ORDER BY COALESCE(liquidity_usd, 0) DESC,
                                 COALESCE(volume_24h_usd, 0) DESC,
                                 pool_address
                      ) AS pool_rank
               FROM pools
               WHERE lower(chain) = 'solana'
                 AND (lower(token_a_symbol) = lower(?1)
                      OR lower(token_b_symbol) = lower(?1))
             )
             SELECT pool_address, protocol, pool_type, token_a_symbol, token_b_symbol
             FROM ranked
             WHERE ?2 = 0 OR pool_rank <= ?2
             ORDER BY protocol, pool_rank",
        )?;
        pools = statement
            .query_map(params![token, limit as i64], |row| {
                Ok(WatchFilter {
                    address: Some(row.get(0)?),
                    protocol: row.get(1).ok(),
                    pool_type: row.get(2).ok(),
                    token_a_symbol: row.get(3).ok(),
                    token_b_symbol: row.get(4).ok(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
    }
    if pools.is_empty() {
        bail!(
            "no Solana pools found for token {token} in {}",
            path.display()
        );
    }
    Ok(pools)
}

fn pool_catalog_path() -> PathBuf {
    env::var_os("SOLANA_LISTENER_POOL_DB")
        .or_else(|| env::var_os("LP_SCREENER_DB"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data/lp_screener.db"))
}

fn discover_sol_price_usd(path: &PathBuf) -> Option<f64> {
    let connection = Connection::open(path).ok()?;
    connection
        .query_row(
            "SELECT price_usd
             FROM pools
             WHERE lower(chain) = 'solana'
               AND lower(token_a_address) = lower(?1)
               AND lower(token_b_address) IN (lower(?2), lower(?3))
               AND price_usd IS NOT NULL AND price_usd > 0
             ORDER BY COALESCE(liquidity_usd, 0) DESC,
                      COALESCE(volume_24h_usd, 0) DESC
             LIMIT 1",
            params![WSOL_MINT, DEFAULT_QUOTE_MINTS[1], DEFAULT_QUOTE_MINTS[2]],
            |row| row.get::<_, f64>(0),
        )
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
}

fn deduplicate_filters(filters: &mut Vec<WatchFilter>) {
    let mut seen = HashSet::new();
    filters.retain(|filter| {
        let key = filter
            .address
            .as_deref()
            .unwrap_or("all")
            .to_ascii_lowercase();
        seen.insert(key)
    });
}

#[derive(Clone, Debug, Serialize)]
struct ActiveListener {
    token: Option<String>,
    filters: usize,
    protocols: Vec<String>,
    pool_types: Vec<String>,
    output: String,
    event_db: String,
    whale_db: String,
    realtime_db: String,
    pool_refresh_secs: u64,
    started_at_ms: u128,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StartRequest {
    token: String,
}

static LISTENER_TASK: OnceLock<Mutex<Option<JoinHandle<()>>>> = OnceLock::new();
static LISTENER_STATUS: OnceLock<Mutex<Option<ActiveListener>>> = OnceLock::new();

fn listener_task() -> &'static Mutex<Option<JoinHandle<()>>> {
    LISTENER_TASK.get_or_init(|| Mutex::new(None))
}

fn listener_status() -> &'static Mutex<Option<ActiveListener>> {
    LISTENER_STATUS.get_or_init(|| Mutex::new(None))
}

fn start_config(config: Config, state: AppState) -> Result<(ActiveListener, bool)> {
    let mut protocols = config
        .filters
        .iter()
        .filter_map(|filter| filter.protocol.clone())
        .collect::<Vec<_>>();
    protocols.sort();
    protocols.dedup();
    let mut pool_types = config
        .filters
        .iter()
        .filter_map(|filter| filter.pool_type.clone())
        .collect::<Vec<_>>();
    pool_types.sort();
    pool_types.dedup();
    let status = ActiveListener {
        token: config.target_token.clone(),
        filters: config.filters.len(),
        protocols,
        pool_types,
        output: config.output.display().to_string(),
        event_db: config.event_db.display().to_string(),
        whale_db: config.whale_db.display().to_string(),
        realtime_db: config.realtime_db.display().to_string(),
        pool_refresh_secs: config.pool_refresh_secs,
        started_at_ms: now_ms(),
    };
    {
        let mut task = listener_task()
            .lock()
            .map_err(|_| anyhow!("listener task lock poisoned"))?;
        let current = listener_status()
            .lock()
            .map_err(|_| anyhow!("listener status lock poisoned"))?;
        if let (Some(current_status), Some(current_task)) = (current.as_ref(), task.as_ref()) {
            if current_status.token == status.token && !current_task.is_finished() {
                return Ok((current_status.clone(), true));
            }
        }
        if let Some(previous) = task.take() {
            previous.abort();
        }
        *task = Some(tokio::spawn(async move {
            if let Err(error) = run(config, state).await {
                eprintln!("Solana listener stopped: {error:#}");
            }
        }));
    }
    *listener_status()
        .lock()
        .map_err(|_| anyhow!("listener status lock poisoned"))? = Some(status.clone());
    Ok((status, false))
}

/// Start the listener from environment configuration when explicitly enabled.
pub(crate) fn spawn(state: AppState) -> Result<Option<JoinHandle<()>>> {
    let Some(config) = Config::from_env(&state)? else {
        return Ok(None);
    };

    println!(
        "Solana listener enabled; filters={}; ws_endpoints={}; enrich={}; output={}",
        config.filters.len(),
        config.ws_urls.len(),
        config.enrich,
        config.output.display()
    );
    start_config(config, state)?;
    Ok(None)
}

pub(crate) async fn start(
    State(state): State<AppState>,
    Json(request): Json<StartRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let config = Config::from_token(&state, &request.token).map_err(listener_error)?;
    let (status, reused) = start_config(config, state).map_err(listener_error)?;
    Ok(Json(json!({
        "success": true,
        "reused": reused,
        "listener": status,
    })))
}

pub(crate) async fn status() -> Json<Value> {
    let task_active = listener_task()
        .lock()
        .ok()
        .and_then(|value| value.as_ref().map(|task| !task.is_finished()))
        .unwrap_or(false);
    let active = listener_status()
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .filter(|_| task_active);
    Json(json!({
        "success": true,
        "active": active.is_some(),
        "listener": active,
    }))
}

fn listener_error(error: anyhow::Error) -> (axum::http::StatusCode, Json<Value>) {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(json!({"success": false, "error": error.to_string()})),
    )
}

async fn run(config: Config, state: AppState) -> Result<()> {
    let http = state.http.clone();
    if let Some(parent) = config
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        create_dir_all(parent)
            .await
            .with_context(|| format!("create listener output directory {}", parent.display()))?;
    }
    let mut output = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config.output)
        .await
        .with_context(|| format!("open listener output {}", config.output.display()))?;
    if let Some(parent) = config
        .event_db
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        create_dir_all(parent).await.with_context(|| {
            format!(
                "create listener event database directory {}",
                parent.display()
            )
        })?;
    }
    let event_db = initialize_event_db(&config.event_db)?;
    let whale_db = initialize_whale_db(&config.whale_db)?;
    migrate_legacy_whale_alerts(&event_db, &config.event_db, &whale_db)?;
    let restored_whale_alerts = restore_whale_alerts_from_log(&whale_db, &config.output)?;
    if restored_whale_alerts > 0 {
        eprintln!(
            "restored {restored_whale_alerts} historical Solana whale alerts from the event log"
        );
    }
    let removed_unmatched_routes =
        cleanup_unmatched_route_trades_from_log(&event_db, &whale_db, &config.output)?;
    if removed_unmatched_routes > 0 {
        eprintln!(
            "removed {removed_unmatched_routes} trades attributed to a different routed pool"
        );
    }
    if let Some(target_token) = config.target_token.as_deref() {
        let excluded_owners = config
            .filters
            .iter()
            .filter_map(|filter| filter.address.as_deref())
            .map(str::to_ascii_lowercase)
            .collect::<HashSet<_>>();
        let pool_metadata = config
            .filters
            .iter()
            .filter_map(|filter| {
                filter.address.as_ref().map(|address| {
                    (
                        address.to_ascii_lowercase(),
                        (filter.protocol.clone(), filter.pool_type.clone()),
                    )
                })
            })
            .collect::<HashMap<_, _>>();
        let backfilled = backfill_event_trades(
            &event_db,
            target_token,
            &config.quote_mints,
            config.whale_usd,
            config.sol_price_usd,
            &excluded_owners,
            &pool_metadata,
        )?;
        if backfilled > 0 {
            eprintln!("backfilled {backfilled} classified Solana pool trades for {target_token}");
        }
    }
    prune_recent_events(&event_db)?;
    let event_db = Arc::new(Mutex::new(event_db));
    if let Some(parent) = config
        .realtime_db
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        create_dir_all(parent).await.with_context(|| {
            format!(
                "create listener realtime database directory {}",
                parent.display()
            )
        })?;
    }
    let realtime_db = initialize_realtime_db(&config.realtime_db)?;
    let refresh = PoolRefreshCoordinator::new(realtime_db, config.pool_refresh_secs);
    let decode_slots = Arc::new(Semaphore::new(16));

    let mut endpoint_index = 0usize;
    let mut backoff_secs = 1u64;
    let mut seen = SeenSignatures::default();
    loop {
        let endpoint = &config.ws_urls[endpoint_index % config.ws_urls.len()];
        let http_endpoint = &config.http_urls[endpoint_index % config.http_urls.len()];
        match listen_once(
            endpoint,
            http_endpoint,
            &config,
            &http,
            &mut output,
            &event_db,
            &refresh,
            &mut seen,
            &state,
            &decode_slots,
        )
        .await
        {
            Ok(()) => {
                eprintln!(
                    "Solana listener connection closed: {}",
                    display_url(endpoint)
                );
                backoff_secs = 1;
            }
            Err(error) => {
                eprintln!(
                    "Solana listener error on {}: {error:#}",
                    display_url(endpoint)
                );
                backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
            }
        }
        endpoint_index = (endpoint_index + 1) % config.ws_urls.len();
        sleep(Duration::from_secs(backoff_secs)).await;
    }
}

async fn listen_once(
    endpoint: &str,
    http_endpoint: &str,
    config: &Config,
    http: &reqwest::Client,
    output: &mut File,
    event_db: &Arc<Mutex<Connection>>,
    refresh: &PoolRefreshCoordinator,
    seen: &mut SeenSignatures,
    state: &AppState,
    decode_slots: &Arc<Semaphore>,
) -> Result<()> {
    let (mut socket, _) = connect_async(endpoint)
        .await
        .with_context(|| format!("connect {}", display_url(endpoint)))?;
    let mut pending = HashMap::new();
    for (index, filter) in config.filters.iter().enumerate() {
        let request_id = (index + 1) as i64;
        let subscribe = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "logsSubscribe",
            "params": [filter.subscription_filter(), {"commitment": config.commitment}],
        });
        socket
            .send(Message::Text(subscribe.to_string().into()))
            .await
            .context("send logsSubscribe")?;
        pending.insert(request_id, filter.clone());
    }

    let mut active = HashMap::new();
    while let Some(message) = socket.next().await {
        match message.context("read Solana WebSocket message")? {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(&text).context("decode Solana event")?;
                if let Some(request_id) = value.get("id").and_then(Value::as_i64) {
                    let filter = pending.remove(&request_id);
                    if let Some(error) = value.get("error") {
                        eprintln!(
                            "Solana logsSubscribe rejected on {} for {:?}: {error}",
                            display_url(endpoint),
                            filter
                                .as_ref()
                                .and_then(|item| item.address.as_deref())
                                .unwrap_or("all")
                        );
                        if pending.is_empty() && active.is_empty() {
                            bail!(
                                "Solana WebSocket endpoint {} rejected every pool subscription",
                                display_url(endpoint)
                            );
                        }
                        continue;
                    }
                    if let (Some(subscription), Some(filter)) =
                        (value.get("result").and_then(Value::as_u64), filter)
                    {
                        active.insert(subscription, filter);
                    }
                    if pending.is_empty() && active.is_empty() {
                        bail!(
                            "Solana WebSocket endpoint {} rejected every pool subscription",
                            display_url(endpoint)
                        );
                    }
                    eprintln!(
                        "Solana logsSubscribe acknowledged on {} ({}/{})",
                        display_url(endpoint),
                        active.len(),
                        config.filters.len()
                    );
                } else if value.get("method").and_then(Value::as_str) == Some("logsNotification") {
                    let subscription = value
                        .pointer("/params/subscription")
                        .and_then(Value::as_u64);
                    let filter = subscription.and_then(|id| active.get(&id));
                    append_notification(
                        output,
                        endpoint,
                        http_endpoint,
                        config,
                        http,
                        event_db,
                        refresh,
                        seen,
                        state,
                        decode_slots,
                        filter,
                        &value,
                    )
                    .await?;
                }
            }
            Message::Binary(bytes) => {
                let value: Value =
                    serde_json::from_slice(&bytes).context("decode binary Solana event")?;
                if value.get("method").and_then(Value::as_str) == Some("logsNotification") {
                    let subscription = value
                        .pointer("/params/subscription")
                        .and_then(Value::as_u64);
                    let filter = subscription.and_then(|id| active.get(&id));
                    append_notification(
                        output,
                        endpoint,
                        http_endpoint,
                        config,
                        http,
                        event_db,
                        refresh,
                        seen,
                        state,
                        decode_slots,
                        filter,
                        &value,
                    )
                    .await?;
                }
            }
            Message::Ping(payload) => {
                socket
                    .send(Message::Pong(payload))
                    .await
                    .context("reply Solana ping")?;
            }
            Message::Pong(_) => {}
            Message::Close(frame) => {
                if active.is_empty() {
                    bail!(
                        "Solana WebSocket closed before any subscription acknowledgement: {frame:?}"
                    );
                }
                return Ok(());
            }
            Message::Frame(_) => {}
        }
    }

    if !active.is_empty() {
        Ok(())
    } else {
        bail!("Solana WebSocket ended before subscription acknowledgement")
    }
}

async fn append_notification(
    output: &mut File,
    endpoint: &str,
    http_endpoint: &str,
    config: &Config,
    http: &reqwest::Client,
    event_db: &Arc<Mutex<Connection>>,
    refresh: &PoolRefreshCoordinator,
    seen: &mut SeenSignatures,
    state: &AppState,
    decode_slots: &Arc<Semaphore>,
    watched_filter: Option<&WatchFilter>,
    event: &Value,
) -> Result<()> {
    let signature = notification_signature(event);
    let Some(signature) = signature else {
        append_event(
            output,
            endpoint,
            event,
            watched_filter,
            event_db,
            json!({"available": false, "error": "notification has no transaction signature"}),
        )
        .await?;
        refresh.schedule(
            watched_filter,
            None,
            None,
            http_endpoint,
            config,
            http,
            state,
        );
        return Ok(());
    };
    let scope = watched_filter
        .and_then(|filter| filter.address.as_deref())
        .unwrap_or("all");
    if !seen.insert(&signature, scope) {
        return Ok(());
    }

    let pending_enrichment = if config.enrich {
        let logs = event
            .pointer("/params/result/value/logs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let instruction = logs
            .iter()
            .filter_map(Value::as_str)
            .find_map(|line| line.split_once("Instruction:").map(|(_, name)| name.trim()));
        let is_swap = logs
            .iter()
            .filter_map(Value::as_str)
            .any(|line| line.to_ascii_lowercase().contains("swap"));
        json!({
            "available": true,
            "pending": true,
            "transaction": {
                "event_kind": if is_swap { "swap" } else { "protocol_event_unresolved" },
                "protocol_instruction": instruction,
                "protocol_explanation": "Decoding transaction…",
                "trade": {
                    "side": "unknown",
                    "target_mint": config.target_token.as_deref(),
                    "is_whale": false,
                }
            }
        })
    } else {
        json!({"available": false, "skipped": true})
    };
    let record = append_event(
        output,
        endpoint,
        event,
        watched_filter,
        event_db,
        pending_enrichment,
    )
    .await?;
    if config.enrich {
        let queued_at = Instant::now();
        let http = http.clone();
        let http_urls = config.http_urls.clone();
        let http_endpoint = http_endpoint.to_string();
        let config = config.clone();
        let filter = watched_filter.cloned();
        let signature = signature.clone();
        let excluded_owners = config
            .filters
            .iter()
            .filter_map(|filter| filter.address.as_deref())
            .map(str::to_ascii_lowercase)
            .collect::<HashSet<_>>();
        let event_db = Arc::clone(event_db);
        let decode_slots = Arc::clone(decode_slots);
        tokio::spawn(async move {
            let Ok(_permit) = decode_slots.acquire_owned().await else {
                return;
            };
            let queue_ms = queued_at.elapsed().as_millis() as u64;
            let job_started = Instant::now();
            let mut enrichment = Value::Null;
            let mut attempts = Vec::new();
            // getTransaction can be briefly unavailable just after logsSubscribe.
            // One short retry covers that race; retrying the full two-encoding RPC
            // three times multiplied provider timeouts into very long UI delays.
            for attempt in 0..2 {
                enrichment = enrich_transaction(
                    &http,
                    &http_urls,
                    &http_endpoint,
                    &signature,
                    &config,
                    &excluded_owners,
                )
                .await;
                attempts.push(json!({
                    "attempt": attempt + 1,
                    "available": enrichment.get("available").and_then(Value::as_bool),
                    "elapsed_ms": enrichment.pointer("/timing_ms/total").cloned(),
                    "error": enrichment.get("error").cloned(),
                }));
                if enrichment.get("available").and_then(Value::as_bool) == Some(true) {
                    break;
                }
                if attempt == 0
                    && enrichment.get("retryable").and_then(Value::as_bool) == Some(true)
                {
                    sleep(Duration::from_millis(150)).await;
                } else {
                    break;
                }
            }
            let total_ms = job_started.elapsed().as_millis() as u64;
            enrichment["diagnostics"] = json!({
                "queue_ms": queue_ms,
                "job_ms": total_ms,
                "total_latency_ms": queue_ms.saturating_add(total_ms),
                "attempts": attempts,
            });
            let transaction_decoded = enrichment.pointer("/transaction").is_some();
            if !transaction_decoded {
                let mut fallback = record
                    .pointer("/enrichment/transaction")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                fallback["protocol_explanation"] =
                    json!("Transaction fetch failed; decode will retry on a future notification");
                enrichment["transaction"] = fallback;
            }
            if transaction_decoded {
                annotate_protocol_decoder(&mut enrichment, filter.as_ref());
            }
            let transaction = enrichment.get("transaction").unwrap_or(&Value::Null);
            let trade = transaction.get("trade").unwrap_or(&Value::Null);
            let unresolved_reason = trade
                .get("reason")
                .and_then(Value::as_str)
                .filter(|_| trade.get("side").and_then(Value::as_str) == Some("unknown"))
                .map(ToOwned::to_owned);
            if let Some(object) = enrichment.as_object_mut() {
                if let Some(diagnostics) =
                    object.get_mut("diagnostics").and_then(Value::as_object_mut)
                {
                    diagnostics.insert(
                        "outcome".to_string(),
                        json!(if unresolved_reason.is_some() {
                            "transaction_decoded_trade_unresolved"
                        } else if transaction_decoded {
                            "transaction_decoded"
                        } else {
                            "transaction_unavailable"
                        }),
                    );
                    if let Some(reason) = unresolved_reason {
                        diagnostics.insert("attribution_reason".to_string(), json!(reason));
                    }
                }
                object.insert("pending".to_string(), json!(false));
                object.insert("completed_at_ms".to_string(), json!(now_ms()));
            }
            let mut record = record;
            record["enrichment"] = enrichment;
            if let Err(error) = update_enriched_event(&event_db, &record) {
                eprintln!(
                    "failed to persist asynchronous Solana decode for {signature}: {error:#}"
                );
            }
        });
    }
    refresh.schedule(
        watched_filter,
        Some(signature.as_str()),
        event
            .pointer("/params/result/context/slot")
            .and_then(Value::as_u64),
        http_endpoint,
        config,
        http,
        state,
    );
    Ok(())
}

#[derive(Clone)]
struct PoolRefreshCoordinator {
    db: Arc<Mutex<Connection>>,
    interval_ms: u128,
    last_refresh_ms: Arc<Mutex<HashMap<String, u128>>>,
}

impl PoolRefreshCoordinator {
    fn new(connection: Connection, interval_secs: u64) -> Self {
        Self {
            db: Arc::new(Mutex::new(connection)),
            interval_ms: interval_secs.max(1) as u128 * 1_000,
            last_refresh_ms: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn schedule(
        &self,
        watched_filter: Option<&WatchFilter>,
        event_signature: Option<&str>,
        event_slot: Option<u64>,
        http_endpoint: &str,
        config: &Config,
        http: &reqwest::Client,
        state: &AppState,
    ) {
        let Some(filter) = watched_filter else {
            return;
        };
        let Some(pool_address) = filter.address.clone() else {
            return;
        };
        if !self.claim(&pool_address) {
            return;
        }

        let db = Arc::clone(&self.db);
        let preferred_endpoint = http_endpoint.to_string();
        let http_urls = config.http_urls.clone();
        let commitment = config.commitment.clone();
        let protocol = filter.protocol.clone();
        let pool_type = filter.pool_type.clone();
        let token_a_symbol = filter.token_a_symbol.clone();
        let token_b_symbol = filter.token_b_symbol.clone();
        let sol_price_usd = config.sol_price_usd;
        let event_signature = event_signature.map(ToOwned::to_owned);
        let http = http.clone();
        let state = state.clone();
        tokio::spawn(async move {
            let mut snapshot = fetch_pool_snapshot(
                &http,
                &http_urls,
                &preferred_endpoint,
                &pool_address,
                &commitment,
            )
            .await;
            if let Some(pool_type) = pool_type.as_deref() {
                let is_sol_symbol = |symbol: Option<&String>| {
                    symbol.is_some_and(|value| {
                        matches!(value.to_ascii_uppercase().as_str(), "SOL" | "WSOL")
                    })
                };
                let price_request = crate::price_quote::PriceRequest {
                    pool_type: pool_type.to_string(),
                    token0_anchor_usd: is_sol_symbol(token_a_symbol.as_ref())
                        .then_some(sol_price_usd)
                        .flatten(),
                    token1_anchor_usd: is_sol_symbol(token_b_symbol.as_ref())
                        .then_some(sol_price_usd)
                        .flatten(),
                    token0_symbol: token_a_symbol,
                    token1_symbol: token_b_symbol,
                };
                if let Ok(decoded_price) = crate::price_quote::decode_pool(
                    state,
                    SOLANA_ROUTE,
                    &pool_address,
                    price_request,
                )
                .await
                {
                    if let Some(snapshot_object) = snapshot.as_object_mut() {
                        snapshot_object.insert("decoded_price".to_string(), decoded_price);
                    }
                }
            }
            if let Err(error) = persist_pool_snapshot(
                &db,
                &pool_address,
                protocol.as_deref(),
                pool_type.as_deref(),
                event_signature.as_deref(),
                event_slot,
                &snapshot,
            ) {
                eprintln!("persist Solana pool snapshot failed for {pool_address}: {error:#}");
            }
        });
    }

    fn claim(&self, pool_address: &str) -> bool {
        let now = now_ms();
        let Ok(mut last_refresh) = self.last_refresh_ms.lock() else {
            return false;
        };
        if last_refresh
            .get(pool_address)
            .is_some_and(|last| now.saturating_sub(*last) < self.interval_ms)
        {
            return false;
        }
        last_refresh.insert(pool_address.to_string(), now);
        true
    }
}

fn initialize_realtime_db(path: &PathBuf) -> Result<Connection> {
    let connection = Connection::open(path)
        .with_context(|| format!("open Solana realtime database {}", path.display()))?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS solana_pool_snapshots (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           refreshed_at_ms INTEGER NOT NULL,
           pool_address TEXT NOT NULL,
           watched_protocol TEXT,
           watched_pool_type TEXT,
           event_signature TEXT,
           event_slot INTEGER,
           account_slot INTEGER,
           available INTEGER NOT NULL DEFAULT 0,
           rpc_endpoint TEXT,
           snapshot_json TEXT NOT NULL,
           UNIQUE(pool_address, refreshed_at_ms)
         );
         CREATE TABLE IF NOT EXISTS solana_pool_latest (
           pool_address TEXT PRIMARY KEY,
           refreshed_at_ms INTEGER NOT NULL,
           watched_protocol TEXT,
           watched_pool_type TEXT,
           event_signature TEXT,
           event_slot INTEGER,
           account_slot INTEGER,
           available INTEGER NOT NULL DEFAULT 0,
           rpc_endpoint TEXT,
           snapshot_json TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_solana_pool_snapshots_refreshed
           ON solana_pool_snapshots(refreshed_at_ms DESC);
         CREATE INDEX IF NOT EXISTS idx_solana_pool_snapshots_pool
         ON solana_pool_snapshots(pool_address, refreshed_at_ms DESC);",
    )?;
    ensure_sqlite_column(
        &connection,
        "solana_pool_snapshots",
        "watched_pool_type",
        "TEXT",
    )?;
    ensure_sqlite_column(
        &connection,
        "solana_pool_latest",
        "watched_pool_type",
        "TEXT",
    )?;
    Ok(connection)
}

async fn fetch_pool_snapshot(
    http: &reqwest::Client,
    http_urls: &[String],
    preferred_url: &str,
    pool_address: &str,
    commitment: &str,
) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "getAccountInfo",
        "params": [pool_address, {"commitment": commitment, "encoding": "base64"}]
    });
    let mut endpoints = Vec::with_capacity(http_urls.len());
    endpoints.push(preferred_url);
    endpoints.extend(
        http_urls
            .iter()
            .map(String::as_str)
            .filter(|endpoint| *endpoint != preferred_url),
    );
    let mut errors = Vec::new();
    for endpoint in endpoints {
        let response = match http.post(endpoint).json(&request).send().await {
            Ok(response) => response,
            Err(error) => {
                errors.push(format!("{}: {error}", display_url(endpoint)));
                continue;
            }
        };
        if !response.status().is_success() {
            errors.push(format!(
                "{}: HTTP {}",
                display_url(endpoint),
                response.status()
            ));
            continue;
        }
        let payload: Value = match response.json().await {
            Ok(payload) => payload,
            Err(error) => {
                errors.push(format!("{}: invalid JSON: {error}", display_url(endpoint)));
                continue;
            }
        };
        if let Some(error) = payload.get("error") {
            errors.push(format!("{}: RPC error {error}", display_url(endpoint)));
            continue;
        }
        let result = payload.get("result").cloned().unwrap_or(Value::Null);
        return json!({
            "available": !result.get("value").is_none_or(Value::is_null),
            "rpc_endpoint": display_url(endpoint),
            "slot": result.pointer("/context/slot"),
            "account": result.get("value"),
        });
    }
    json!({
        "available": false,
        "error": if errors.is_empty() {
            "pool account is not available from configured RPC endpoints".to_string()
        } else {
            errors.join("; ")
        }
    })
}

fn persist_pool_snapshot(
    db: &Arc<Mutex<Connection>>,
    pool_address: &str,
    watched_protocol: Option<&str>,
    watched_pool_type: Option<&str>,
    event_signature: Option<&str>,
    event_slot: Option<u64>,
    snapshot: &Value,
) -> Result<()> {
    let refreshed_at_ms = now_ms() as i64;
    let account_slot = snapshot
        .get("slot")
        .and_then(Value::as_u64)
        .map(|value| value as i64);
    let available = snapshot
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let connection = db
        .lock()
        .map_err(|_| anyhow!("Solana realtime database lock poisoned"))?;
    let transaction = connection.unchecked_transaction()?;
    transaction.execute(
        "INSERT OR IGNORE INTO solana_pool_snapshots (
           refreshed_at_ms, pool_address, watched_protocol, watched_pool_type,
           event_signature, event_slot, account_slot, available, rpc_endpoint, snapshot_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            refreshed_at_ms,
            pool_address,
            watched_protocol,
            watched_pool_type,
            event_signature,
            event_slot.map(|value| value as i64),
            account_slot,
            if available { 1 } else { 0 },
            snapshot.get("rpc_endpoint").and_then(Value::as_str),
            snapshot.to_string(),
        ],
    )?;
    transaction.execute(
        "INSERT INTO solana_pool_latest (
           pool_address, refreshed_at_ms, watched_protocol, watched_pool_type,
           event_signature, event_slot, account_slot, available, rpc_endpoint, snapshot_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(pool_address) DO UPDATE SET
           refreshed_at_ms=excluded.refreshed_at_ms,
           watched_protocol=excluded.watched_protocol,
           watched_pool_type=excluded.watched_pool_type,
           event_signature=excluded.event_signature,
           event_slot=excluded.event_slot,
           account_slot=excluded.account_slot,
           available=excluded.available,
           rpc_endpoint=excluded.rpc_endpoint,
           snapshot_json=excluded.snapshot_json",
        params![
            pool_address,
            refreshed_at_ms,
            watched_protocol,
            watched_pool_type,
            event_signature,
            event_slot.map(|value| value as i64),
            account_slot,
            if available { 1 } else { 0 },
            snapshot.get("rpc_endpoint").and_then(Value::as_str),
            snapshot.to_string(),
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

fn initialize_event_db(path: &PathBuf) -> Result<Connection> {
    let connection = Connection::open(path)
        .with_context(|| format!("open listener event database {}", path.display()))?;
    prepare_listener_database(&connection)?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS solana_events (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           observed_at_ms INTEGER NOT NULL,
           signature TEXT,
           watched_scope TEXT NOT NULL,
           watched_pool TEXT,
           watched_protocol TEXT,
           watched_pool_type TEXT,
           slot INTEGER,
           event_json TEXT NOT NULL,
           enrichment_json TEXT NOT NULL,
           target_token TEXT,
           side TEXT,
           trader TEXT,
           notional_usd REAL,
           is_whale INTEGER NOT NULL DEFAULT 0,
           alert_type TEXT,
           UNIQUE(signature, watched_scope)
         );
         CREATE INDEX IF NOT EXISTS idx_solana_events_observed
           ON solana_events(observed_at_ms DESC);
         CREATE INDEX IF NOT EXISTS idx_solana_events_token
           ON solana_events(target_token, observed_at_ms DESC);
         CREATE INDEX IF NOT EXISTS idx_solana_events_whale
           ON solana_events(is_whale, observed_at_ms DESC);
         CREATE TABLE IF NOT EXISTS solana_listener_meta (
           key TEXT PRIMARY KEY,
           value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS solana_pool_trades (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           observed_at_ms INTEGER NOT NULL,
           signature TEXT NOT NULL,
           watched_pool TEXT NOT NULL,
           watched_protocol TEXT,
           watched_pool_type TEXT,
           slot INTEGER,
           trader TEXT,
           target_mint TEXT NOT NULL,
           side TEXT NOT NULL,
           target_amount_ui REAL NOT NULL,
           quote_mint TEXT,
           quote_amount_ui REAL,
           swap_ratio_quote_per_target REAL,
           swap_price_usd REAL,
           notional_usd REAL,
           is_whale INTEGER NOT NULL DEFAULT 0,
           trade_json TEXT NOT NULL,
           UNIQUE(watched_pool, signature)
         );
         CREATE INDEX IF NOT EXISTS idx_solana_pool_trades_pool_time
           ON solana_pool_trades(watched_pool, observed_at_ms DESC);
         CREATE INDEX IF NOT EXISTS idx_solana_pool_trades_whale
           ON solana_pool_trades(is_whale, observed_at_ms DESC);",
    )?;
    ensure_sqlite_column(&connection, "solana_events", "watched_pool_type", "TEXT")?;
    ensure_sqlite_column(
        &connection,
        "solana_pool_trades",
        "watched_pool_type",
        "TEXT",
    )?;
    Ok(connection)
}

fn initialize_whale_db(path: &PathBuf) -> Result<Connection> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let connection = Connection::open(path)
        .with_context(|| format!("open listener whale database {}", path.display()))?;
    prepare_listener_database(&connection)?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS solana_whale_alerts (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           observed_at_ms INTEGER NOT NULL,
           signature TEXT NOT NULL,
           watched_pool TEXT,
           watched_protocol TEXT,
           watched_pool_type TEXT,
           slot INTEGER,
           trader TEXT,
           target_mint TEXT,
           side TEXT,
           target_amount_ui REAL,
           quote_mint TEXT,
           quote_amount_ui REAL,
           swap_ratio_quote_per_target REAL,
           swap_price_usd REAL,
           notional_usd REAL,
           trade_json TEXT NOT NULL,
           event_json TEXT NOT NULL,
           UNIQUE(watched_pool, signature)
         );
         CREATE INDEX IF NOT EXISTS idx_solana_whale_alerts_time
           ON solana_whale_alerts(observed_at_ms DESC);
         CREATE TABLE IF NOT EXISTS solana_whale_meta (
           key TEXT PRIMARY KEY, value TEXT NOT NULL
         );",
    )?;
    Ok(connection)
}

fn prepare_listener_database(connection: &Connection) -> Result<()> {
    connection.busy_timeout(Duration::from_secs(5))?;
    let mode = connection.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        connection.execute_batch("PRAGMA journal_mode=WAL;")?;
    }
    Ok(())
}

fn whale_db_path_for_event(event_db: &Connection) -> PathBuf {
    if let Some(path) = env::var_os("SOLANA_LISTENER_WHALE_DB") {
        return PathBuf::from(path);
    }
    if let Some(event_path) = event_db.path().map(PathBuf::from) {
        let parent = event_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let stem = event_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("solana_events");
        let whale_name = if stem == "solana_events" {
            "solana_whales.db".to_string()
        } else {
            format!("{stem}_whales.db")
        };
        return parent.join(whale_name);
    }
    PathBuf::from(DEFAULT_WHALE_DB)
}

fn persist_whale_alert_to_db(connection: &Connection, record: &Value, trade: &Value) -> Result<()> {
    if trade.get("is_whale").and_then(Value::as_bool) != Some(true)
        || record
            .pointer("/enrichment/transaction/err")
            .is_some_and(|error| !error.is_null())
    {
        return Ok(());
    }
    let Some(signature) = record.get("signature").and_then(Value::as_str) else {
        return Ok(());
    };
    connection.execute(
        "INSERT OR REPLACE INTO solana_whale_alerts (
           observed_at_ms, signature, watched_pool, watched_protocol, watched_pool_type, slot,
           trader, target_mint, side, target_amount_ui, quote_mint, quote_amount_ui,
           swap_ratio_quote_per_target, swap_price_usd, notional_usd, trade_json, event_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            record
                .get("observed_at_ms")
                .and_then(Value::as_u64)
                .unwrap_or_default() as i64,
            signature,
            record.get("watched_pool").and_then(Value::as_str),
            record.get("watched_protocol").and_then(Value::as_str),
            record.get("watched_pool_type").and_then(Value::as_str),
            record
                .get("slot")
                .and_then(Value::as_u64)
                .map(|value| value as i64),
            trade.get("trader").and_then(Value::as_str),
            trade.get("target_mint").and_then(Value::as_str),
            trade.get("side").and_then(Value::as_str),
            trade.get("target_amount_ui").and_then(Value::as_f64),
            trade.get("quote_mint").and_then(Value::as_str),
            trade.get("quote_amount_ui").and_then(Value::as_f64),
            trade
                .get("swap_ratio_quote_per_target")
                .and_then(Value::as_f64),
            trade.get("swap_price_usd").and_then(Value::as_f64),
            trade.get("notional_usd").and_then(Value::as_f64),
            trade.to_string(),
            record.to_string(),
        ],
    )?;
    Ok(())
}

fn migrate_legacy_whale_alerts(
    event_db: &Connection,
    event_path: &PathBuf,
    whale_db: &Connection,
) -> Result<usize> {
    let exists = event_db
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='solana_whale_alerts'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        return Ok(0);
    }
    whale_db.execute(
        "ATTACH DATABASE ?1 AS legacy_events",
        [event_path.to_string_lossy().as_ref()],
    )?;
    let copied = whale_db.execute(
        "INSERT OR IGNORE INTO main.solana_whale_alerts (
           observed_at_ms,signature,watched_pool,watched_protocol,watched_pool_type,slot,
           trader,target_mint,side,target_amount_ui,quote_mint,quote_amount_ui,
           swap_ratio_quote_per_target,swap_price_usd,notional_usd,trade_json,event_json
         ) SELECT observed_at_ms,signature,watched_pool,watched_protocol,watched_pool_type,slot,
           trader,target_mint,side,target_amount_ui,quote_mint,quote_amount_ui,
           swap_ratio_quote_per_target,swap_price_usd,notional_usd,trade_json,event_json
         FROM legacy_events.solana_whale_alerts",
        [],
    )?;
    whale_db.execute_batch("DETACH DATABASE legacy_events")?;
    event_db.execute_batch(
        "DROP INDEX IF EXISTS idx_solana_whale_alerts_time; DROP TABLE solana_whale_alerts;",
    )?;
    Ok(copied)
}

fn restore_whale_alerts_from_log(connection: &Connection, path: &PathBuf) -> Result<usize> {
    let restored = connection
        .query_row(
            "SELECT value FROM solana_whale_meta WHERE key = 'whale_log_recovery_v2'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if restored.as_deref() == Some("complete") {
        return Ok(0);
    }

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("read historical Solana listener log {}", path.display()))?;
    let mut count = 0;
    for line in contents.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(trade) = record.pointer("/enrichment/transaction/trade") else {
            continue;
        };
        if record
            .pointer("/enrichment/transaction/err")
            .is_some_and(|error| !error.is_null())
        {
            continue;
        }
        if trade.get("is_whale").and_then(Value::as_bool) == Some(true) {
            persist_whale_alert_to_db(connection, &record, trade)?;
            count += 1;
        }
    }

    let failed_alerts = {
        let mut statement = connection
            .prepare("SELECT signature, watched_pool, event_json FROM solana_whale_alerts")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (signature, pool, event_json) in failed_alerts {
        let failed = serde_json::from_str::<Value>(&event_json)
            .ok()
            .and_then(|record| {
                record
                    .pointer("/enrichment/transaction/err")
                    .map(|error| !error.is_null())
            })
            .unwrap_or(false);
        if failed {
            connection.execute(
                "DELETE FROM solana_whale_alerts WHERE signature = ?1 AND watched_pool IS ?2",
                params![signature, pool],
            )?;
        }
    }
    connection.execute(
        "INSERT INTO solana_whale_meta (key, value)
         VALUES ('whale_log_recovery_v2', 'complete')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [],
    )?;
    Ok(count)
}

fn cleanup_unmatched_route_trades_from_log(
    connection: &Connection,
    whale_connection: &Connection,
    path: &PathBuf,
) -> Result<usize> {
    let complete = connection
        .query_row(
            "SELECT value FROM solana_listener_meta WHERE key = 'route_pool_cleanup_v2'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if complete.as_deref() == Some("complete") {
        return Ok(0);
    }

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("read historical Solana listener log {}", path.display()))?;
    let mut route_status = HashMap::<(String, String), (bool, bool, bool)>::new();
    for line in contents.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let (Some(signature), Some(pool)) = (
            record.get("signature").and_then(Value::as_str),
            record.get("watched_pool").and_then(Value::as_str),
        ) else {
            continue;
        };
        let transaction = record.pointer("/enrichment/transaction");
        let failed = transaction
            .and_then(|transaction| transaction.get("err"))
            .is_some_and(|error| !error.is_null());
        let routes = transaction
            .and_then(|transaction| transaction.get("sdk_route_swaps"))
            .and_then(Value::as_array)
            .filter(|routes| !routes.is_empty());
        let status = route_status
            .entry((signature.to_ascii_lowercase(), pool.to_ascii_lowercase()))
            .or_default();
        status.2 |= failed;
        if let Some(routes) = routes {
            status.0 = true;
            status.1 |= routes.iter().any(|route| {
                route
                    .get("pool")
                    .and_then(Value::as_str)
                    .is_some_and(|route_pool| route_pool.eq_ignore_ascii_case(pool))
            });
        }
    }

    let mut removed = 0;
    for ((signature, pool), (had_routes, matched_pool, failed)) in route_status {
        if failed || (had_routes && !matched_pool) {
            removed += connection.execute(
                "DELETE FROM solana_pool_trades
                 WHERE lower(signature) = ?1 AND lower(watched_pool) = ?2",
                params![signature, pool],
            )?;
            whale_connection.execute(
                "DELETE FROM solana_whale_alerts
                 WHERE lower(signature) = ?1 AND lower(watched_pool) = ?2",
                params![signature, pool],
            )?;
        }
    }
    connection.execute(
        "INSERT INTO solana_listener_meta (key, value)
         VALUES ('route_pool_cleanup_v2', 'complete')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [],
    )?;
    Ok(removed)
}

fn ensure_sqlite_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !names.iter().any(|name| name.eq_ignore_ascii_case(column)) {
        connection.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn persist_event(event_db: &Arc<Mutex<Connection>>, record: &Value) -> Result<()> {
    let enrichment = record.get("enrichment").cloned().unwrap_or(Value::Null);
    let trade = enrichment
        .pointer("/transaction/trade")
        .cloned()
        .unwrap_or(Value::Null);
    let signature = record
        .get("signature")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let watched_pool = record
        .get("watched_pool")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let watched_scope = watched_pool.clone().unwrap_or_else(|| "all".to_string());
    let observed_at_ms = record
        .get("observed_at_ms")
        .and_then(Value::as_u64)
        .unwrap_or_default() as i64;
    let slot = record
        .get("slot")
        .and_then(Value::as_u64)
        .map(|value| value as i64);
    let notional_usd = trade.get("notional_usd").and_then(Value::as_f64);
    let is_whale = trade
        .get("is_whale")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let connection = event_db
        .lock()
        .map_err(|_| anyhow!("listener event database lock poisoned"))?;
    connection.execute(
        "INSERT OR IGNORE INTO solana_events (
           observed_at_ms, signature, watched_scope, watched_pool,
           watched_protocol, watched_pool_type, slot, event_json, enrichment_json,
           target_token, side, trader, notional_usd, is_whale, alert_type
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            observed_at_ms,
            signature,
            watched_scope,
            watched_pool,
            record.get("watched_protocol").and_then(Value::as_str),
            record.get("watched_pool_type").and_then(Value::as_str),
            slot,
            record.to_string(),
            enrichment.to_string(),
            trade.get("target_mint").and_then(Value::as_str),
            trade.get("side").and_then(Value::as_str),
            trade.get("trader").and_then(Value::as_str),
            notional_usd,
            if is_whale { 1 } else { 0 },
            trade.get("alert_type").and_then(Value::as_str),
        ],
    )?;
    persist_trade_row(&connection, record, &trade)?;
    persist_whale_alert(&connection, record, &trade)?;
    prune_recent_events(&connection)?;
    Ok(())
}

fn persist_whale_alert(connection: &Connection, record: &Value, trade: &Value) -> Result<()> {
    if trade.get("is_whale").and_then(Value::as_bool) != Some(true)
        || record
            .pointer("/enrichment/transaction/err")
            .is_some_and(|error| !error.is_null())
    {
        return Ok(());
    }
    let whale_path = whale_db_path_for_event(connection);
    let whale_db = initialize_whale_db(&whale_path)?;
    persist_whale_alert_to_db(&whale_db, record, trade)
}

fn delete_whale_alert_for_event_db(
    event_db: &Connection,
    signature: &str,
    watched_pool: Option<&str>,
) -> Result<()> {
    let whale_db = initialize_whale_db(&whale_db_path_for_event(event_db))?;
    whale_db.execute(
        "DELETE FROM solana_whale_alerts WHERE signature = ?1 AND watched_pool IS ?2",
        params![signature, watched_pool],
    )?;
    Ok(())
}

fn prune_recent_events(connection: &Connection) -> Result<()> {
    // Keep in-flight enrichments and a bounded set of recent diagnostics.
    // The API filters unresolved events, but pruning them here erased the
    // evidence needed to tell RPC failures from attribution failures.
    let rows = {
        let mut statement = connection.prepare(
            "SELECT id, event_json FROM solana_events ORDER BY observed_at_ms DESC, id DESC",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut visible_count = 0;
    let mut diagnostic_count = 0;
    let mut remove_ids = Vec::new();
    for (id, event_json) in rows {
        let Ok(event) = serde_json::from_str::<Value>(&event_json) else {
            remove_ids.push(id);
            continue;
        };
        if event
            .pointer("/enrichment/pending")
            .and_then(Value::as_bool)
            == Some(true)
        {
            continue;
        }
        if event_is_decoded_for_ui(&event) && visible_count < 18 {
            visible_count += 1;
            diagnostic_count += 1;
        } else if diagnostic_count < 218 {
            diagnostic_count += 1;
        } else {
            remove_ids.push(id);
        }
    }
    for id in remove_ids {
        connection.execute("DELETE FROM solana_events WHERE id = ?1", [id])?;
    }
    Ok(())
}

fn event_is_decoded_for_ui(event: &Value) -> bool {
    if event
        .pointer("/enrichment/transaction/err")
        .is_some_and(|error| !error.is_null())
        || event
            .pointer("/enrichment/pending")
            .and_then(Value::as_bool)
            == Some(true)
    {
        return false;
    }
    let event_kind = event
        .pointer("/enrichment/transaction/event_kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match event_kind {
        "swap" => {
            let trade = event.pointer("/enrichment/transaction/trade");
            matches!(
                trade
                    .and_then(|trade| trade.get("side"))
                    .and_then(Value::as_str),
                Some("buy" | "sell")
            ) && trade
                .and_then(|trade| trade.get("target_amount_ui"))
                .and_then(Value::as_f64)
                .is_some_and(|amount| amount > 0.0)
                && trade
                    .and_then(|trade| trade.get("quote_amount_ui"))
                    .and_then(Value::as_f64)
                    .is_some_and(|amount| amount > 0.0)
        }
        "add_liquidity" | "remove_liquidity" => {
            event
                .pointer("/enrichment/transaction/liquidity/detected")
                .and_then(Value::as_bool)
                == Some(true)
                || event
                    .pointer("/enrichment/transaction/protocol_details/amounts")
                    .and_then(Value::as_array)
                    .is_some_and(|amounts| {
                        amounts
                            .iter()
                            .any(|amount| amount.as_u64().is_some_and(|amount| amount > 0))
                    })
        }
        "protocol_event_unresolved" | "pool_maintenance" | "unrelated_router_event" => false,
        _ => true,
    }
}

fn update_event_json(connection: &Connection, event: &Value, enrichment: &Value) -> Result<()> {
    connection.execute(
        "UPDATE solana_events
         SET event_json = ?1, enrichment_json = ?2
         WHERE signature = ?3 AND watched_scope = COALESCE(?4, watched_scope)",
        params![
            event.to_string(),
            enrichment.to_string(),
            event.get("signature").and_then(Value::as_str),
            event.get("watched_pool").and_then(Value::as_str),
        ],
    )?;
    Ok(())
}

fn update_enriched_event(event_db: &Arc<Mutex<Connection>>, record: &Value) -> Result<()> {
    let enrichment = record.get("enrichment").cloned().unwrap_or(Value::Null);
    let trade = enrichment
        .pointer("/transaction/trade")
        .cloned()
        .unwrap_or(Value::Null);
    let signature = record
        .get("signature")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("asynchronous Solana enrichment has no signature"))?;
    let watched_pool = record.get("watched_pool").and_then(Value::as_str);
    let watched_scope = watched_pool.unwrap_or("all");
    let connection = event_db
        .lock()
        .map_err(|_| anyhow!("listener event database lock poisoned"))?;
    let updated = connection.execute(
        "UPDATE solana_events
         SET event_json = ?1, enrichment_json = ?2,
             target_token = ?3, side = ?4, trader = ?5,
             notional_usd = ?6, is_whale = ?7, alert_type = ?8
         WHERE signature = ?9 AND watched_scope = ?10",
        params![
            record.to_string(),
            enrichment.to_string(),
            trade.get("target_mint").and_then(Value::as_str),
            trade.get("side").and_then(Value::as_str),
            trade.get("trader").and_then(Value::as_str),
            trade.get("notional_usd").and_then(Value::as_f64),
            if trade.get("is_whale").and_then(Value::as_bool) == Some(true) {
                1
            } else {
                0
            },
            trade.get("alert_type").and_then(Value::as_str),
            signature,
            watched_scope,
        ],
    )?;
    if updated == 0 {
        return Ok(());
    }
    connection.execute(
        "DELETE FROM solana_pool_trades WHERE signature = ?1 AND watched_pool IS ?2",
        params![signature, watched_pool],
    )?;
    delete_whale_alert_for_event_db(&connection, &signature, watched_pool)?;
    persist_trade_row(&connection, record, &trade)?;
    persist_whale_alert(&connection, record, &trade)?;
    prune_recent_events(&connection)?;
    Ok(())
}

fn persist_trade_row(connection: &Connection, record: &Value, trade: &Value) -> Result<()> {
    if record
        .pointer("/enrichment/transaction/err")
        .is_some_and(|error| !error.is_null())
    {
        return Ok(());
    }
    let Some(signature) = record.get("signature").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(watched_pool) = record.get("watched_pool").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(side) = trade
        .get("side")
        .and_then(Value::as_str)
        .filter(|side| *side != "unknown")
    else {
        return Ok(());
    };
    let target_amount = trade
        .get("target_amount_ui")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    if target_amount <= 0.0 {
        return Ok(());
    }
    let notional_usd = trade.get("notional_usd").and_then(Value::as_f64);
    let is_whale = trade
        .get("is_whale")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    connection.execute(
        "INSERT OR REPLACE INTO solana_pool_trades (
           observed_at_ms, signature, watched_pool, watched_protocol, watched_pool_type, slot,
           trader, target_mint, side, target_amount_ui, quote_mint,
           quote_amount_ui, swap_ratio_quote_per_target, swap_price_usd,
           notional_usd, is_whale, trade_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            record
                .get("observed_at_ms")
                .and_then(Value::as_u64)
                .unwrap_or_default() as i64,
            signature,
            watched_pool,
            record.get("watched_protocol").and_then(Value::as_str),
            record.get("watched_pool_type").and_then(Value::as_str),
            record
                .get("slot")
                .and_then(Value::as_u64)
                .map(|value| value as i64),
            trade.get("trader").and_then(Value::as_str),
            trade
                .get("target_mint")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            side,
            target_amount,
            trade.get("quote_mint").and_then(Value::as_str),
            trade.get("quote_amount_ui").and_then(Value::as_f64),
            trade
                .get("swap_ratio_quote_per_target")
                .and_then(Value::as_f64),
            trade.get("swap_price_usd").and_then(Value::as_f64),
            notional_usd,
            if is_whale { 1 } else { 0 },
            trade.to_string(),
        ],
    )?;
    Ok(())
}

fn backfill_event_trades(
    connection: &Connection,
    target_token: &str,
    quote_mints: &HashSet<String>,
    whale_usd: f64,
    sol_price_usd: Option<f64>,
    excluded_owners: &HashSet<String>,
    pool_metadata: &HashMap<String, (Option<String>, Option<String>)>,
) -> Result<usize> {
    // Reclassify the recent event window after listener rules change, but keep
    // older derived trades and whale alerts as durable history. The source
    // event table is intentionally pruned to its latest 18 rows, so clearing
    // all rows for a token here would erase history that can no longer be
    // rebuilt from that table.
    let rows = {
        let mut statement = connection.prepare(
            "SELECT event_json, enrichment_json
             FROM solana_events
             ORDER BY id ASC",
        )?;
        let mapped = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        mapped
    };
    let mut count = 0;
    for (event_json, enrichment_json) in rows {
        let Ok(mut event) = serde_json::from_str::<Value>(&event_json) else {
            continue;
        };
        let Ok(mut enrichment) = serde_json::from_str::<Value>(&enrichment_json) else {
            continue;
        };
        if let Some(event_kind) = classify_log_event_kind(event.get("logs")) {
            if let Some(transaction) = enrichment.get_mut("transaction") {
                if let Some(transaction) = transaction.as_object_mut() {
                    transaction
                        .entry("event_kind".to_string())
                        .or_insert_with(|| json!(event_kind));
                }
            }
            event["enrichment"] = enrichment.clone();
            update_event_json(connection, &event, &enrichment)?;
        }
        if enrichment
            .pointer("/transaction/event_kind")
            .and_then(Value::as_str)
            == Some("program_event")
        {
            if let Some(transaction) = enrichment.get_mut("transaction") {
                if let Some(transaction) = transaction.as_object_mut() {
                    transaction.insert("event_kind".to_string(), json!("pool_program_event"));
                }
            }
            event["enrichment"] = enrichment.clone();
            update_event_json(connection, &event, &enrichment)?;
        }
        if enrichment
            .pointer("/transaction/event_kind")
            .and_then(Value::as_str)
            == Some("pool_program_event")
            && event.get("err").is_some_and(|error| !error.is_null())
        {
            if let Some(transaction) = enrichment.get_mut("transaction") {
                if let Some(transaction) = transaction.as_object_mut() {
                    transaction.insert("event_kind".to_string(), json!("failed_pool_program"));
                }
            }
            event["enrichment"] = enrichment.clone();
            update_event_json(connection, &event, &enrichment)?;
        }
        let token_changes = enrichment
            .pointer("/transaction/token_changes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let token_instructions = enrichment
            .pointer("/transaction/token_instructions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let has_target_balance = token_changes.iter().any(|change| {
            change
                .get("mint")
                .and_then(Value::as_str)
                .is_some_and(|mint| mint.eq_ignore_ascii_case(target_token))
        });
        let has_target_transfer = token_instructions.iter().any(|instruction| {
            instruction
                .get("info")
                .and_then(|info| info.get("mint"))
                .and_then(Value::as_str)
                .is_some_and(|mint| mint.eq_ignore_ascii_case(target_token))
        });
        if !has_target_balance && !has_target_transfer {
            continue;
        }
        if let Some(pool) = event.get("watched_pool").and_then(Value::as_str) {
            if let Some((protocol, pool_type)) = pool_metadata.get(&pool.to_ascii_lowercase()) {
                event["watched_protocol"] = json!(protocol);
                event["watched_pool_type"] = json!(pool_type);
            }
        }
        let trade = classify_trade(
            enrichment.pointer("/transaction").unwrap_or(&Value::Null),
            &token_changes,
            &token_instructions,
            Some(target_token),
            quote_mints,
            whale_usd,
            sol_price_usd,
            excluded_owners,
        );
        if let Some(transaction) = enrichment
            .get_mut("transaction")
            .and_then(Value::as_object_mut)
        {
            transaction.insert("trade".to_string(), trade.clone());
        }
        if let (Some(pool), Some(transaction)) = (
            event.get("watched_pool").and_then(Value::as_str),
            enrichment
                .get_mut("transaction")
                .and_then(Value::as_object_mut),
        ) {
            let filter = WatchFilter {
                address: Some(pool.to_string()),
                protocol: event
                    .get("watched_protocol")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                pool_type: event
                    .get("watched_pool_type")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                token_a_symbol: None,
                token_b_symbol: None,
            };
            let exact_route = apply_exact_route_trade(transaction, &filter);
            let exact_sdk_event = !exact_route
                && transaction
                    .get("sdk_protocol_events")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .find(|decoded| {
                        decoded.get("event").and_then(Value::as_str) == Some("MeteoraDlmmSwap")
                            && decoded
                                .get("pool_address")
                                .and_then(Value::as_str)
                                .zip(filter.address.as_deref())
                                .is_some_and(|(pool, watched_pool)| {
                                    pool.eq_ignore_ascii_case(watched_pool)
                                })
                    })
                    .cloned()
                    .is_some_and(|decoded| {
                        apply_meteora_dlmm_event_trade(transaction, &filter, &decoded)
                    });
            suppress_unreliable_route_trade(transaction, &filter, exact_route || exact_sdk_event);
        }
        let trade = enrichment
            .pointer("/transaction/trade")
            .cloned()
            .unwrap_or(trade);
        let classified = trade.get("side").and_then(Value::as_str) != Some("unknown");
        if let Some(transaction) = enrichment.get_mut("transaction") {
            if let Some(transaction) = transaction.as_object_mut() {
                transaction.insert("trade".to_string(), trade.clone());
            }
        }
        event["enrichment"] = enrichment.clone();
        let id = event.get("signature").and_then(Value::as_str);
        connection.execute(
            "UPDATE solana_events
             SET event_json = ?1, enrichment_json = ?2,
                 target_token = ?3, side = ?4, trader = ?5,
                 notional_usd = ?6, is_whale = ?7, alert_type = ?8,
                 watched_pool_type = ?9
             WHERE signature = ?10 AND watched_scope = COALESCE(?11, watched_scope)",
            params![
                event.to_string(),
                enrichment.to_string(),
                trade.get("target_mint").and_then(Value::as_str),
                trade.get("side").and_then(Value::as_str),
                trade.get("trader").and_then(Value::as_str),
                trade.get("notional_usd").and_then(Value::as_f64),
                if trade
                    .get("is_whale")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    1
                } else {
                    0
                },
                trade.get("alert_type").and_then(Value::as_str),
                event.get("watched_pool_type").and_then(Value::as_str),
                id,
                event.get("watched_pool").and_then(Value::as_str),
            ],
        )?;
        if let (Some(signature), Some(pool)) = (
            event.get("signature").and_then(Value::as_str),
            event.get("watched_pool").and_then(Value::as_str),
        ) {
            connection.execute(
                "DELETE FROM solana_pool_trades WHERE lower(signature) = lower(?1) AND lower(watched_pool) = lower(?2)",
                params![signature, pool],
            )?;
            delete_whale_alert_for_event_db(connection, signature, Some(pool))?;
        }
        if classified {
            persist_trade_row(connection, &event, &trade)?;
            persist_whale_alert(connection, &event, &trade)?;
            count += 1;
        }
    }
    Ok(count)
}

async fn append_event(
    output: &mut File,
    endpoint: &str,
    event: &Value,
    watched_filter: Option<&WatchFilter>,
    event_db: &Arc<Mutex<Connection>>,
    enrichment: Value,
) -> Result<Value> {
    let record = json!({
        "observed_at_ms": now_ms(),
        "endpoint": display_url(endpoint),
        "signature": notification_signature(event),
        "watched_pool": watched_filter.and_then(|filter| filter.address.clone()),
        "watched_protocol": watched_filter.and_then(|filter| filter.protocol.clone()),
        "watched_pool_type": watched_filter.and_then(|filter| filter.pool_type.clone()),
        "slot": event.pointer("/params/result/context/slot"),
        "logs": event.pointer("/params/result/value/logs"),
        "err": event.pointer("/params/result/value/err"),
        "event": event,
        "enrichment": enrichment,
    });
    output
        .write_all(record.to_string().as_bytes())
        .await
        .context("write Solana event")?;
    output
        .write_all(b"\n")
        .await
        .context("write event newline")?;
    output.flush().await.context("flush Solana event")?;
    persist_event(event_db, &record)?;
    Ok(record)
}

#[derive(Default)]
struct SeenSignatures {
    set: HashSet<String>,
    order: VecDeque<String>,
}

impl SeenSignatures {
    fn insert(&mut self, signature: &str, scope: &str) -> bool {
        let key = format!("{scope}:{signature}");
        if !self.set.insert(key.clone()) {
            return false;
        }
        self.order.push_back(key);
        if self.order.len() > 10_000 {
            if let Some(oldest) = self.order.pop_front() {
                self.set.remove(&oldest);
            }
        }
        true
    }
}

async fn enrich_transaction(
    http: &reqwest::Client,
    http_urls: &[String],
    preferred_url: &str,
    signature: &str,
    config: &Config,
    excluded_owners: &HashSet<String>,
) -> Value {
    let total_started = Instant::now();
    let request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "getTransaction",
        "params": [
            signature,
            {
                "commitment": config.commitment,
                "encoding": "jsonParsed",
                "maxSupportedTransactionVersion": 1
            }
        ]
    });
    let mut endpoints = Vec::with_capacity(http_urls.len());
    endpoints.push(preferred_url);
    endpoints.extend(
        http_urls
            .iter()
            .map(String::as_str)
            .filter(|endpoint| *endpoint != preferred_url),
    );
    let mut errors = Vec::new();
    let mut last_fetch_ms = 0u64;
    let mut result_was_null = false;

    for endpoint in endpoints {
        // Both encodings are needed by the current attribution path. Fetch
        // them together so SDK decoding does not add a second RPC round trip.
        let fetch_started = Instant::now();
        let (response, sdk_transaction) = tokio::join!(
            http.post(endpoint).json(&request).send(),
            fetch_sdk_transaction(http, endpoint, signature, config),
        );
        last_fetch_ms = fetch_started.elapsed().as_millis() as u64;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                errors.push(format!("{}: {error}", display_url(endpoint)));
                continue;
            }
        };
        if !response.status().is_success() {
            errors.push(format!(
                "{}: HTTP {}",
                display_url(endpoint),
                response.status()
            ));
            continue;
        }
        let payload: Value = match response.json().await {
            Ok(payload) => payload,
            Err(error) => {
                errors.push(format!("{}: invalid JSON: {error}", display_url(endpoint)));
                continue;
            }
        };
        if let Some(error) = payload.get("error") {
            errors.push(format!("{}: RPC error {error}", display_url(endpoint)));
            continue;
        }
        let Some(transaction) = payload.get("result") else {
            errors.push(format!("{}: missing result", display_url(endpoint)));
            continue;
        };
        if transaction.is_null() {
            result_was_null = true;
            continue;
        }
        let transaction = transaction.clone();
        let sdk_transaction = sdk_transaction;
        let signature = signature.to_owned();
        let config = config.clone();
        let excluded_owners = excluded_owners.clone();
        let decode_started = Instant::now();
        let Ok(summary) = tokio::task::spawn_blocking(move || {
            summarize_transaction(
                &signature,
                &transaction,
                sdk_transaction.as_ref(),
                &config,
                &excluded_owners,
            )
        })
        .await
        else {
            return json!({
                "available": false,
                "error": "transaction decode worker failed",
                "timing_ms": {"fetch": last_fetch_ms, "total": total_started.elapsed().as_millis() as u64}
            });
        };
        let decode_ms = decode_started.elapsed().as_millis() as u64;
        return json!({
            "available": true,
            "rpc_endpoint": display_url(endpoint),
            "transaction": summary,
            "timing_ms": {
                "fetch": last_fetch_ms,
                "decode": decode_ms,
                "total": total_started.elapsed().as_millis() as u64
            },
        });
    }

    let error = if errors.is_empty() {
        "transaction is not available from configured RPC endpoints".to_string()
    } else {
        errors.join("; ")
    };
    json!({
        "available": false,
        "error": error,
        // A null result immediately after a log notification can be a brief
        // commitment/indexing race. Transport/RPC errors already fail over
        // through the configured endpoints and should not replay the whole set.
        "retryable": result_was_null && errors.is_empty(),
        "timing_ms": {"fetch": last_fetch_ms, "total": total_started.elapsed().as_millis() as u64}
    })
}

async fn fetch_sdk_transaction(
    http: &reqwest::Client,
    endpoint: &str,
    signature: &str,
    config: &Config,
) -> Option<Value> {
    if env::var("SOLANA_LISTENER_SDK_DECODE")
        .ok()
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "no"))
    {
        return None;
    }
    let request = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "getTransaction",
        "params": [
            signature,
            {
                "commitment": config.commitment,
                "encoding": "base64",
                "maxSupportedTransactionVersion": 1
            }
        ]
    });
    let response = http.post(endpoint).json(&request).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let payload: Value = response.json().await.ok()?;
    if payload.get("error").is_some() {
        return None;
    }
    payload
        .get("result")
        .filter(|value| !value.is_null())
        .cloned()
}

fn summarize_transaction(
    signature: &str,
    transaction: &Value,
    sdk_transaction: Option<&Value>,
    config: &Config,
    excluded_owners: &HashSet<String>,
) -> Value {
    let meta = transaction.get("meta").unwrap_or(&Value::Null);
    let token_changes = extract_token_changes(meta);
    let raw_protocol_events = solana_decoders::decode_transaction(transaction);
    let sdk_protocol_events = sdk_transaction
        .map(solana_sdk_decoder::decode_transaction)
        .unwrap_or_default();
    let sdk_route_swaps = sdk_transaction
        .map(solana_sdk_decoder::decode_routes)
        .unwrap_or_default();
    let account_mints = token_account_mints(transaction);
    let mut token_mints = token_changes
        .iter()
        .filter_map(|change| change.get("mint").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect::<HashSet<_>>();
    let account_keys = account_keys(transaction);
    let mut token_instructions = Vec::new();
    collect_token_instructions(
        transaction.pointer("/transaction/message/instructions"),
        &account_keys,
        &mut token_instructions,
        &mut token_mints,
    );
    if let Some(inner) = meta.get("innerInstructions").and_then(Value::as_array) {
        for group in inner {
            collect_token_instructions(
                group.get("instructions"),
                &account_keys,
                &mut token_instructions,
                &mut token_mints,
            );
        }
    }
    let mut token_mints = token_mints.into_iter().collect::<Vec<_>>();
    token_mints.sort();
    let trade = classify_trade(
        transaction,
        &token_changes,
        &token_instructions,
        config.target_token.as_deref(),
        &config.quote_mints,
        config.whale_usd,
        config.sol_price_usd,
        excluded_owners,
    );
    let event_kind = classify_event_kind(meta, &token_instructions, &token_changes);
    let liquidity = classify_liquidity_flow(
        transaction,
        &token_changes,
        &token_instructions,
        config.target_token.as_deref(),
        &config.quote_mints,
        excluded_owners,
    );
    let mut trade = trade;
    if trade.get("side").and_then(Value::as_str) == Some("unknown") {
        if let Some(object) = trade.as_object_mut() {
            object.insert(
                "reason".to_string(),
                json!(match event_kind.as_str() {
                    "add_liquidity" => "liquidity add or LP deposit; no trader swap pair",
                    "remove_liquidity" => "liquidity removal or LP withdrawal; no trader swap pair",
                    "pool_state_change" => "pool token balance change without a trader swap pair",
                    "pool_program_event" => "pool program event without token balance deltas",
                    "protocol_event_unresolved" => "known pool program invoked, but its instruction was not decoded",
                    "failed_program_event" => "known pool program failed before token balances settled",
                    "failed_pool_program" => "pool/LP program failed before token balances settled",
                    "swap" => "swap instruction detected, but the transaction did not settle a user balance pair",
                    _ => "no owner balance pair decoded",
                }),
            );
        }
    }

    json!({
        "signature": signature,
        "slot": transaction.get("slot"),
        "block_time": transaction.get("blockTime"),
        "err": meta.get("err"),
        "program_ids": program_ids(transaction, &account_keys),
        "token_mints": token_mints,
        "token_account_mints": account_mints,
        "token_changes": token_changes,
        "token_instructions": token_instructions,
        "log_messages": meta.get("logMessages").cloned().unwrap_or_else(|| json!([])),
        "raw_protocol_events": raw_protocol_events
            .iter()
            .map(|event| json!({
                "protocol": event.protocol,
                "pool_type": event.pool_type,
                "program_id": event.program_id,
                "instruction": event.instruction,
                "event_kind": event.event_kind,
                "action": event.action,
                "explanation": event.explanation,
            }))
            .collect::<Vec<_>>(),
        "sdk_protocol_events": sdk_protocol_events,
        "sdk_route_swaps": sdk_route_swaps,
        "event_kind": event_kind,
        "liquidity": liquidity,
        "trade": trade,
    })
}

fn classify_event_kind(
    meta: &Value,
    token_instructions: &[Value],
    token_changes: &[Value],
) -> String {
    let logs = meta.get("logMessages");
    let instructions = token_instructions
        .iter()
        .filter_map(|instruction| instruction.get("type").and_then(Value::as_str))
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    if let Some(kind) = classify_log_event_kind(logs) {
        return kind.to_string();
    }
    let has = |needle: &str| {
        log_contains(logs, needle)
            || instructions
                .iter()
                .any(|instruction| instruction.contains(needle))
    };
    if has("addliquidity")
        || has("add_liquidity")
        || has("increaseliquidity")
        || has("increase_liquidity")
    {
        "add_liquidity".to_string()
    } else if has("removeliquidity")
        || has("remove_liquidity")
        || has("decreaseliquidity")
        || has("decrease_liquidity")
    {
        "remove_liquidity".to_string()
    } else if has("swap") {
        "swap".to_string()
    } else if !token_changes.is_empty() {
        "pool_state_change".to_string()
    } else if meta.get("err").is_some_and(|error| !error.is_null()) {
        "failed_program_event".to_string()
    } else {
        "protocol_event_unresolved".to_string()
    }
}

fn classify_liquidity_flow(
    transaction: &Value,
    token_changes: &[Value],
    token_instructions: &[Value],
    target_token: Option<&str>,
    quote_mints: &HashSet<String>,
    excluded_owners: &HashSet<String>,
) -> Value {
    let Some(target_token) = target_token else {
        return Value::Null;
    };
    let target_token = target_token.to_ascii_lowercase();
    let mut owners = HashMap::<String, (f64, HashMap<String, f64>)>::new();
    for change in token_changes {
        let Some(owner) = change.get("owner").and_then(Value::as_str) else {
            continue;
        };
        if excluded_owners.contains(&owner.to_ascii_lowercase()) {
            continue;
        }
        let Some(mint) = change.get("mint").and_then(Value::as_str) else {
            continue;
        };
        let delta = change
            .get("delta_ui_amount")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        if mint.eq_ignore_ascii_case(&target_token) {
            owners.entry(owner.to_string()).or_default().0 += delta;
        } else if quote_mints.contains(&mint.to_ascii_lowercase()) {
            *owners
                .entry(owner.to_string())
                .or_default()
                .1
                .entry(mint.to_string())
                .or_default() += delta;
        }
    }

    for (owner, (target_delta, quote_deltas)) in &mut owners {
        if target_delta.abs() <= f64::EPSILON || !quote_deltas.is_empty() {
            continue;
        }
        let inferred = token_instructions
            .iter()
            .filter_map(|instruction| {
                if !matches!(
                    instruction.get("type").and_then(Value::as_str),
                    Some("transfer" | "transferChecked")
                ) {
                    return None;
                }
                let info = instruction.get("info")?;
                if !info
                    .get("authority")
                    .and_then(Value::as_str)
                    .is_some_and(|authority| authority.eq_ignore_ascii_case(owner))
                {
                    return None;
                }
                let mint = info.get("mint").and_then(Value::as_str)?;
                if !quote_mints.contains(&mint.to_ascii_lowercase()) {
                    return None;
                }
                let amount = info
                    .get("tokenAmount")
                    .and_then(|value| value.get("uiAmount"))
                    .and_then(Value::as_f64)
                    .or_else(|| {
                        let value = info.get("tokenAmount")?;
                        let raw = value.get("amount")?.as_str()?.parse::<f64>().ok()?;
                        let decimals = value
                            .get("decimals")
                            .and_then(Value::as_u64)
                            .unwrap_or_default();
                        Some(raw / 10f64.powi(decimals as i32))
                    })
                    .filter(|amount| amount.is_finite() && *amount > 0.0)?;
                Some((mint.to_string(), amount))
            })
            .max_by(|left, right| left.1.total_cmp(&right.1));
        if let Some((mint, amount)) = inferred {
            quote_deltas.insert(mint, if *target_delta < 0.0 { -amount } else { amount });
        }
    }

    let mut candidates = owners
        .into_iter()
        .filter_map(|(owner, (target_delta, quote_deltas))| {
            if target_delta.abs() <= f64::EPSILON {
                return None;
            }
            let quote_delta = quote_deltas.values().sum::<f64>();
            let same_direction =
                quote_delta.abs() <= f64::EPSILON || target_delta.signum() == quote_delta.signum();
            if !same_direction {
                return None;
            }
            let (quote_mint, selected_quote_delta) = quote_deltas
                .into_iter()
                .max_by(|(_, left), (_, right)| left.abs().total_cmp(&right.abs()))
                .map(|(mint, delta)| (Some(mint), Some(delta)))
                .unwrap_or((None, None));
            Some((
                owner,
                target_delta,
                quote_mint,
                selected_quote_delta,
                target_delta.abs() + quote_delta.abs(),
            ))
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Value::Null;
    }

    let signer = first_signer(transaction);
    let selected_index = signer
        .as_ref()
        .and_then(|signer| candidates.iter().position(|item| item.0 == *signer))
        .or_else(|| {
            candidates
                .iter()
                .enumerate()
                .min_by(|(_, left), (_, right)| left.4.total_cmp(&right.4))
                .map(|(index, _)| index)
        })
        .unwrap_or(0);
    let (owner, target_delta, quote_mint, quote_delta, _) = candidates.swap_remove(selected_index);
    json!({
        "detected": true,
        "direction": if target_delta < 0.0 { "deposit" } else { "withdrawal" },
        "owner": owner,
        "confidence": if signer.as_deref() == Some(owner.as_str()) { "signer_owner" } else { "balance_direction" },
        "target_mint": target_token,
        "target_amount_ui": target_delta.abs(),
        "quote_mint": quote_mint,
        "quote_amount_ui": quote_delta.map(f64::abs),
    })
}

fn classify_log_event_kind(logs: Option<&Value>) -> Option<&'static str> {
    if log_contains(logs, "addliquidity")
        || log_contains(logs, "add_liquidity")
        || log_contains(logs, "increaseliquidity")
        || log_contains(logs, "increase_liquidity")
    {
        Some("add_liquidity")
    } else if log_contains(logs, "removeliquidity")
        || log_contains(logs, "remove_liquidity")
        || log_contains(logs, "decreaseliquidity")
        || log_contains(logs, "decrease_liquidity")
    {
        Some("remove_liquidity")
    } else if log_contains(logs, "swap") {
        Some("swap")
    } else {
        None
    }
}

fn log_contains(logs: Option<&Value>, needle: &str) -> bool {
    logs.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|log| log.to_ascii_lowercase().contains(needle))
}

#[derive(Clone)]
struct TokenBalance {
    account_index: i64,
    mint: String,
    owner: Option<String>,
    amount: u128,
    decimals: u32,
}

fn extract_token_changes(meta: &Value) -> Vec<Value> {
    let pre = token_balances(meta.get("preTokenBalances"));
    let post = token_balances(meta.get("postTokenBalances"));
    let mut keys = pre.keys().cloned().collect::<HashSet<_>>();
    keys.extend(post.keys().cloned());
    let mut changes = keys
        .into_iter()
        .filter_map(|key| {
            let before = pre.get(&key);
            let after = post.get(&key);
            let before_amount = before.map(|item| item.amount).unwrap_or_default();
            let after_amount = after.map(|item| item.amount).unwrap_or_default();
            if before_amount == after_amount {
                return None;
            }
            let item = after.or(before)?;
            let delta_amount = if after_amount >= before_amount {
                format!("+{}", after_amount - before_amount)
            } else {
                format!("-{}", before_amount - after_amount)
            };
            let scale = 10_f64.powi(item.decimals.min(i32::MAX as u32) as i32);
            let delta_ui_amount = if scale.is_finite() && scale > 0.0 {
                (after_amount as f64 - before_amount as f64) / scale
            } else {
                0.0
            };
            Some(json!({
                "account_index": item.account_index,
                "mint": item.mint,
                "owner": item.owner,
                "decimals": item.decimals,
                "pre_amount": before_amount.to_string(),
                "post_amount": after_amount.to_string(),
                "delta_amount": delta_amount,
                "delta_ui_amount": delta_ui_amount,
            }))
        })
        .collect::<Vec<_>>();
    changes.sort_by(|left, right| {
        left.get("mint")
            .and_then(Value::as_str)
            .cmp(&right.get("mint").and_then(Value::as_str))
            .then_with(|| {
                left.get("account_index")
                    .and_then(Value::as_i64)
                    .cmp(&right.get("account_index").and_then(Value::as_i64))
            })
    });
    changes
}

#[derive(Default)]
struct OwnerFlow {
    target_delta: f64,
    quote_deltas: HashMap<String, f64>,
}

#[derive(Default)]
struct TransferFlow {
    target: Vec<(usize, f64)>,
    quotes: HashMap<String, Vec<(usize, f64)>>,
}

fn classify_trade(
    transaction: &Value,
    token_changes: &[Value],
    token_instructions: &[Value],
    target_token: Option<&str>,
    quote_mints: &HashSet<String>,
    whale_usd: f64,
    sol_price_usd: Option<f64>,
    excluded_owners: &HashSet<String>,
) -> Value {
    let failed = transaction
        .get("meta")
        .and_then(|meta| meta.get("err"))
        .or_else(|| transaction.get("err"))
        .is_some_and(|error| !error.is_null());
    if failed {
        return json!({
            "side": "unknown",
            "is_whale": false,
            "trader": first_signer(transaction),
            "target_mint": target_token,
            "reason": "transaction failed; no settled trade",
        });
    }
    let Some(target_token) = target_token else {
        return json!({
            "side": "unknown",
            "is_whale": false,
            "reason": "SOLANA_LISTENER_TOKEN is not configured",
        });
    };
    let fallback_trader = first_signer(transaction);
    let target_token = target_token.to_ascii_lowercase();
    let mut owner_flows = HashMap::<String, OwnerFlow>::new();
    let mut pool_flows = HashMap::<String, OwnerFlow>::new();
    let mut token_account_mints = token_account_mints(transaction);
    // A temporary token account may be absent from pre/postTokenBalances but
    // still appears in the parsed transfer chain. Propagate a known mint
    // across every explicit transfer so a following legacy `transfer` can be
    // decoded as well.
    for instruction in token_instructions {
        let Some(info) = instruction.get("info") else {
            continue;
        };
        let Some(mint) = info.get("mint").and_then(Value::as_str) else {
            continue;
        };
        for field in ["source", "destination"] {
            if let Some(account) = info.get(field).and_then(Value::as_str) {
                token_account_mints
                    .entry(account.to_string())
                    .or_insert_with(|| mint.to_string());
            }
        }
    }

    // The transaction signer is often an aggregator/router. The wallet that
    // actually bought or sold the token is the owner whose target and quote
    // balances move in opposite directions, so classify by owner instead.
    for change in token_changes {
        let Some(owner) = change.get("owner").and_then(Value::as_str) else {
            continue;
        };
        let Some(mint) = change.get("mint").and_then(Value::as_str) else {
            continue;
        };
        let delta = change
            .get("delta_ui_amount")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let is_excluded_owner = excluded_owners.contains(&owner.to_ascii_lowercase());
        let flows = if is_excluded_owner {
            &mut pool_flows
        } else {
            &mut owner_flows
        };
        let flow = flows.entry(owner.to_string()).or_default();
        if mint.to_ascii_lowercase() == target_token {
            flow.target_delta += delta;
        } else if quote_mints.contains(&mint.to_ascii_lowercase()) {
            *flow.quote_deltas.entry(mint.to_string()).or_default() += delta;
        }
        // Keep a second view for the final pool-balance fallback. The pool
        // owner is not always the pool address reported by the catalog (some
        // protocols use vault authorities), so relying only on the watched
        // address would leave valid settled swaps unresolved.
        if !is_excluded_owner {
            let flow = pool_flows.entry(owner.to_string()).or_default();
            if mint.to_ascii_lowercase() == target_token {
                flow.target_delta += delta;
            } else if quote_mints.contains(&mint.to_ascii_lowercase()) {
                *flow.quote_deltas.entry(mint.to_string()).or_default() += delta;
            }
        }
    }

    // Routed swaps can wrap SOL into a temporary account and close it before
    // the transaction finishes. In that case the quote-side owner balance is
    // absent from pre/postTokenBalances, while transferChecked still carries
    // the exact amount authorised by the wallet. Use that transfer only when
    // the normal owner balance path has no quote leg.
    for (owner, flow) in &mut owner_flows {
        if flow.target_delta.abs() <= f64::EPSILON || !flow.quote_deltas.is_empty() {
            continue;
        }
        let route_like_target_leg = flow.target_delta > 0.0
            && token_instructions.iter().any(|instruction| {
                if !matches!(
                    instruction.get("type").and_then(Value::as_str),
                    Some("transfer" | "transferChecked")
                ) {
                    return false;
                }
                let Some(info) = instruction.get("info") else {
                    return false;
                };
                if !info
                    .get("authority")
                    .and_then(Value::as_str)
                    .is_some_and(|authority| authority.eq_ignore_ascii_case(owner))
                    || !info
                        .get("mint")
                        .and_then(Value::as_str)
                        .is_some_and(|mint| mint.eq_ignore_ascii_case(&target_token))
                {
                    return false;
                }
                let amount = info
                    .get("tokenAmount")
                    .and_then(|value| value.get("uiAmount"))
                    .and_then(Value::as_f64)
                    .unwrap_or_default();
                amount > flow.target_delta.abs() * 1.5
            });
        if route_like_target_leg {
            continue;
        }
        let inferred = token_instructions
            .iter()
            .filter_map(|instruction| {
                let kind = instruction.get("type").and_then(Value::as_str)?;
                if !matches!(kind, "transfer" | "transferChecked") {
                    return None;
                }
                let info = instruction.get("info")?;
                let authority = info.get("authority").and_then(Value::as_str)?;
                if !authority.eq_ignore_ascii_case(owner) {
                    return None;
                }
                let mint = info.get("mint").and_then(Value::as_str)?;
                if !quote_mints.contains(&mint.to_ascii_lowercase()) {
                    return None;
                }
                let amount = info
                    .get("tokenAmount")
                    .and_then(|value| value.get("uiAmount"))
                    .and_then(Value::as_f64)
                    .or_else(|| {
                        let token_amount = info.get("tokenAmount")?;
                        let raw = token_amount.get("amount")?.as_str()?.parse::<f64>().ok()?;
                        let decimals = token_amount
                            .get("decimals")
                            .and_then(Value::as_u64)
                            .unwrap_or_default();
                        Some(raw / 10f64.powi(decimals as i32))
                    })
                    .or_else(|| {
                        let raw = info.get("amount")?.as_str()?.parse::<f64>().ok()?;
                        let decimals = token_changes
                            .iter()
                            .find(|change| {
                                change
                                    .get("mint")
                                    .and_then(Value::as_str)
                                    .is_some_and(|value| value.eq_ignore_ascii_case(mint))
                            })
                            .and_then(|change| change.get("decimals"))
                            .and_then(Value::as_u64)
                            .unwrap_or_default();
                        Some(raw / 10f64.powi(decimals as i32))
                    })
                    .filter(|value| value.is_finite() && *value > 0.0)?;
                Some((mint.to_string(), amount))
            })
            .max_by(|left, right| left.1.total_cmp(&right.1));
        if let Some((mint, amount)) = inferred {
            flow.quote_deltas.insert(
                mint,
                if flow.target_delta > 0.0 {
                    -amount
                } else {
                    amount
                },
            );
        }
    }

    // Aggregators and protocol vaults frequently close temporary token
    // accounts before the transaction finishes. In that shape there is no
    // owner balance delta to pair, but the parsed transfer instructions still
    // contain both legs. Use the common transfer authority and instruction
    // order as a conservative fallback: the first leg is the input, the
    // second leg is the output. Max-per-mint avoids counting the same amount
    // twice when a temporary account is forwarded to its final account.
    let mut transfer_flows = HashMap::<String, TransferFlow>::new();
    for (index, instruction) in token_instructions.iter().enumerate() {
        if !matches!(
            instruction.get("type").and_then(Value::as_str),
            Some("transfer" | "transferChecked")
        ) {
            continue;
        }
        let Some(info) = instruction.get("info") else {
            continue;
        };
        let Some(authority) = info.get("authority").and_then(Value::as_str) else {
            continue;
        };
        let Some(mint) = info
            .get("mint")
            .and_then(Value::as_str)
            .or_else(|| {
                info.get("source")
                    .and_then(Value::as_str)
                    .and_then(|account| token_account_mints.get(account))
                    .map(String::as_str)
            })
            .or_else(|| {
                info.get("destination")
                    .and_then(Value::as_str)
                    .and_then(|account| token_account_mints.get(account))
                    .map(String::as_str)
            })
        else {
            continue;
        };
        let Some(amount) = transfer_amount_ui(info, mint, token_changes) else {
            continue;
        };
        if amount <= f64::EPSILON {
            continue;
        }
        let flow = transfer_flows.entry(authority.to_string()).or_default();
        if mint.eq_ignore_ascii_case(&target_token) {
            flow.target.push((index, amount));
        } else if quote_mints.contains(&mint.to_ascii_lowercase()) {
            flow.quotes
                .entry(mint.to_string())
                .or_default()
                .push((index, amount));
        }
    }
    for (authority, transfer_flow) in transfer_flows {
        if excluded_owners.contains(&authority.to_ascii_lowercase())
            || transfer_flow.target.is_empty()
            || transfer_flow.quotes.is_empty()
        {
            continue;
        }
        let Some((target_index, target_amount)) = transfer_flow
            .target
            .iter()
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .copied()
        else {
            continue;
        };
        let Some((quote_mint, quote_transfers)) =
            transfer_flow.quotes.iter().max_by(|left, right| {
                left.1
                    .iter()
                    .map(|(_, amount)| *amount)
                    .fold(0.0, f64::max)
                    .total_cmp(
                        &right
                            .1
                            .iter()
                            .map(|(_, amount)| *amount)
                            .fold(0.0, f64::max),
                    )
            })
        else {
            continue;
        };
        let Some((quote_index, quote_amount)) = quote_transfers
            .iter()
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .copied()
        else {
            continue;
        };
        if target_index == quote_index {
            continue;
        }
        // Target sent first means sell; quote sent first means buy.
        let target_sign = if target_index < quote_index {
            -1.0
        } else {
            1.0
        };
        let flow = owner_flows.entry(authority).or_default();
        if flow.target_delta.abs() <= f64::EPSILON && flow.quote_deltas.is_empty() {
            flow.target_delta = target_sign * target_amount;
            flow.quote_deltas
                .insert(quote_mint.clone(), -target_sign * quote_amount);
        }
    }

    let mut selected: Option<(
        String,
        f64,
        HashMap<String, f64>,
        f64,
        Option<(String, f64, f64)>,
        String,
    )> = None;
    for (owner, flow) in owner_flows {
        let quote_delta = flow.quote_deltas.values().sum::<f64>();
        let side = if flow.target_delta > 0.0 && quote_delta < 0.0 {
            "buy"
        } else if flow.target_delta < 0.0 && quote_delta > 0.0 {
            "sell"
        } else {
            continue;
        };

        let mut known_quote_usd = 0.0_f64;
        let mut best_quote_trade: Option<(String, f64, f64)> = None;
        for (mint, delta) in &flow.quote_deltas {
            let mint_lower = mint.to_ascii_lowercase();
            let unit_usd = if mint_lower == WSOL_MINT.to_ascii_lowercase() {
                sol_price_usd
            } else if mint_lower == DEFAULT_QUOTE_MINTS[1].to_ascii_lowercase()
                || mint_lower == DEFAULT_QUOTE_MINTS[2].to_ascii_lowercase()
                || quote_mints.contains(&mint_lower)
                    && (mint_lower == "usdc" || mint_lower == "usdt")
            {
                Some(1.0)
            } else {
                None
            };
            if let Some(unit_usd) = unit_usd {
                let amount_usd = delta.abs() * unit_usd;
                known_quote_usd += amount_usd;
                if best_quote_trade
                    .as_ref()
                    .map(|(_, _, amount)| amount_usd > *amount)
                    .unwrap_or(true)
                {
                    best_quote_trade = Some((mint.clone(), *delta, amount_usd));
                }
            }
        }

        let score = known_quote_usd.max(
            flow.quote_deltas
                .values()
                .map(|value| value.abs())
                .sum::<f64>(),
        );
        if selected
            .as_ref()
            .map(|item| item.3 >= score)
            .unwrap_or(false)
        {
            continue;
        }
        selected = Some((
            owner,
            flow.target_delta,
            flow.quote_deltas,
            known_quote_usd,
            best_quote_trade,
            side.to_string(),
        ));
    }

    // When the user leg is represented by separate owners (common with
    // routed Raydium transactions), the pool-side balances still expose the
    // settled pair. A pool receiving target and sending quote is a sell; the
    // inverse is a buy. This is only considered after user/transfer-authority
    // attribution so normal wallet attribution always wins.
    if selected.is_none() {
        for (owner, flow) in pool_flows {
            let quote_delta = flow.quote_deltas.values().sum::<f64>();
            let side = if flow.target_delta < 0.0 && quote_delta > 0.0 {
                "buy"
            } else if flow.target_delta > 0.0 && quote_delta < 0.0 {
                "sell"
            } else {
                continue;
            };
            let mut known_quote_usd = 0.0_f64;
            let mut best_quote_trade: Option<(String, f64, f64)> = None;
            for (mint, delta) in &flow.quote_deltas {
                let mint_lower = mint.to_ascii_lowercase();
                let unit_usd = if mint_lower == WSOL_MINT.to_ascii_lowercase() {
                    sol_price_usd
                } else if mint_lower == DEFAULT_QUOTE_MINTS[1].to_ascii_lowercase()
                    || mint_lower == DEFAULT_QUOTE_MINTS[2].to_ascii_lowercase()
                    || quote_mints.contains(&mint_lower)
                        && (mint_lower == "usdc" || mint_lower == "usdt")
                {
                    Some(1.0)
                } else {
                    None
                };
                if let Some(unit_usd) = unit_usd {
                    let amount_usd = delta.abs() * unit_usd;
                    known_quote_usd += amount_usd;
                    if best_quote_trade
                        .as_ref()
                        .map(|(_, _, amount)| amount_usd > *amount)
                        .unwrap_or(true)
                    {
                        best_quote_trade = Some((mint.clone(), *delta, amount_usd));
                    }
                }
            }
            let target_amount = flow.target_delta.abs();
            let quote_amount = best_quote_trade
                .as_ref()
                .map(|(_, delta, _)| delta.abs())
                .unwrap_or_else(|| quote_delta.abs());
            let swap_ratio_quote_per_target =
                (target_amount > 0.0 && quote_amount > 0.0).then_some(quote_amount / target_amount);
            let swap_price_usd = (target_amount > 0.0 && known_quote_usd > 0.0)
                .then_some(known_quote_usd / target_amount);
            let is_whale = known_quote_usd >= whale_usd;
            return json!({
                "side": side,
                "action": if side == "buy" { "bought" } else { "sold" },
                "trader": fallback_trader,
                "target_mint": target_token,
                "target_delta_ui_amount": if side == "buy" { target_amount } else { -target_amount },
                "quote_delta_ui_amount": if side == "buy" { -quote_amount } else { quote_amount },
                "quote_mint": best_quote_trade.as_ref().map(|(mint, _, _)| mint),
                "target_amount_ui": target_amount,
                "quote_amount_ui": quote_amount,
                "swap_ratio_quote_per_target": swap_ratio_quote_per_target,
                "swap_price_usd": swap_price_usd,
                "notional_usd": if known_quote_usd > 0.0 { Some(known_quote_usd) } else { None::<f64> },
                "whale_threshold_usd": whale_usd,
                "sol_price_usd": sol_price_usd,
                "is_whale": is_whale,
                "alert_type": if is_whale {
                    Some(if side == "buy" { "whale_buy" } else { "whale_sell" })
                } else {
                    None::<&str>
                },
                "attribution": "pool_balance_pair",
                "pool_owner": owner,
            });
        }
    }

    let Some((trader, target_delta, quote_deltas, known_quote_usd, best_quote_trade, side)) =
        selected
    else {
        return json!({
            "side": "unknown",
            "is_whale": false,
            "trader": fallback_trader,
            "target_mint": target_token,
            "reason": "no single token owner had opposite target and quote balance changes",
        });
    };

    let quote_delta = quote_deltas.values().sum::<f64>();
    let selected_quote_delta = best_quote_trade
        .as_ref()
        .map(|(_, delta, _)| *delta)
        .unwrap_or(quote_delta);
    let target_amount = target_delta.abs();
    let quote_amount = selected_quote_delta.abs();
    let swap_ratio_quote_per_target =
        (target_amount > 0.0 && quote_amount > 0.0).then_some(quote_amount / target_amount);
    let swap_price_usd =
        (target_amount > 0.0 && known_quote_usd > 0.0).then_some(known_quote_usd / target_amount);
    let is_whale = known_quote_usd >= whale_usd;
    json!({
        "side": side,
        "action": if side == "buy" { "bought" } else { "sold" },
        "trader": trader,
        "target_mint": target_token,
        "target_delta_ui_amount": target_delta,
        "quote_delta_ui_amount": quote_delta,
        "quote_mint": best_quote_trade.as_ref().map(|(mint, _, _)| mint),
        "target_amount_ui": target_amount,
        "quote_amount_ui": quote_amount,
        "swap_ratio_quote_per_target": swap_ratio_quote_per_target,
        "swap_price_usd": swap_price_usd,
        "notional_usd": if known_quote_usd > 0.0 { Some(known_quote_usd) } else { None::<f64> },
        "whale_threshold_usd": whale_usd,
        "sol_price_usd": sol_price_usd,
        "is_whale": is_whale,
        "alert_type": if is_whale {
            Some(if side == "buy" { "whale_buy" } else { "whale_sell" })
        } else {
            None::<&str>
        },
    })
}

fn transfer_amount_ui(info: &Value, mint: &str, token_changes: &[Value]) -> Option<f64> {
    if let Some(amount) = info
        .get("tokenAmount")
        .and_then(|value| value.get("uiAmount"))
        .and_then(Value::as_f64)
    {
        return Some(amount);
    }
    let raw = info
        .get("tokenAmount")
        .and_then(|value| value.get("amount"))
        .and_then(Value::as_str)
        .or_else(|| info.get("amount").and_then(Value::as_str))?
        .parse::<f64>()
        .ok()?;
    let decimals = info
        .get("tokenAmount")
        .and_then(|value| value.get("decimals"))
        .and_then(Value::as_u64)
        .or_else(|| {
            token_changes.iter().find_map(|change| {
                change
                    .get("mint")
                    .and_then(Value::as_str)
                    .filter(|value| value.eq_ignore_ascii_case(mint))
                    .and_then(|_| change.get("decimals"))
                    .and_then(Value::as_u64)
            })
        })
        .or_else(|| {
            if mint.eq_ignore_ascii_case(WSOL_MINT) {
                Some(9)
            } else if mint.eq_ignore_ascii_case(DEFAULT_QUOTE_MINTS[1])
                || mint.eq_ignore_ascii_case(DEFAULT_QUOTE_MINTS[2])
            {
                Some(6)
            } else {
                None
            }
        })?;
    Some(raw / 10f64.powi(decimals as i32))
}

fn first_signer(transaction: &Value) -> Option<String> {
    transaction
        .pointer("/transaction/message/accountKeys")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|key| key.get("signer").and_then(Value::as_bool) == Some(true))
        .and_then(|key| {
            key.as_str()
                .or_else(|| key.get("pubkey").and_then(Value::as_str))
        })
        .map(ToOwned::to_owned)
}

fn token_balances(value: Option<&Value>) -> HashMap<(i64, String), TokenBalance> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|balance| {
            let account_index = balance.get("accountIndex")?.as_i64()?;
            let mint = balance.get("mint")?.as_str()?.to_string();
            let token_amount = balance.get("uiTokenAmount")?;
            let amount = token_amount.get("amount")?.as_str()?.parse().ok()?;
            let decimals = token_amount.get("decimals")?.as_u64()?.try_into().ok()?;
            Some((
                (account_index, mint.clone()),
                TokenBalance {
                    account_index,
                    mint,
                    owner: balance
                        .get("owner")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    amount,
                    decimals,
                },
            ))
        })
        .collect()
}

fn account_keys(transaction: &Value) -> Vec<String> {
    transaction
        .pointer("/transaction/message/accountKeys")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|key| {
            key.as_str()
                .or_else(|| key.get("pubkey").and_then(Value::as_str))
                .map(ToOwned::to_owned)
        })
        .collect()
}

fn token_account_mints(transaction: &Value) -> HashMap<String, String> {
    if let Some(saved) = transaction
        .get("token_account_mints")
        .and_then(Value::as_object)
    {
        return saved
            .iter()
            .filter_map(|(account, mint)| Some((account.clone(), mint.as_str()?.to_string())))
            .collect();
    }
    let keys = account_keys(transaction);
    let mut mints = HashMap::new();
    for balances in [
        transaction.pointer("/meta/preTokenBalances"),
        transaction.pointer("/meta/postTokenBalances"),
    ] {
        for balance in balances.and_then(Value::as_array).into_iter().flatten() {
            let Some(index) = balance.get("accountIndex").and_then(Value::as_u64) else {
                continue;
            };
            let Some(account) = keys.get(index as usize) else {
                continue;
            };
            let Some(mint) = balance.get("mint").and_then(Value::as_str) else {
                continue;
            };
            mints.insert(account.clone(), mint.to_string());
        }
    }
    mints
}

fn program_ids(transaction: &Value, account_keys: &[String]) -> Vec<String> {
    let mut programs = HashSet::new();
    let mut collect = |instructions: Option<&Value>| {
        for instruction in instructions.and_then(Value::as_array).into_iter().flatten() {
            if let Some(program) = instruction.get("programId").and_then(Value::as_str) {
                programs.insert(program.to_string());
            } else if let Some(index) = instruction.get("programIdIndex").and_then(Value::as_u64) {
                if let Some(program) = account_keys.get(index as usize) {
                    programs.insert(program.clone());
                }
            } else if let Some(program) = instruction.get("program").and_then(Value::as_str) {
                programs.insert(program.to_string());
            }
        }
    };
    collect(transaction.pointer("/transaction/message/instructions"));
    if let Some(inner) = transaction.pointer("/meta/innerInstructions") {
        for group in inner.as_array().into_iter().flatten() {
            collect(group.get("instructions"));
        }
    }
    if let Some(logs) = transaction
        .pointer("/meta/logMessages")
        .and_then(Value::as_array)
    {
        for log in logs.iter().filter_map(Value::as_str) {
            let Some(program) = log
                .strip_prefix("Program ")
                .and_then(|value| value.split_whitespace().next())
            else {
                continue;
            };
            if program != "log:" && program != "data:" {
                programs.insert(program.to_string());
            }
        }
    }
    let mut programs = programs.into_iter().collect::<Vec<_>>();
    programs.sort();
    programs
}

fn collect_token_instructions(
    instructions: Option<&Value>,
    account_keys: &[String],
    output: &mut Vec<Value>,
    token_mints: &mut HashSet<String>,
) {
    for instruction in instructions.and_then(Value::as_array).into_iter().flatten() {
        let program = instruction
            .get("program")
            .and_then(Value::as_str)
            .or_else(|| instruction.get("programId").and_then(Value::as_str))
            .or_else(|| {
                instruction
                    .get("programIdIndex")
                    .and_then(Value::as_u64)
                    .and_then(|index| account_keys.get(index as usize).map(String::as_str))
            });
        let parsed = instruction.get("parsed");
        let Some(parsed) = parsed else {
            continue;
        };
        let instruction_type = parsed
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let info = parsed.get("info").cloned().unwrap_or(Value::Null);
        let mint = info.get("mint").and_then(Value::as_str);
        let is_token_program = matches!(program, Some("spl-token" | "spl-token-2022"))
            || mint.is_some()
            || matches!(
                instruction_type,
                "transfer"
                    | "transferChecked"
                    | "mintTo"
                    | "mintToChecked"
                    | "burn"
                    | "burnChecked"
            );
        if !is_token_program {
            continue;
        }
        if let Some(mint) = mint {
            token_mints.insert(mint.to_string());
        }
        output.push(json!({
            "program": program,
            "type": instruction_type,
            "info": info,
        }));
    }
}

fn notification_signature(event: &Value) -> Option<String> {
    event
        .pointer("/params/result/value/signature")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn env_bool(name: &str) -> bool {
    matches!(
        env::var(name)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn to_ws_url(value: &str) -> Result<String> {
    let value = value.trim();
    if value.starts_with("wss://") || value.starts_with("ws://") {
        return Ok(value.to_string());
    }
    if let Some(rest) = value.strip_prefix("https://") {
        return Ok(format!("wss://{rest}"));
    }
    if let Some(rest) = value.strip_prefix("http://") {
        return Ok(format!("ws://{rest}"));
    }
    bail!("unsupported Solana RPC URL scheme: {value}")
}

fn display_url(value: &str) -> String {
    let without_query = value.split_once('?').map_or(value, |(prefix, _)| prefix);
    let Some((scheme, rest)) = without_query.split_once("://") else {
        return "[redacted]".to_string();
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if path.is_empty() {
        format!("{scheme}://{authority}")
    } else {
        format!("{scheme}://{authority}/[redacted]")
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or_default()
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct EventQuery {
    pub(crate) token: Option<String>,
    pub(crate) side: Option<String>,
    pub(crate) min_usd: Option<f64>,
    pub(crate) limit: Option<usize>,
    pub(crate) whale_only: Option<bool>,
}

pub(crate) async fn events(Query(query): Query<EventQuery>) -> Json<Value> {
    Json(read_event_file(&query, false).await)
}

pub(crate) async fn whale_alerts(Query(mut query): Query<EventQuery>) -> Json<Value> {
    query.whale_only = Some(true);
    let events = read_event_file(&query, true).await;
    let alerts = events
        .get("events")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    Json(json!({"success": true, "alerts": alerts}))
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct SnapshotQuery {
    pub(crate) pool: Option<String>,
    pub(crate) limit: Option<usize>,
}

pub(crate) async fn pool_snapshots(Query(query): Query<SnapshotQuery>) -> Json<Value> {
    let path = env::var_os("SOLANA_LISTENER_REALTIME_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_REALTIME_DB));
    let source = path.clone();
    match tokio::task::spawn_blocking(move || read_pool_snapshots(&path, &query)).await {
        Ok(Ok(snapshots)) => Json(snapshots),
        Ok(Err(error)) => Json(json!({
            "snapshots": [],
            "count": 0,
            "error": error.to_string(),
            "source": source,
            "storage": "sqlite"
        })),
        Err(error) => Json(json!({
            "snapshots": [],
            "count": 0,
            "error": error.to_string(),
            "source": source,
            "storage": "sqlite"
        })),
    }
}

fn read_pool_snapshots(path: &PathBuf, query: &SnapshotQuery) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({
            "snapshots": [],
            "count": 0,
            "source": path,
            "storage": "sqlite"
        }));
    }
    let connection = initialize_realtime_db(path)?;
    let limit = query.limit.unwrap_or(100).clamp(1, 1_000) as i64;
    let mut statement = connection.prepare(
        "SELECT pool_address, refreshed_at_ms, watched_protocol, watched_pool_type,
                event_signature, event_slot, account_slot, available, rpc_endpoint, snapshot_json
         FROM solana_pool_latest
         WHERE (?1 IS NULL OR lower(pool_address) = lower(?1))
         ORDER BY refreshed_at_ms DESC
         LIMIT ?2",
    )?;
    let rows = statement
        .query_map(params![query.pool.as_deref(), limit], |row| {
            let snapshot_json: String = row.get(9)?;
            let snapshot = serde_json::from_str::<Value>(&snapshot_json).unwrap_or(Value::Null);
            Ok(json!({
                "pool_address": row.get::<_, String>(0)?,
                "refreshed_at_ms": row.get::<_, i64>(1)?,
                "watched_protocol": row.get::<_, Option<String>>(2)?,
                "watched_pool_type": row.get::<_, Option<String>>(3)?,
                "event_signature": row.get::<_, Option<String>>(4)?,
                "event_slot": row.get::<_, Option<i64>>(5)?,
                "account_slot": row.get::<_, Option<i64>>(6)?,
                "available": row.get::<_, i64>(7)? != 0,
                "rpc_endpoint": row.get::<_, Option<String>>(8)?,
                "snapshot": snapshot,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let count = rows.len();
    Ok(json!({
        "snapshots": rows,
        "count": count,
        "source": path,
        "storage": "sqlite"
    }))
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct PoolHistoryQuery {
    pub(crate) pool: Option<String>,
    pub(crate) target_mint: Option<String>,
    pub(crate) hours: Option<u64>,
    pub(crate) resolution: Option<String>,
    pub(crate) limit: Option<usize>,
}

pub(crate) async fn pool_history(Query(query): Query<PoolHistoryQuery>) -> Json<Value> {
    let path = env::var_os("SOLANA_LISTENER_EVENT_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_EVENT_DB));
    let source = path.clone();
    match tokio::task::spawn_blocking(move || read_pool_history(&path, &query)).await {
        Ok(Ok(history)) => Json(history),
        Ok(Err(error)) => Json(json!({
            "trades": [], "candles": [], "error": error.to_string(),
            "source": source, "storage": "sqlite"
        })),
        Err(error) => Json(json!({
            "trades": [], "candles": [], "error": error.to_string(),
            "source": source, "storage": "sqlite"
        })),
    }
}

struct CandleAggregate {
    open_timestamp_ms: i64,
    close_timestamp_ms: i64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume_usd: f64,
    trades: usize,
}

fn record_candle_observation(
    candles: &mut BTreeMap<i64, CandleAggregate>,
    timestamp_ms: i64,
    bucket_ms: i64,
    price: f64,
    volume_usd: f64,
    is_trade: bool,
) {
    let bucket = timestamp_ms.div_euclid(bucket_ms) * bucket_ms;
    let candle = candles.entry(bucket).or_insert_with(|| CandleAggregate {
        open_timestamp_ms: timestamp_ms,
        close_timestamp_ms: timestamp_ms,
        open: price,
        high: price,
        low: price,
        close: price,
        volume_usd: 0.0,
        trades: 0,
    });
    candle.high = candle.high.max(price);
    candle.low = candle.low.min(price);
    if timestamp_ms < candle.open_timestamp_ms {
        candle.open_timestamp_ms = timestamp_ms;
        candle.open = price;
    }
    if timestamp_ms >= candle.close_timestamp_ms {
        candle.close_timestamp_ms = timestamp_ms;
        candle.close = price;
    }
    if is_trade {
        candle.volume_usd += volume_usd;
        candle.trades += 1;
    }
}

fn append_snapshot_candles(
    candles: &mut BTreeMap<i64, CandleAggregate>,
    pool: &str,
    since: i64,
    bucket_ms: i64,
    target_mint: Option<&str>,
) {
    let path = env::var_os("SOLANA_LISTENER_REALTIME_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_REALTIME_DB));
    let Ok(connection) = Connection::open(path) else {
        return;
    };
    let Ok(mut statement) = connection.prepare(
        "SELECT refreshed_at_ms, snapshot_json
         FROM solana_pool_snapshots
         WHERE lower(pool_address) = lower(?1) AND refreshed_at_ms >= ?2
         ORDER BY refreshed_at_ms ASC",
    ) else {
        return;
    };
    let Ok(rows) = statement.query_map(params![pool, since], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    }) else {
        return;
    };
    for row in rows.flatten() {
        let Ok(snapshot) = serde_json::from_str::<Value>(&row.1) else {
            continue;
        };
        let Some(decoded) = snapshot.get("decoded_price") else {
            continue;
        };
        let target = target_mint.map(str::to_ascii_lowercase);
        let price_key = target.as_deref().and_then(|target| {
            if decoded
                .get("token0_address")
                .and_then(Value::as_str)
                .is_some_and(|address| address.eq_ignore_ascii_case(target))
            {
                Some("token0_price_usd")
            } else if decoded
                .get("token1_address")
                .and_then(Value::as_str)
                .is_some_and(|address| address.eq_ignore_ascii_case(target))
            {
                Some("token1_price_usd")
            } else {
                None
            }
        });
        let price = if target_mint.is_some() {
            price_key.and_then(|key| decoded.get(key).and_then(Value::as_f64))
        } else {
            decoded.get("token0_price_usd").and_then(Value::as_f64)
        }
        .filter(|value| value.is_finite() && *value > 0.0);
        let Some(price) = price else {
            continue;
        };
        record_candle_observation(candles, row.0, bucket_ms, price, 0.0, false);
    }
}

fn read_pool_history(path: &PathBuf, query: &PoolHistoryQuery) -> Result<Value> {
    let Some(pool) = query
        .pool
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(json!({"trades": [], "candles": [], "summary": {"trades": 0}}));
    };
    if !path.exists() {
        return Ok(json!({
            "pool": pool, "trades": [], "candles": [],
            "summary": {"trades": 0, "volume_usd": 0.0},
            "source": path, "storage": "sqlite"
        }));
    }
    let connection = initialize_event_db(path)?;
    let hours = query.hours.unwrap_or(24).clamp(1, 168);
    let since = now_ms().saturating_sub(hours as u128 * 3_600_000) as i64;
    let limit = query.limit.unwrap_or(2_000).clamp(1, 5_000) as i64;
    let mut statement = connection.prepare(
        "SELECT observed_at_ms, signature, watched_protocol, watched_pool_type, slot, trader,
                target_mint, side, target_amount_ui, quote_mint, quote_amount_ui,
                swap_ratio_quote_per_target, swap_price_usd, notional_usd,
                is_whale, trade_json
         FROM solana_pool_trades
         WHERE lower(watched_pool) = lower(?1)
           AND (?2 IS NULL OR lower(target_mint) = lower(?2))
           AND observed_at_ms >= ?3
         ORDER BY observed_at_ms DESC, id DESC LIMIT ?4",
    )?;
    let mut rows = statement
        .query_map(params![pool, query.target_mint.as_deref(), since, limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, f64>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<f64>>(10)?,
                row.get::<_, Option<f64>>(11)?,
                row.get::<_, Option<f64>>(12)?,
                row.get::<_, Option<f64>>(13)?,
                row.get::<_, i64>(14)? != 0,
                row.get::<_, String>(15)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.reverse();

    let bucket_ms = match query.resolution.as_deref() {
        Some("1m") => 60 * 1_000,
        Some("3m") => 3 * 60 * 1_000,
        Some("5m") => 5 * 60 * 1_000,
        Some("15m") => 15 * 60 * 1_000,
        Some("30m") => 30 * 60 * 1_000,
        Some("1h") => 60 * 60 * 1_000,
        _ if hours <= 24 => 15 * 60 * 1_000,
        _ => 60 * 60 * 1_000,
    };
    let mut candles = BTreeMap::<i64, CandleAggregate>::new();
    append_snapshot_candles(
        &mut candles,
        pool,
        since,
        bucket_ms,
        query.target_mint.as_deref(),
    );
    let mut trades = Vec::with_capacity(rows.len());
    let mut volume_usd = 0.0;
    let mut buys = 0usize;
    let mut sells = 0usize;
    let mut whales = 0usize;
    for (
        observed_at_ms,
        signature,
        watched_protocol,
        watched_pool_type,
        slot,
        trader,
        target_mint,
        side,
        target_amount_ui,
        quote_mint,
        quote_amount_ui,
        ratio,
        price,
        notional_usd,
        is_whale,
        trade_json,
    ) in rows
    {
        if side == "buy" {
            buys += 1;
        } else if side == "sell" {
            sells += 1;
        }
        if is_whale {
            whales += 1;
        }
        if let Some(value) = notional_usd {
            volume_usd += value;
        }
        if let Some(price) = price.filter(|value| value.is_finite() && *value > 0.0) {
            record_candle_observation(
                &mut candles,
                observed_at_ms,
                bucket_ms,
                price,
                notional_usd.unwrap_or_default(),
                true,
            );
        }
        trades.push(json!({
            "observed_at_ms": observed_at_ms, "signature": signature,
            "watched_pool": pool, "watched_protocol": watched_protocol, "slot": slot,
            "watched_pool_type": watched_pool_type,
            "trader": trader, "target_mint": target_mint, "side": side,
            "target_amount_ui": target_amount_ui, "quote_mint": quote_mint,
            "quote_amount_ui": quote_amount_ui, "swap_ratio_quote_per_target": ratio,
            "swap_price_usd": price, "notional_usd": notional_usd,
            "is_whale": is_whale,
            "trade": serde_json::from_str::<Value>(&trade_json).unwrap_or(Value::Null),
        }));
    }
    let candles = candles
        .into_iter()
        .map(|(timestamp, candle)| {
            json!({
                "timestamp": timestamp, "open": candle.open, "high": candle.high,
                "low": candle.low, "close": candle.close,
                "volume_usd": candle.volume_usd, "trades": candle.trades,
            })
        })
        .collect::<Vec<_>>();
    let latest = trades.last().cloned().unwrap_or(Value::Null);
    let first_price = candles
        .first()
        .and_then(|c| c.get("open"))
        .and_then(Value::as_f64);
    let latest_price = candles
        .last()
        .and_then(|c| c.get("close"))
        .and_then(Value::as_f64);
    let change_pct = match (first_price, latest_price) {
        (Some(first), Some(last)) if first > 0.0 => Some((last / first - 1.0) * 100.0),
        _ => None,
    };
    let high_price = candles
        .iter()
        .filter_map(|c| c.get("high").and_then(Value::as_f64))
        .reduce(f64::max);
    let low_price = candles
        .iter()
        .filter_map(|c| c.get("low").and_then(Value::as_f64))
        .reduce(f64::min);
    Ok(json!({
        "pool": pool, "since_ms": since, "hours": hours,
        "trades": trades, "candles": candles, "latest_trade": latest,
        "summary": {
            "trades": buys + sells, "buys": buys, "sells": sells, "whales": whales,
            "volume_usd": volume_usd, "open": first_price,
            "high": high_price, "low": low_price,
            "close": latest_price, "change_pct": change_pct,
        },
        "source": path, "storage": "sqlite"
    }))
}

async fn read_event_file(query: &EventQuery, alerts_only: bool) -> Value {
    let path = if alerts_only || query.whale_only == Some(true) {
        env::var_os("SOLANA_LISTENER_WHALE_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_WHALE_DB))
    } else {
        env::var_os("SOLANA_LISTENER_EVENT_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_EVENT_DB))
    };
    let query = query.clone();
    let source = path.clone();
    match tokio::task::spawn_blocking(move || read_event_db(&path, &query, alerts_only)).await {
        Ok(Ok(events)) => events,
        Ok(Err(error)) => {
            json!({"events": [], "count": 0, "error": error.to_string(), "source": source, "storage": "sqlite"})
        }
        Err(error) => {
            json!({"events": [], "count": 0, "error": error.to_string(), "source": source, "storage": "sqlite"})
        }
    }
}

fn read_event_db(path: &PathBuf, query: &EventQuery, alerts_only: bool) -> Result<Value> {
    let connection = if alerts_only || query.whale_only == Some(true) {
        let whale_db = initialize_whale_db(path)?;
        let configured_whale_path = env::var_os("SOLANA_LISTENER_WHALE_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_WHALE_DB));
        if path == &configured_whale_path {
            let event_path = env::var_os("SOLANA_LISTENER_EVENT_DB")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_EVENT_DB));
            if event_path.exists() {
                let event_db = initialize_event_db(&event_path)?;
                migrate_legacy_whale_alerts(&event_db, &event_path, &whale_db)?;
            }
            let output_path = env::var_os("SOLANA_LISTENER_OUTPUT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT));
            if output_path.exists() {
                restore_whale_alerts_from_log(&whale_db, &output_path)?;
            }
        }
        whale_db
    } else {
        if !path.exists() {
            return Ok(json!({"events": [], "count": 0, "source": path, "storage": "sqlite"}));
        }
        initialize_event_db(path)?
    };
    let source_table = if alerts_only || query.whale_only == Some(true) {
        "solana_whale_alerts"
    } else {
        "solana_events"
    };
    let mut statement = connection.prepare(&format!(
        "SELECT event_json
             FROM {source_table}
             ORDER BY observed_at_ms DESC, id DESC
             LIMIT 5000"
    ))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let token = query.token.as_deref().map(str::to_ascii_lowercase);
    let side = query.side.as_deref().map(str::to_ascii_lowercase);
    let minimum = query.min_usd.unwrap_or_default();
    let limit = query.limit.unwrap_or(100).clamp(1, 1_000);
    let mut selected = Vec::new();

    for event_json in rows {
        let Ok(event) = serde_json::from_str::<Value>(&event_json) else {
            continue;
        };
        if !event_is_decoded_for_ui(&event) {
            continue;
        }
        if alerts_only
            && event
                .pointer("/enrichment/transaction/trade/is_whale")
                .and_then(Value::as_bool)
                != Some(true)
        {
            continue;
        }
        if query.whale_only == Some(true)
            && event
                .pointer("/enrichment/transaction/trade/is_whale")
                .and_then(Value::as_bool)
                != Some(true)
        {
            continue;
        }
        if let Some(token) = token.as_deref() {
            let target_matches = event
                .pointer("/enrichment/transaction/trade/target_mint")
                .and_then(Value::as_str)
                .map(|value| value.eq_ignore_ascii_case(token))
                .unwrap_or(false);
            let mint_matches = event
                .pointer("/enrichment/transaction/token_mints")
                .and_then(Value::as_array)
                .map(|mints| {
                    mints
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|mint| mint.eq_ignore_ascii_case(token))
                })
                .unwrap_or(false);
            if !target_matches && !mint_matches {
                continue;
            }
        }
        if let Some(side) = side.as_deref() {
            if event
                .pointer("/enrichment/transaction/trade/side")
                .and_then(Value::as_str)
                .map(|value| value.to_ascii_lowercase())
                .as_deref()
                != Some(side)
            {
                continue;
            }
        }
        let notional = event
            .pointer("/enrichment/transaction/trade/notional_usd")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        if notional < minimum {
            continue;
        }
        selected.push(event);
        if selected.len() >= limit {
            break;
        }
    }
    selected.reverse();
    let count = selected.len();
    Ok(json!({"events": selected, "count": count, "source": path, "storage": "sqlite"}))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        classify_trade, display_url, extract_token_changes, initialize_event_db,
        initialize_realtime_db, notification_signature, persist_event, persist_pool_snapshot,
        read_event_db, read_pool_snapshots, summarize_transaction, to_ws_url, Config, EventQuery,
        PoolRefreshCoordinator, SeenSignatures, SnapshotQuery, DEFAULT_QUOTE_MINTS,
    };
    use rusqlite::Connection;
    use std::{
        collections::HashSet,
        path::PathBuf,
        sync::{Arc, Mutex},
    };

    fn test_config(target_token: Option<&str>) -> Config {
        Config {
            ws_urls: Vec::new(),
            http_urls: Vec::new(),
            filters: Vec::new(),
            target_token: target_token.map(ToOwned::to_owned),
            quote_mints: DEFAULT_QUOTE_MINTS
                .iter()
                .map(|value| value.to_ascii_lowercase())
                .collect::<HashSet<_>>(),
            whale_usd: 10_000.0,
            sol_price_usd: Some(150.0),
            commitment: "confirmed".to_string(),
            enrich: true,
            output: PathBuf::from("test.jsonl"),
            event_db: PathBuf::from("test.db"),
            whale_db: PathBuf::from("test_whales.db"),
            realtime_db: PathBuf::from("realtime.db"),
            pool_refresh_secs: 15,
        }
    }

    #[test]
    fn classifies_transfer_authority_when_temporary_accounts_hide_balances() {
        let target = "TargetMint";
        let quote = DEFAULT_QUOTE_MINTS[0];
        let instructions = vec![
            json!({"type":"transferChecked","info":{"authority":"Trader111","mint":quote,"tokenAmount":{"uiAmount":0.006}}}),
            json!({"type":"transferChecked","info":{"authority":"Trader111","mint":target,"tokenAmount":{"uiAmount":4.2}}}),
        ];
        let trade = classify_trade(
            &json!({"transaction":{"message":{"accountKeys":[{"pubkey":"Trader111","signer":true}]}}}),
            &[],
            &instructions,
            Some(target),
            &DEFAULT_QUOTE_MINTS
                .iter()
                .map(|value| value.to_ascii_lowercase())
                .collect(),
            10_000.0,
            Some(150.0),
            &HashSet::new(),
        );
        assert_eq!(trade["side"], "buy");
        assert_eq!(trade["target_amount_ui"], 4.2);
        assert_eq!(trade["quote_amount_ui"], 0.006);
    }

    #[test]
    fn classifies_pool_pair_when_user_and_pool_owners_differ() {
        let target = "TargetMint";
        let quote = DEFAULT_QUOTE_MINTS[1];
        let changes = vec![
            json!({"mint":target,"owner":"UserVault","delta_ui_amount":-20.0}),
            json!({"mint":target,"owner":"Pool111","delta_ui_amount":20.0}),
            json!({"mint":quote,"owner":"Pool111","delta_ui_amount":-3.0}),
            json!({"mint":quote,"owner":"UserVault","delta_ui_amount":3.0}),
        ];
        let trade = classify_trade(
            &json!({"transaction":{"message":{"accountKeys":[{"pubkey":"Router111","signer":true}]}}}),
            &changes,
            &[],
            Some(target),
            &DEFAULT_QUOTE_MINTS
                .iter()
                .map(|value| value.to_ascii_lowercase())
                .collect(),
            10_000.0,
            Some(150.0),
            &HashSet::from(["pool111".to_string()]),
        );
        assert_eq!(trade["side"], "sell");
        assert_eq!(trade["target_amount_ui"], 20.0);
        assert_eq!(trade["quote_amount_ui"], 3.0);
    }

    #[test]
    fn converts_http_rpc_urls_to_websocket_urls() {
        assert_eq!(
            to_ws_url("https://mainnet.helius-rpc.com/?api-key=secret").unwrap(),
            "wss://mainnet.helius-rpc.com/?api-key=secret"
        );
        assert_eq!(
            to_ws_url("http://localhost:8899").unwrap(),
            "ws://localhost:8899"
        );
    }

    #[test]
    fn preserves_existing_websocket_urls() {
        assert_eq!(
            to_ws_url("wss://rpc.example/ws").unwrap(),
            "wss://rpc.example/ws"
        );
    }

    #[test]
    fn redacts_query_parameters_in_logs() {
        assert_eq!(
            display_url("wss://rpc.example/ws?api-key=secret"),
            "wss://rpc.example/[redacted]"
        );
    }

    #[test]
    fn redacts_path_credentials_in_logs() {
        assert_eq!(
            display_url("wss://provider.example/secret-token"),
            "wss://provider.example/[redacted]"
        );
    }

    #[test]
    fn extracts_token_balance_deltas_without_floating_point_amounts() {
        let meta = json!({
            "preTokenBalances": [{
                "accountIndex": 3,
                "mint": "Mint111",
                "owner": "Owner111",
                "uiTokenAmount": {"amount": "1000000", "decimals": 6}
            }],
            "postTokenBalances": [{
                "accountIndex": 3,
                "mint": "Mint111",
                "owner": "Owner111",
                "uiTokenAmount": {"amount": "1250000", "decimals": 6}
            }]
        });
        let changes = extract_token_changes(&meta);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0]["delta_amount"], "+250000");
        assert_eq!(changes[0]["delta_ui_amount"], 0.25);
    }

    #[test]
    fn summarizes_programs_mints_and_token_instructions() {
        let transaction = json!({
            "slot": 42,
            "blockTime": 123,
            "meta": {
                "err": null,
                "preTokenBalances": [],
                "postTokenBalances": [],
                "logMessages": [
                    "Program whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc invoke [1]",
                    "Program log: Instruction: SwapV2",
                    "Program whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc success"
                ]
            },
            "transaction": {
                "message": {
                    "accountKeys": [{"pubkey": "Trader111", "signer": true}],
                    "instructions": [{
                        "program": "spl-token",
                        "parsed": {
                            "type": "mintTo",
                            "info": {"mint": "Mint111", "amount": "10"}
                        }
                    }]
                }
            }
        });
        let summary = summarize_transaction(
            "Sig111",
            &transaction,
            None,
            &test_config(None),
            &HashSet::new(),
        );
        assert_eq!(summary["signature"], "Sig111");
        assert_eq!(summary["token_mints"][0], "Mint111");
        assert_eq!(summary["token_instructions"][0]["type"], "mintTo");
        assert!(summary["program_ids"]
            .as_array()
            .is_some_and(|programs| programs
                .iter()
                .any(|program| { program == "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc" })));
    }

    #[test]
    fn classifies_a_stablecoin_buy_and_whale_alert() {
        let transaction = json!({
            "slot": 42,
            "blockTime": 123,
            "meta": {
                "err": null,
                "preTokenBalances": [
                    {"accountIndex": 3, "mint": "Target111", "owner": "Trader111",
                     "uiTokenAmount": {"amount": "0", "decimals": 6}},
                    {"accountIndex": 4, "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "owner": "Trader111",
                     "uiTokenAmount": {"amount": "20000000000", "decimals": 6}}
                ],
                "postTokenBalances": [
                    {"accountIndex": 3, "mint": "Target111", "owner": "Trader111",
                     "uiTokenAmount": {"amount": "100000000", "decimals": 6}},
                    {"accountIndex": 4, "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "owner": "Trader111",
                     "uiTokenAmount": {"amount": "10000000000", "decimals": 6}}
                ]
            },
            "transaction": {
                "message": {
                    "accountKeys": [{"pubkey": "Trader111", "signer": true}],
                    "instructions": []
                }
            }
        });
        let mut config = test_config(Some("Target111"));
        config.whale_usd = 5_000.0;
        let summary = summarize_transaction("Sig111", &transaction, None, &config, &HashSet::new());
        assert_eq!(summary["trade"]["side"], "buy");
        assert_eq!(summary["trade"]["trader"], "Trader111");
        assert_eq!(summary["trade"]["notional_usd"], 10_000.0);
        assert_eq!(summary["trade"]["swap_ratio_quote_per_target"], 100.0);
        assert_eq!(summary["trade"]["swap_price_usd"], 100.0);
        assert_eq!(summary["trade"]["alert_type"], "whale_buy");
    }

    #[test]
    fn classifies_a_large_sell_as_a_whale_sell_alert() {
        let transaction = json!({
            "meta": {
                "preTokenBalances": [
                    {"accountIndex": 3, "mint": "Target111", "owner": "Trader111",
                     "uiTokenAmount": {"amount": "100000000", "decimals": 6}},
                    {"accountIndex": 4, "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "owner": "Trader111",
                     "uiTokenAmount": {"amount": "10000000000", "decimals": 6}}
                ],
                "postTokenBalances": [
                    {"accountIndex": 3, "mint": "Target111", "owner": "Trader111",
                     "uiTokenAmount": {"amount": "0", "decimals": 6}},
                    {"accountIndex": 4, "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "owner": "Trader111",
                     "uiTokenAmount": {"amount": "20000000000", "decimals": 6}}
                ]
            },
            "transaction": {
                "message": {"accountKeys": [{"pubkey": "Trader111", "signer": true}]}
            }
        });
        let mut config = test_config(Some("Target111"));
        config.whale_usd = 5_000.0;
        let summary =
            summarize_transaction("SigSell111", &transaction, None, &config, &HashSet::new());
        assert_eq!(summary["trade"]["side"], "sell");
        assert_eq!(summary["trade"]["notional_usd"], 10_000.0);
        assert_eq!(summary["trade"]["is_whale"], true);
        assert_eq!(summary["trade"]["alert_type"], "whale_sell");
    }

    #[test]
    fn classifies_by_token_owner_when_router_is_the_signer() {
        let transaction = json!({
            "meta": {
                "preTokenBalances": [
                    {"accountIndex": 3, "mint": "Target111", "owner": "Wallet111",
                     "uiTokenAmount": {"amount": "0", "decimals": 6}},
                    {"accountIndex": 4, "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "owner": "Wallet111",
                     "uiTokenAmount": {"amount": "100000000", "decimals": 6}}
                ],
                "postTokenBalances": [
                    {"accountIndex": 3, "mint": "Target111", "owner": "Wallet111",
                     "uiTokenAmount": {"amount": "5000000", "decimals": 6}},
                    {"accountIndex": 4, "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "owner": "Wallet111",
                     "uiTokenAmount": {"amount": "50000000", "decimals": 6}}
                ]
            },
            "transaction": {
                "message": {"accountKeys": [{"pubkey": "Router111", "signer": true}]}
            }
        });
        let summary = summarize_transaction(
            "SigOwner111",
            &transaction,
            None,
            &test_config(Some("Target111")),
            &HashSet::new(),
        );
        assert_eq!(summary["trade"]["side"], "buy");
        assert_eq!(summary["trade"]["trader"], "Wallet111");
        assert_eq!(summary["trade"]["target_amount_ui"], 5.0);
        assert_eq!(summary["trade"]["quote_amount_ui"], 50.0);
    }

    #[test]
    fn deduplicates_replayed_signatures() {
        let mut seen = SeenSignatures::default();
        assert!(seen.insert("Sig111", "Pool111"));
        assert!(!seen.insert("Sig111", "Pool111"));
        assert!(seen.insert("Sig111", "Pool222"));
        assert_eq!(
            notification_signature(&json!({
                "params": {"result": {"value": {"signature": "Sig222"}}}
            })),
            Some("Sig222".to_string())
        );
    }

    #[test]
    fn persists_events_in_sqlite_and_reads_them_back() {
        let path = std::env::temp_dir().join(format!("solana-events-{}.db", super::now_ms()));
        let whale_path = path.with_file_name(format!(
            "{}_whales.db",
            path.file_stem().unwrap().to_string_lossy()
        ));
        let connection = initialize_event_db(&path).unwrap();
        let event_db = Arc::new(Mutex::new(connection));
        let record = json!({
            "observed_at_ms": 1_790_000_000_000u64,
            "signature": "SigDb111",
            "watched_pool": "PoolDb111",
            "watched_protocol": "Raydium",
            "slot": 42,
            "event": {"method": "logsNotification"},
            "enrichment": {
                "available": true,
                "transaction": {
                    "token_mints": ["TargetDb111"],
                    "trade": {
                        "target_mint": "TargetDb111",
                        "side": "buy",
                        "trader": "TraderDb111",
                        "notional_usd": 12000.0,
                        "is_whale": true,
                        "alert_type": "whale_buy"
                    }
                }
            }
        });
        persist_event(&event_db, &record).unwrap();
        let events = read_event_db(
            &whale_path,
            &EventQuery {
                token: Some("TargetDb111".to_string()),
                side: Some("buy".to_string()),
                min_usd: Some(10_000.0),
                limit: Some(10),
                whale_only: Some(true),
            },
            true,
        )
        .unwrap();
        assert_eq!(events["count"], 1);
        assert_eq!(events["events"][0]["signature"], "SigDb111");
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(whale_path);
    }

    #[test]
    fn persists_realtime_pool_snapshots_in_the_separate_database() {
        let path = std::env::temp_dir().join(format!("solana-realtime-{}.db", super::now_ms()));
        let connection = initialize_realtime_db(&path).unwrap();
        let realtime_db = Arc::new(Mutex::new(connection));
        persist_pool_snapshot(
            &realtime_db,
            "PoolRealtime111",
            Some("Raydium"),
            Some("solana_standard"),
            Some("SigRealtime111"),
            Some(42),
            &json!({
                "available": true,
                "rpc_endpoint": "https://rpc.example",
                "slot": 43,
                "account": {"lamports": 123, "data": ["AQI=", "base64"]}
            }),
        )
        .unwrap();
        let snapshots = read_pool_snapshots(
            &path,
            &SnapshotQuery {
                pool: Some("PoolRealtime111".to_string()),
                limit: Some(10),
            },
        )
        .unwrap();
        assert_eq!(snapshots["count"], 1);
        assert_eq!(snapshots["snapshots"][0]["available"], true);
        assert_eq!(snapshots["snapshots"][0]["account_slot"], 43);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn pool_refresh_claim_is_rate_limited_per_pool() {
        let coordinator = PoolRefreshCoordinator::new(Connection::open_in_memory().unwrap(), 15);
        assert!(coordinator.claim("PoolRateLimited111"));
        assert!(!coordinator.claim("PoolRateLimited111"));
        assert!(coordinator.claim("PoolRateLimited222"));
    }
}
