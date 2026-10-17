use super::{decode_discriminator, find_instruction, has_program, ProtocolEvent};

pub(crate) const PROGRAM_ID: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";

pub(crate) fn decode_raw(data: &[u8]) -> Option<ProtocolEvent> {
    decode_discriminator(
        data,
        "Orca Whirlpools",
        "solana_whirlpool",
        PROGRAM_ID,
        &[
            (&[248, 198, 158, 145, 225, 117, 135, 200], "Swap", "swap", "swap", "Orca Whirlpool swap; raw discriminator decoded from pool instruction"),
            (&[43, 4, 237, 11, 26, 201, 30, 98], "SwapV2", "swap", "swap", "Orca Whirlpool swap v2; raw discriminator decoded from pool instruction"),
            (&[195, 96, 237, 108, 68, 162, 219, 230], "TwoHopSwap", "swap", "swap", "Orca Whirlpool two-hop swap; raw discriminator decoded from pool instruction"),
            (&[186, 143, 209, 29, 254, 2, 194, 117], "TwoHopSwapV2", "swap", "swap", "Orca Whirlpool two-hop swap v2; raw discriminator decoded from pool instruction"),
            (&[135, 128, 47, 77, 15, 152, 240, 49], "OpenPosition", "add_liquidity", "add liquidity", "Orca Whirlpool position opened; raw discriminator decoded from pool instruction"),
            (&[242, 29, 134, 48, 58, 110, 14, 60], "OpenPositionWithMetadata", "add_liquidity", "add liquidity", "Orca Whirlpool position opened with metadata; raw discriminator decoded from pool instruction"),
            (&[46, 156, 243, 118, 13, 205, 251, 178], "IncreaseLiquidity", "add_liquidity", "add liquidity", "Orca Whirlpool liquidity increased; raw discriminator decoded from pool instruction"),
            (&[133, 29, 89, 223, 69, 238, 176, 10], "IncreaseLiquidityV2", "add_liquidity", "add liquidity", "Orca Whirlpool liquidity increased v2; raw discriminator decoded from pool instruction"),
            (&[239, 251, 9, 124, 210, 198, 53, 43], "IncreaseLiquidityByTokenAmountsV2", "add_liquidity", "add liquidity", "Orca Whirlpool token-amount liquidity increase v2; raw discriminator decoded from pool instruction"),
            (&[212, 47, 95, 92, 114, 102, 131, 250], "OpenPositionWithTokenExtensions", "add_liquidity", "add liquidity", "Orca Whirlpool token-extension position opened; raw discriminator decoded from pool instruction"),
            (&[169, 113, 126, 171, 213, 172, 212, 49], "OpenBundledPosition", "add_liquidity", "add liquidity", "Orca Whirlpool bundled position opened; raw discriminator decoded from pool instruction"),
            (&[160, 38, 208, 111, 104, 91, 44, 1], "DecreaseLiquidity", "remove_liquidity", "remove liquidity", "Orca Whirlpool liquidity decreased; raw discriminator decoded from pool instruction"),
            (&[58, 127, 188, 62, 79, 82, 196, 96], "DecreaseLiquidityV2", "remove_liquidity", "remove liquidity", "Orca Whirlpool liquidity decreased v2; raw discriminator decoded from pool instruction"),
            (&[123, 134, 81, 0, 49, 68, 98, 98], "ClosePosition", "remove_liquidity", "remove liquidity", "Orca Whirlpool position closed; raw discriminator decoded from pool instruction"),
            (&[1, 182, 135, 59, 155, 25, 99, 223], "ClosePositionWithTokenExtensions", "remove_liquidity", "remove liquidity", "Orca Whirlpool position closed; raw discriminator decoded from pool instruction"),
            (&[41, 36, 216, 245, 27, 85, 103, 67], "CloseBundledPosition", "remove_liquidity", "remove liquidity", "Orca Whirlpool bundled position closed; raw discriminator decoded from pool instruction"),
            (&[191, 169, 224, 11, 131, 19, 158, 253], "RepositionLiquidityV2", "pool_maintenance", "pool / position maintenance", "Orca Whirlpool position range maintenance; no settled user trade"),
            (&[164, 152, 207, 99, 30, 186, 19, 182], "CollectFees", "pool_maintenance", "pool / position maintenance", "Orca Whirlpool fee collection; no settled user trade"),
            (&[207, 117, 95, 191, 229, 180, 226, 15], "CollectFeesV2", "pool_maintenance", "pool / position maintenance", "Orca Whirlpool fee collection v2; no settled user trade"),
            (&[70, 5, 132, 87, 86, 235, 177, 34], "CollectReward", "pool_maintenance", "pool / position maintenance", "Orca Whirlpool reward collection; no settled user trade"),
            (&[177, 107, 37, 180, 160, 19, 49, 209], "CollectRewardV2", "pool_maintenance", "pool / position maintenance", "Orca Whirlpool reward collection v2; no settled user trade"),
            (&[154, 230, 250, 13, 236, 209, 75, 223], "UpdateFeesAndRewards", "pool_maintenance", "pool / position maintenance", "Orca Whirlpool fee and reward update; no settled user trade"),
            (&[164, 123, 180, 141, 194, 100, 160, 175], "ResetPositionRange", "pool_maintenance", "pool / position maintenance", "Orca Whirlpool position range reset; no settled user trade"),
        ],
    )
}

pub(crate) fn decode(program_ids: &[String], logs: &[String]) -> Option<ProtocolEvent> {
    if !has_program(program_ids, PROGRAM_ID) {
        return None;
    }
    let (instruction, event_kind, action, explanation) = if let Some(instruction) =
        find_instruction(logs, &["Swap", "SwapV2", "TwoHopSwap", "TwoHopSwapV2"])
    {
        (
            instruction,
            "swap",
            "swap",
            "Orca Whirlpool swap instruction; owner balance deltas determine the settled trade",
        )
    } else if let Some(instruction) = find_instruction(
        logs,
        &[
            "OpenPosition",
            "OpenPositionWithMetadata",
            "OpenPositionV2",
            "OpenPositionWithMetadataV2",
            "OpenPositionWithTokenExtensions",
            "OpenBundledPosition",
            "IncreaseLiquidity",
            "IncreaseLiquidityV2",
            "IncreaseLiquidityByTokenAmountsV2",
        ],
    ) {
        (
            instruction,
            "add_liquidity",
            "add liquidity",
            "Orca Whirlpool position opened or liquidity increased",
        )
    } else if let Some(instruction) = find_instruction(
        logs,
        &[
            "DecreaseLiquidity",
            "DecreaseLiquidityV2",
            "ClosePosition",
            "ClosePositionWithTokenExtensions",
            "CloseBundledPosition",
        ],
    ) {
        (
            instruction,
            "remove_liquidity",
            "remove liquidity",
            "Orca Whirlpool liquidity decreased or position closed",
        )
    } else if let Some(instruction) = find_instruction(
        logs,
        &[
            "CollectFees",
            "CollectReward",
            "UpdateFeesAndRewards",
            "InitializeTickArray",
            "SetRewardAuthority",
            "SetFeeRate",
        ],
    ) {
        (
            instruction,
            "pool_maintenance",
            "pool / position maintenance",
            "Orca Whirlpool fee, reward, tick-array or pool-state maintenance instruction",
        )
    } else {
        return None;
    };

    Some(ProtocolEvent {
        protocol: "Orca Whirlpools",
        pool_type: "solana_whirlpool",
        program_id: PROGRAM_ID.to_string(),
        instruction: instruction.to_string(),
        event_kind,
        action,
        explanation,
    })
}
