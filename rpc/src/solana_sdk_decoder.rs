//! Adapter for the checked-in `sol-parser-sdk` clone.
//!
//! The listener still fetches a `jsonParsed` transaction for balance deltas,
//! but the SDK needs the same transaction in `base64` form.  This module keeps
//! that transport-format detail out of the listener and turns the SDK's rich
//! `DexEvent` enum into small JSON records that the existing UI can consume.

use serde_json::{json, Value};
use sol_parser_sdk::{
    analyze_rpc_transaction_routes, parse_rpc_transaction, DexEvent, RouteSwapLeg, SwapProtocol,
};
use solana_transaction_status::EncodedConfirmedTransactionWithStatusMeta;

pub(crate) fn decode_transaction(transaction: &Value) -> Vec<Value> {
    let Ok(transaction) =
        serde_json::from_value::<EncodedConfirmedTransactionWithStatusMeta>(transaction.clone())
    else {
        return Vec::new();
    };
    let Ok(events) = parse_rpc_transaction(&transaction, None) else {
        return Vec::new();
    };
    events.into_iter().filter_map(normalize_event).collect()
}

/// Decode the executed swap legs independently from the event parser.  This
/// is important for Jupiter/router transactions: an owner balance delta is a
/// transaction-level total, while this route view identifies the exact pool
/// and the executed amounts for each leg.
pub(crate) fn decode_routes(transaction: &Value) -> Vec<Value> {
    let Ok(transaction) =
        serde_json::from_value::<EncodedConfirmedTransactionWithStatusMeta>(transaction.clone())
    else {
        return Vec::new();
    };
    let Ok(route) = analyze_rpc_transaction_routes(&transaction, &[]) else {
        return Vec::new();
    };
    route.legs.into_iter().map(normalize_route_leg).collect()
}

fn normalize_route_leg(leg: RouteSwapLeg) -> Value {
    json!({
        "protocol": route_protocol(leg.protocol),
        "pool": leg.pool.to_string(),
        "trader": leg.trader.to_string(),
        "input_mint": leg.input_mint.map(|mint| mint.to_string()),
        "output_mint": leg.output_mint.map(|mint| mint.to_string()),
        "actual_input_amount": leg.actual_input_amount,
        "actual_output_amount": leg.actual_output_amount,
        "specified_amount": leg.specified_amount,
        "amount_specified_is_input": leg.amount_specified_is_input,
        "position": {
            "outer_index": leg.position.outer_index,
            "inner_index": leg.position.inner_index,
            "stack_height": leg.position.stack_height,
        },
    })
}

fn route_protocol(protocol: SwapProtocol) -> &'static str {
    match protocol {
        SwapProtocol::PumpSwap => "PumpSwap",
        SwapProtocol::LaunchLab => "Raydium LaunchLab",
        SwapProtocol::RaydiumCpmm => "Raydium CPMM",
        SwapProtocol::RaydiumAmmV4 => "Raydium AMM V4",
        SwapProtocol::RaydiumClmm => "Raydium CLMM",
        SwapProtocol::OrcaWhirlpool => "Orca Whirlpools",
        SwapProtocol::MeteoraDlmm => "Meteora DLMM",
    }
}

