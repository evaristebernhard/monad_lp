use super::{
    decode_discriminator, decode_legacy_discriminator, find_instruction, has_program, ProtocolEvent,
};

pub(crate) const CLMM_PROGRAM_ID: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";
pub(crate) const CPMM_PROGRAM_ID: &str = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";
pub(crate) const AMM_PROGRAM_ID: &str = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";

pub(crate) fn decode_clmm_raw(data: &[u8]) -> Option<ProtocolEvent> {
    decode_discriminator(
        data,
        "Raydium",
        "solana_clmm",
        CLMM_PROGRAM_ID,
        &[
            (&[248, 198, 158, 145, 225, 117, 135, 200], "Swap", "swap", "swap", "Raydium CLMM swap; raw discriminator decoded from pool instruction"),
            (&[43, 4, 237, 11, 26, 201, 30, 98], "SwapV2", "swap", "swap", "Raydium CLMM swap v2; raw discriminator decoded from pool instruction"),
            (&[135, 128, 47, 77, 15, 152, 240, 49], "OpenPosition", "add_liquidity", "add liquidity", "Raydium CLMM position opened; raw discriminator decoded from pool instruction"),
            (&[77, 184, 74, 214, 112, 86, 241, 199], "OpenPositionV2", "add_liquidity", "add liquidity", "Raydium CLMM position opened v2; raw discriminator decoded from pool instruction"),
            (&[77, 255, 174, 82, 125, 29, 201, 46], "OpenPositionWithToken22Nft", "add_liquidity", "add liquidity", "Raydium CLMM position opened with Token-2022 NFT; raw discriminator decoded from pool instruction"),
            (&[46, 156, 243, 118, 13, 205, 251, 178], "IncreaseLiquidity", "add_liquidity", "add liquidity", "Raydium CLMM liquidity increased; raw discriminator decoded from pool instruction"),
            (&[133, 29, 89, 223, 69, 238, 176, 10], "IncreaseLiquidityV2", "add_liquidity", "add liquidity", "Raydium CLMM liquidity increased v2; raw discriminator decoded from pool instruction"),
            (&[160, 38, 208, 111, 104, 91, 44, 1], "DecreaseLiquidity", "remove_liquidity", "remove liquidity", "Raydium CLMM liquidity decreased; raw discriminator decoded from pool instruction"),
            (&[58, 127, 188, 62, 79, 82, 196, 96], "DecreaseLiquidityV2", "remove_liquidity", "remove liquidity", "Raydium CLMM liquidity decreased v2; raw discriminator decoded from pool instruction"),
            (&[123, 134, 81, 0, 49, 68, 98, 98], "ClosePosition", "remove_liquidity", "remove liquidity", "Raydium CLMM position closed; raw discriminator decoded from pool instruction"),
            (&[130, 87, 108, 6, 46, 224, 117, 123], "UpdatePoolStatus", "pool_maintenance", "pool / position maintenance", "Raydium CLMM pool status update; no settled user trade"),
        ],
    )
}

pub(crate) fn decode_cpmm_raw(data: &[u8]) -> Option<ProtocolEvent> {
    decode_discriminator(
        data,
        "Raydium",
        "solana_standard",
        CPMM_PROGRAM_ID,
        &[
            (
                &[143, 190, 90, 218, 196, 30, 51, 222],
                "SwapBaseInput",
                "swap",
                "swap",
                "Raydium CPMM base-input swap; raw discriminator decoded from pool instruction",
            ),
            (
                &[55, 217, 98, 86, 163, 74, 180, 173],
                "SwapBaseOutput",
                "swap",
                "swap",
                "Raydium CPMM base-output swap; raw discriminator decoded from pool instruction",
            ),
            (
                &[242, 35, 198, 137, 82, 225, 242, 182],
                "Deposit",
                "add_liquidity",
                "add liquidity",
                "Raydium CPMM LP deposit; raw discriminator decoded from pool instruction",
            ),
            (
                &[183, 18, 70, 156, 148, 109, 161, 34],
                "Withdraw",
                "remove_liquidity",
                "remove liquidity",
                "Raydium CPMM LP withdrawal; raw discriminator decoded from pool instruction",
            ),
            (
                &[130, 87, 108, 6, 46, 224, 117, 123],
                "UpdatePoolStatus",
                "pool_maintenance",
                "pool / position maintenance",
                "Raydium CPMM pool status update; no settled user trade",
            ),
        ],
    )
}

