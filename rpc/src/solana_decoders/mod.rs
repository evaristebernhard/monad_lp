//! Protocol-aware Solana transaction classification.
//!
//! The listener owns transport, RPC enrichment and persistence. These modules
//! own only protocol vocabulary: which program/instruction was invoked and
//! what user-facing event that instruction represents.

mod meteora_dlmm;
mod orca_whirlpool;
mod raydium;
pub(crate) mod registry;

use serde_json::Value;

#[derive(Clone, Debug)]
pub(crate) struct ProtocolEvent {
    pub protocol: &'static str,
    pub pool_type: &'static str,
    pub program_id: String,
    pub instruction: String,
    pub event_kind: &'static str,
    pub action: &'static str,
    pub explanation: &'static str,
}

pub(crate) fn decode_discriminator(
    data: &[u8],
    protocol: &'static str,
    pool_type: &'static str,
    program_id: &'static str,
    variants: &[(
        &[u8; 8],
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    )],
) -> Option<ProtocolEvent> {
    let discriminator = data.get(..8)?;
    let (_, instruction, event_kind, action, explanation) = variants
        .iter()
        .find(|(expected, ..)| discriminator == expected.as_slice())?;
    Some(ProtocolEvent {
        protocol,
        pool_type,
        program_id: program_id.to_string(),
        instruction: (*instruction).to_string(),
        event_kind,
        action,
        explanation,
    })
}

pub(crate) fn decode_legacy_discriminator(
    data: &[u8],
    protocol: &'static str,
    pool_type: &'static str,
    program_id: &'static str,
    variants: &[(u8, &'static str, &'static str, &'static str, &'static str)],
) -> Option<ProtocolEvent> {
    let discriminator = *data.first()?;
    let (_, instruction, event_kind, action, explanation) = variants
        .iter()
        .find(|(expected, ..)| *expected == discriminator)?;
    Some(ProtocolEvent {
        protocol,
        pool_type,
        program_id: program_id.to_string(),
        instruction: (*instruction).to_string(),
        event_kind,
        action,
        explanation,
    })
}

/// Decode the raw base58 instruction payloads returned by `getTransaction`.
///
/// This keeps Carbon's useful discriminator/account-layout approach in our own
/// small decoder modules. The listener can therefore continue using its
/// existing `logsSubscribe` transport and JSON/SQLite event shape, while
/// protocol recognition is based on raw instruction data instead of logs only.
pub(crate) fn decode_transaction(transaction: &Value) -> Vec<ProtocolEvent> {
    let account_keys = transaction_account_keys(transaction);
    let mut decoded = Vec::new();

    decode_instruction_array(
        transaction.pointer("/transaction/message/instructions"),
        &account_keys,
        &mut decoded,
    );
    if let Some(groups) = transaction
        .pointer("/meta/innerInstructions")
        .and_then(Value::as_array)
    {
        for group in groups {
            decode_instruction_array(group.get("instructions"), &account_keys, &mut decoded);
        }
    }

    decoded
}

fn decode_instruction_array(
    value: Option<&Value>,
    account_keys: &[String],
    output: &mut Vec<ProtocolEvent>,
) {
    for instruction in value.and_then(Value::as_array).into_iter().flatten() {
        let Some(program_id) = instruction_program_id(instruction, account_keys) else {
            continue;
        };
        let Some(data) = instruction.get("data").and_then(Value::as_str) else {
            continue;
        };
        let Ok(data) = bs58::decode(data).into_vec() else {
            continue;
        };
        if let Some(event) = decode_raw_instruction(&program_id, &data) {
            output.push(event);
        }
    }
}

fn decode_raw_instruction(program_id: &str, data: &[u8]) -> Option<ProtocolEvent> {
    if program_id == meteora_dlmm::PROGRAM_ID {
        return meteora_dlmm::decode_raw(data);
    }
    if program_id == orca_whirlpool::PROGRAM_ID {
        return orca_whirlpool::decode_raw(data);
    }
    if program_id == raydium::CLMM_PROGRAM_ID {
        return raydium::decode_clmm_raw(data);
    }
    if program_id == raydium::CPMM_PROGRAM_ID {
        return raydium::decode_cpmm_raw(data);
    }
    if program_id == raydium::AMM_PROGRAM_ID {
        return raydium::decode_amm_raw(data);
    }
    None
}