fn normalize_event(event: DexEvent) -> Option<Value> {
    let (variant, data) = match event {
        DexEvent::MeteoraDlmmSwap(event) => ("MeteoraDlmmSwap", serde_json::to_value(event).ok()?),
        DexEvent::MeteoraDlmmAddLiquidity(event) => {
            ("MeteoraDlmmAddLiquidity", serde_json::to_value(event).ok()?)
        }
        DexEvent::MeteoraDlmmRemoveLiquidity(event) => (
            "MeteoraDlmmRemoveLiquidity",
            serde_json::to_value(event).ok()?,
        ),
        DexEvent::OrcaWhirlpoolSwap(event) => {
            ("OrcaWhirlpoolSwap", serde_json::to_value(event).ok()?)
        }
        DexEvent::OrcaWhirlpoolLiquidityIncreased(event) => (
            "OrcaWhirlpoolLiquidityIncreased",
            serde_json::to_value(event).ok()?,
        ),
        DexEvent::OrcaWhirlpoolLiquidityDecreased(event) => (
            "OrcaWhirlpoolLiquidityDecreased",
            serde_json::to_value(event).ok()?,
        ),
        DexEvent::RaydiumClmmSwap(event) => ("RaydiumClmmSwap", serde_json::to_value(event).ok()?),
        DexEvent::RaydiumClmmIncreaseLiquidity(event) => (
            "RaydiumClmmIncreaseLiquidity",
            serde_json::to_value(event).ok()?,
        ),
        DexEvent::RaydiumClmmDecreaseLiquidity(event) => (
            "RaydiumClmmDecreaseLiquidity",
            serde_json::to_value(event).ok()?,
        ),
        DexEvent::RaydiumCpmmSwap(event) => ("RaydiumCpmmSwap", serde_json::to_value(event).ok()?),
        DexEvent::RaydiumCpmmDeposit(event) => {
            ("RaydiumCpmmDeposit", serde_json::to_value(event).ok()?)
        }
        DexEvent::RaydiumCpmmWithdraw(event) => {
            ("RaydiumCpmmWithdraw", serde_json::to_value(event).ok()?)
        }
        DexEvent::RaydiumAmmV4Swap(event) => {
            ("RaydiumAmmV4Swap", serde_json::to_value(event).ok()?)
        }
        DexEvent::RaydiumAmmV4Deposit(event) => {
            ("RaydiumAmmV4Deposit", serde_json::to_value(event).ok()?)
        }
        DexEvent::RaydiumAmmV4Withdraw(event) => {
            ("RaydiumAmmV4Withdraw", serde_json::to_value(event).ok()?)
        }
        _ => return None,
    };
    let pool_address = pool_address(&variant, &data)?;
    let event_kind = if variant.ends_with("Swap") {
        "swap"
    } else if variant.contains("Add") || variant.contains("Increase") || variant.contains("Deposit")
    {
        "add_liquidity"
    } else if variant.contains("Remove")
        || variant.contains("Decrease")
        || variant.contains("Withdraw")
    {
        "remove_liquidity"
    } else {
        "protocol_event"
    };
    let protocol = if variant.starts_with("Meteora") {
        "Meteora"
    } else if variant.starts_with("Orca") {
        "Orca Whirlpools"
    } else {
        "Raydium"
    };
    let pool_type = match variant {
        v if v.starts_with("MeteoraDlmm") => "solana_dlmm",
        v if v.starts_with("OrcaWhirlpool") => "solana_whirlpool",
        "RaydiumClmmSwap" | "RaydiumClmmIncreaseLiquidity" | "RaydiumClmmDecreaseLiquidity" => {
            "solana_clmm"
        }
        _ => "solana_standard",
    };
    let instruction = data
        .get("ix_name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(variant)
        .to_string();
    Some(json!({
        "decoder": "sol_parser_sdk",
        "event": variant,
        "protocol": protocol,
        "pool_type": pool_type,
        "pool_address": pool_address,
        "event_kind": event_kind,
        "instruction": instruction,
        "details": event_details(&variant, &data),
    }))
}

fn event_details(variant: &str, data: &Value) -> Value {
    let fields: &[(&str, &str, bool)] = match variant {
        "MeteoraDlmmSwap" => &[
            ("pool", "pool", true),
            ("from", "from", true),
            ("token_x_mint", "token_x_mint", true),
            ("token_y_mint", "token_y_mint", true),
            ("user_token_in", "user_token_in", true),
            ("user_token_out", "user_token_out", true),
            ("amount_in", "amount_in", false),
            ("amount_out", "amount_out", false),
            ("swap_for_y", "swap_for_y", false),
            ("fee", "fee", false),
            ("protocol_fee", "protocol_fee", false),
            ("start_bin_id", "start_bin_id", false),
            ("end_bin_id", "end_bin_id", false),
        ],
        "MeteoraDlmmAddLiquidity" | "MeteoraDlmmRemoveLiquidity" => &[
            ("pool", "pool", true),
            ("from", "from", true),
            ("position", "position", true),
            ("amounts", "amounts", false),
            ("active_bin_id", "active_bin_id", false),
        ],
        "OrcaWhirlpoolSwap" => &[
            ("whirlpool", "whirlpool", true),
            ("token_mint_a", "token_mint_a", true),
            ("token_mint_b", "token_mint_b", true),
            ("input_amount", "input_amount", false),
            ("output_amount", "output_amount", false),
            ("a_to_b", "a_to_b", false),
            ("pre_sqrt_price", "pre_sqrt_price", false),
            ("post_sqrt_price", "post_sqrt_price", false),
            ("lp_fee", "lp_fee", false),
            ("protocol_fee", "protocol_fee", false),
            ("ix_name", "ix_name", false),
        ],
        "RaydiumClmmSwap" => &[
            ("pool_state", "pool_state", true),
            ("input_mint", "input_mint", true),
            ("output_mint", "output_mint", true),
            ("amount_0", "amount_0", false),
            ("amount_1", "amount_1", false),
            ("zero_for_one", "zero_for_one", false),
            ("sqrt_price_x64", "sqrt_price_x64", false),
            ("liquidity", "liquidity", false),
            ("tick", "tick", false),
            ("ix_name", "ix_name", false),
        ],
        "RaydiumCpmmSwap" => &[
            ("pool_id", "pool_id", true),
            ("input_token_mint", "input_token_mint", true),
            ("output_token_mint", "output_token_mint", true),
            ("input_amount", "input_amount", false),
            ("output_amount", "output_amount", false),
            ("base_input", "base_input", false),
            ("ix_name", "ix_name", false),
        ],
        "RaydiumAmmV4Swap" => &[
            ("amm", "amm", true),
            ("amount_in", "amount_in", false),
            ("amount_out", "amount_out", false),
            ("instruction_amount_in", "instruction_amount_in", false),
            ("instruction_amount_out", "instruction_amount_out", false),
            ("ix_name", "ix_name", false),
        ],
        _ => &[],
    };
    let mut output = serde_json::Map::new();
    for (name, source, is_pubkey) in fields {
        let value = if *is_pubkey {
            pubkey_value(data.get(*source)).map(Value::String)
        } else {
            data.get(*source).cloned()
        };
        if let Some(value) = value {
            output.insert((*name).to_string(), value);
        }
    }
    if variant == "MeteoraDlmmSwap" {
        // The instruction parser only has requested amounts; the emitted
        // swap event carries executed amounts and fee fields.
        let executed = data.get("amount_in").and_then(Value::as_u64).unwrap_or(0) > 0
            && data.get("amount_out").and_then(Value::as_u64).unwrap_or(0) > 0
            && (data.get("fee").and_then(Value::as_u64).unwrap_or(0) > 0
                || data
                    .get("protocol_fee")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0);
        output.insert("amounts_executed".to_string(), json!(executed));
    }
    Value::Object(output)
}

fn pool_address(variant: &str, data: &Value) -> Option<String> {
    let key = if variant.starts_with("MeteoraDlmm") {
        "pool"
    } else if variant.starts_with("OrcaWhirlpool") {
        "whirlpool"
    } else if variant.starts_with("RaydiumClmm") {
        "pool_state"
    } else if variant.starts_with("RaydiumCpmm") {
        "pool_id"
    } else {
        "amm"
    };
    pubkey_value(data.get(key))
}

fn pubkey_value(value: Option<&Value>) -> Option<String> {
    let bytes = value?
        .as_array()?
        .iter()
        .map(Value::as_u64)
        .collect::<Option<Vec<_>>>()?;
    if bytes.len() != 32 || bytes.iter().any(|byte| *byte > u8::MAX as u64) {
        return None;
    }
    Some(bs58::encode(bytes.into_iter().map(|byte| byte as u8).collect::<Vec<_>>()).into_string())
}