pub(crate) fn decode_amm_raw(data: &[u8]) -> Option<ProtocolEvent> {
    decode_legacy_discriminator(
        data,
        "Raydium",
        "solana_standard",
        AMM_PROGRAM_ID,
        &[
            (
                9,
                "SwapBaseIn",
                "swap",
                "swap",
                "Raydium AMM v4 base-input swap; raw instruction tag decoded",
            ),
            (
                16,
                "SwapBaseInV2",
                "swap",
                "swap",
                "Raydium AMM v4 base-input swap v2; raw instruction tag decoded",
            ),
            (
                11,
                "SwapBaseOut",
                "swap",
                "swap",
                "Raydium AMM v4 base-output swap; raw instruction tag decoded",
            ),
            (
                17,
                "SwapBaseOutV2",
                "swap",
                "swap",
                "Raydium AMM v4 base-output swap v2; raw instruction tag decoded",
            ),
            (
                3,
                "Deposit",
                "add_liquidity",
                "add liquidity",
                "Raydium AMM v4 LP deposit; raw instruction tag decoded",
            ),
            (
                4,
                "Withdraw",
                "remove_liquidity",
                "remove liquidity",
                "Raydium AMM v4 LP withdrawal; raw instruction tag decoded",
            ),
            (
                7,
                "WithdrawPnl",
                "pool_maintenance",
                "pool / position maintenance",
                "Raydium AMM v4 PnL withdrawal; no user swap",
            ),
            (
                8,
                "WithdrawSrm",
                "pool_maintenance",
                "pool / position maintenance",
                "Raydium AMM v4 SRM withdrawal; no user swap",
            ),
            (
                0,
                "Initialize",
                "pool_maintenance",
                "pool / position maintenance",
                "Raydium AMM v4 pool initialization; no user swap",
            ),
            (
                1,
                "Initialize2",
                "pool_maintenance",
                "pool / position maintenance",
                "Raydium AMM v4 pool initialization; no user swap",
            ),
        ],
    )
}

pub(crate) fn decode(
    pool_type: Option<&str>,
    program_ids: &[String],
    logs: &[String],
) -> Option<ProtocolEvent> {
    let (selected_type, program_id) = match pool_type {
        Some("solana_clmm") if has_program(program_ids, CLMM_PROGRAM_ID) => {
            ("solana_clmm", CLMM_PROGRAM_ID)
        }
        Some("solana_standard") if has_program(program_ids, CPMM_PROGRAM_ID) => {
            ("solana_standard", CPMM_PROGRAM_ID)
        }
        Some("solana_standard") if has_program(program_ids, AMM_PROGRAM_ID) => {
            ("solana_standard", AMM_PROGRAM_ID)
        }
        None if has_program(program_ids, CLMM_PROGRAM_ID) => ("solana_clmm", CLMM_PROGRAM_ID),
        None if has_program(program_ids, CPMM_PROGRAM_ID) => ("solana_standard", CPMM_PROGRAM_ID),
        None if has_program(program_ids, AMM_PROGRAM_ID) => ("solana_standard", AMM_PROGRAM_ID),
        _ => return None,
    };

    let (instruction, event_kind, action, explanation) = if let Some(instruction) = find_instruction(
        logs,
        &[
            "Swap",
            "SwapV2",
            "SwapBaseInput",
            "SwapBaseOutput",
            "SwapBaseIn",
            "SwapBaseOut",
        ],
    ) {
        (
            instruction,
            "swap",
            "swap",
            "Raydium swap instruction; owner balance deltas determine the settled trade",
        )
    } else if selected_type == "solana_clmm" {
        if let Some(instruction) = find_instruction(
            logs,
            &[
                "OpenPosition",
                "OpenPositionV2",
                "OpenPositionWithToken22Nft",
                "IncreaseLiquidity",
                "IncreaseLiquidityV2",
            ],
        ) {
            (
                instruction,
                "add_liquidity",
                "add liquidity",
                "Raydium CLMM position opened or liquidity increased",
            )
        } else if let Some(instruction) = find_instruction(
            logs,
            &["DecreaseLiquidity", "DecreaseLiquidityV2", "ClosePosition"],
        ) {
            (
                instruction,
                "remove_liquidity",
                "remove liquidity",
                "Raydium CLMM liquidity decreased or position closed",
            )
        } else if let Some(instruction) = find_instruction(
            logs,
            &[
                "CollectRemainingRewards",
                "CollectProtocolFee",
                "UpdateRewardInfos",
            ],
        ) {
            (
                instruction,
                "pool_maintenance",
                "pool / position maintenance",
                "Raydium CLMM reward or pool-state maintenance instruction",
            )
        } else {
            return None;
        }
    } else if let Some(instruction) = find_instruction(logs, &["Deposit"]) {
        (
            instruction,
            "add_liquidity",
            "add liquidity",
            "Raydium CPMM/AMM LP deposit; pool tokens are minted to the provider",
        )
    } else if let Some(instruction) = find_instruction(logs, &["Withdraw"]) {
        (
            instruction,
            "remove_liquidity",
            "remove liquidity",
            "Raydium CPMM/AMM LP withdrawal; pool tokens are burned",
        )
    } else if let Some(instruction) = find_instruction(
        logs,
        &[
            "Initialize",
            "Initialize2",
            "InitializeWithPermission",
            "UpdatePoolStatus",
        ],
    ) {
        (
            instruction,
            "pool_maintenance",
            "pool maintenance",
            "Raydium pool initialization or status maintenance instruction",
        )
    } else {
        return None;
    };

    Some(ProtocolEvent {
        protocol: "Raydium",
        pool_type: selected_type,
        program_id: program_id.to_string(),
        instruction: instruction.to_string(),
        event_kind,
        action,
        explanation,
    })
}