/// Decode the instruction name from a registered program when a dedicated
/// account/discriminator decoder is not available yet.  This is intentionally
/// conservative: it identifies the program and instruction, then classifies
/// the action for the event feed.  Amounts and prices still come from the
/// listener's balance-delta/RPC enrichment path.
pub(crate) fn decode_registered(
    pool_type: Option<&str>,
    program_ids: &[String],
    logs: &[String],
) -> Option<ProtocolEvent> {
    let descriptor = pool_type
        .and_then(registry::by_slug)
        .filter(|descriptor| has_program(program_ids, descriptor.program_id))
        .or_else(|| {
            program_ids
                .iter()
                .filter_map(|program_id| registry::by_program(program_id))
                .find(|descriptor| descriptor.category != "infrastructure")
        })
        .or_else(|| {
            program_ids
                .iter()
                .find_map(|program_id| registry::by_program(program_id))
        })?;
    let instruction = logs.iter().find_map(|log| {
        let (_, raw) = log.split_once("Instruction:")?;
        let raw = raw.trim();
        (!raw.is_empty()).then_some(raw)
    })?;
    let normalized = normalized_instruction(instruction);
    let (event_kind, action, explanation) = if normalized.contains("swap")
        || normalized.contains("trade")
        || normalized.contains("buy")
        || normalized.contains("sell")
        || normalized.contains("fill")
        || normalized.contains("exchange")
    {
        (
            "swap",
            "swap",
            "Registered Solana protocol instruction; owner balance deltas determine the settled trade",
        )
    } else if normalized.contains("add")
        || normalized.contains("deposit")
        || normalized.contains("increase")
        || normalized.contains("openposition")
        || normalized.contains("provide")
    {
        (
            "add_liquidity",
            "add liquidity",
            "Registered Solana protocol liquidity deposit or position increase",
        )
    } else if normalized.contains("remove")
        || normalized.contains("withdraw")
        || normalized.contains("decrease")
        || normalized.contains("closeposition")
        || normalized.contains("redeem")
    {
        (
            "remove_liquidity",
            "remove liquidity",
            "Registered Solana protocol liquidity withdrawal or position close",
        )
    } else {
        (
            "pool_maintenance",
            "pool / position maintenance",
            "Registered Solana protocol instruction; no settled user trade was identified",
        )
    };
    Some(ProtocolEvent {
        protocol: descriptor.protocol,
        pool_type: descriptor.slug,
        program_id: descriptor.program_id.to_string(),
        instruction: instruction.to_string(),
        event_kind,
        action,
        explanation,
    })
}

fn transaction_account_keys(transaction: &Value) -> Vec<String> {
    let mut keys = transaction
        .pointer("/transaction/message/accountKeys")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|key| {
            key.as_str()
                .or_else(|| key.get("pubkey").and_then(Value::as_str))
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>();
    for path in [
        "/meta/loadedAddresses/writable",
        "/meta/loadedAddresses/readonly",
    ] {
        keys.extend(
            transaction
                .pointer(path)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned),
        );
    }
    keys
}

fn instruction_program_id(instruction: &Value, account_keys: &[String]) -> Option<String> {
    instruction
        .get("programId")
        .and_then(Value::as_str)
        .or_else(|| {
            instruction
                .get("programIdIndex")
                .and_then(Value::as_u64)
                .and_then(|index| account_keys.get(index as usize).map(String::as_str))
        })
        .map(ToOwned::to_owned)
}

pub(crate) fn decode(
    pool_type: Option<&str>,
    program_ids: &[String],
    logs: &[String],
) -> Option<ProtocolEvent> {
    match pool_type {
        Some("solana_dlmm") => meteora_dlmm::decode(program_ids, logs),
        Some("solana_whirlpool") => orca_whirlpool::decode(program_ids, logs),
        Some("solana_standard") | Some("solana_clmm") => {
            raydium::decode(pool_type, program_ids, logs)
        }
        _ => meteora_dlmm::decode(program_ids, logs)
            .or_else(|| orca_whirlpool::decode(program_ids, logs))
            .or_else(|| raydium::decode(None, program_ids, logs)),
    }
    .or_else(|| decode_registered(pool_type, program_ids, logs))
}

pub(crate) fn has_program(program_ids: &[String], program_id: &str) -> bool {
    program_ids.iter().any(|id| id == program_id)
}

pub(crate) fn normalized_instruction(log: &str) -> String {
    log.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub(crate) fn find_instruction<'a>(logs: &'a [String], names: &[&str]) -> Option<&'a str> {
    logs.iter().find_map(|log| {
        let (_, raw) = log.split_once("Instruction:")?;
        let normalized = normalized_instruction(raw);
        names
            .iter()
            .any(|name| normalized == normalized_instruction(name))
            .then_some(raw.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::{decode, decode_transaction, ProtocolEvent};

    fn assert_event(event: Option<ProtocolEvent>, kind: &str, instruction: &str) {
        let event = event.expect("protocol instruction should decode");
        assert_eq!(event.event_kind, kind);
        assert_eq!(event.instruction, instruction);
    }

    #[test]
    fn decodes_meteora_dlmm_liquidity_instruction() {
        assert_event(
            decode(
                Some("solana_dlmm"),
                &["LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo".to_string()],
                &["Program log: Instruction: AddLiquidityByStrategy".to_string()],
            ),
            "add_liquidity",
            "AddLiquidityByStrategy",
        );
    }

    #[test]
    fn decodes_meteora_dlmm_cpi_swap_instruction() {
        assert_event(
            decode(
                Some("solana_dlmm"),
                &["LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo".to_string()],
                &["Program log: Instruction: Swap".to_string()],
            ),
            "swap",
            "Swap",
        );
    }

    #[test]
    fn classifies_registered_protocol_when_dedicated_decoder_is_not_present() {
        assert_event(
            decode(
                None,
                &["pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA".to_string()],
                &["Program log: Instruction: Buy".to_string()],
            ),
            "swap",
            "Buy",
        );
    }

    #[test]
    fn decodes_orca_whirlpool_position_instruction() {
        assert_event(
            decode(
                Some("solana_whirlpool"),
                &["whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc".to_string()],
                &["Program log: Instruction: IncreaseLiquidityV2".to_string()],
            ),
            "add_liquidity",
            "IncreaseLiquidityV2",
        );
    }

    #[test]
    fn decodes_raydium_cpmm_deposit() {
        assert_event(
            decode(
                Some("solana_standard"),
                &["CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C".to_string()],
                &["Program log: Instruction: Deposit".to_string()],
            ),
            "add_liquidity",
            "Deposit",
        );
    }

    #[test]
    fn decodes_raw_instruction_data_without_log_messages() {
        let transaction = serde_json::json!({
            "transaction": {"message": {"accountKeys": [], "instructions": [{
                "programId": "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
                "accounts": [],
                "data": bs58::encode([248, 198, 158, 145, 225, 117, 135, 200, 1, 0]).into_string()
            }]}},
            "meta": {"innerInstructions": []}
        });
        let events = decode_transaction(&transaction);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].instruction, "Swap");
        assert_eq!(events[0].event_kind, "swap");
    }

    #[test]
    fn decodes_orca_token_amount_liquidity_instruction() {
        let transaction = serde_json::json!({
            "transaction": {"message": {"accountKeys": [], "instructions": [{
                "programId": "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",
                "accounts": [],
                "data": bs58::encode([239, 251, 9, 124, 210, 198, 53, 43, 0]).into_string()
            }]}},
            "meta": {"innerInstructions": []}
        });
        let events = decode_transaction(&transaction);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].instruction, "IncreaseLiquidityByTokenAmountsV2");
        assert_eq!(events[0].event_kind, "add_liquidity");
    }

    #[test]
    fn decodes_legacy_raydium_tag_without_anchor_logs() {
        let transaction = serde_json::json!({
            "transaction": {"message": {"accountKeys": [], "instructions": [{
                "programId": "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",
                "accounts": [],
                "data": bs58::encode([9, 1, 2, 3]).into_string()
            }]}},
            "meta": {"innerInstructions": []}
        });
        let events = decode_transaction(&transaction);
        assert_eq!(events[0].instruction, "SwapBaseIn");
        assert_eq!(events[0].event_kind, "swap");
    }
}
