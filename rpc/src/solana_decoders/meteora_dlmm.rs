use super::{decode_discriminator, find_instruction, has_program, ProtocolEvent};

pub(crate) const PROGRAM_ID: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";

pub(crate) fn decode_raw(data: &[u8]) -> Option<ProtocolEvent> {
    decode_discriminator(
        data,
        "Meteora DLMM",
        "solana_dlmm",
        PROGRAM_ID,
        &[
            (&[248, 198, 158, 145, 225, 117, 135, 200], "Swap", "swap", "swap", "Meteora DLMM swap; raw discriminator decoded from pool instruction"),
            (&[65, 75, 63, 76, 235, 91, 91, 136], "Swap2", "swap", "swap", "Meteora DLMM swap; raw discriminator decoded from pool instruction"),
            (&[250, 73, 101, 33, 38, 207, 75, 184], "SwapExactOut", "swap", "swap", "Meteora DLMM exact-output swap; raw discriminator decoded from pool instruction"),
            (&[43, 215, 247, 132, 137, 60, 243, 81], "SwapExactOut2", "swap", "swap", "Meteora DLMM exact-output swap; raw discriminator decoded from pool instruction"),
            (&[56, 173, 230, 208, 173, 228, 156, 205], "SwapWithPriceImpact", "swap", "swap", "Meteora DLMM protected swap; raw discriminator decoded from pool instruction"),
            (&[74, 98, 192, 214, 177, 51, 75, 51], "SwapWithPriceImpact2", "swap", "swap", "Meteora DLMM protected swap; raw discriminator decoded from pool instruction"),
            (&[181, 157, 89, 67, 143, 182, 52, 72], "AddLiquidity", "add_liquidity", "add liquidity", "Meteora DLMM liquidity deposit; raw discriminator decoded from pool instruction"),
            (&[228, 162, 78, 28, 70, 219, 116, 115], "AddLiquidity2", "add_liquidity", "add liquidity", "Meteora DLMM liquidity deposit; raw discriminator decoded from pool instruction"),
            (&[7, 3, 150, 127, 148, 40, 61, 200], "AddLiquidityByStrategy", "add_liquidity", "add liquidity", "Meteora DLMM strategy liquidity deposit; raw discriminator decoded from pool instruction"),
            (&[3, 221, 149, 218, 111, 141, 118, 213], "AddLiquidityByStrategy2", "add_liquidity", "add liquidity", "Meteora DLMM strategy liquidity deposit; raw discriminator decoded from pool instruction"),
            (&[41, 5, 238, 175, 100, 225, 6, 205], "AddLiquidityByStrategyOneSide", "add_liquidity", "add liquidity", "Meteora DLMM one-sided strategy deposit; raw discriminator decoded from pool instruction"),
            (&[28, 140, 238, 99, 231, 162, 21, 149], "AddLiquidityByWeight", "add_liquidity", "add liquidity", "Meteora DLMM weighted liquidity deposit; raw discriminator decoded from pool instruction"),
            (&[209, 59, 63, 91, 111, 200, 153, 228], "AddLiquidityByWeight2", "add_liquidity", "add liquidity", "Meteora DLMM weighted liquidity deposit; raw discriminator decoded from pool instruction"),
            (&[94, 155, 103, 151, 70, 95, 220, 165], "AddLiquidityOneSide", "add_liquidity", "add liquidity", "Meteora DLMM one-sided liquidity deposit; raw discriminator decoded from pool instruction"),
            (&[161, 194, 103, 84, 171, 71, 250, 154], "AddLiquidityOneSidePrecise", "add_liquidity", "add liquidity", "Meteora DLMM precise one-sided deposit; raw discriminator decoded from pool instruction"),
            (&[33, 51, 163, 201, 117, 98, 125, 231], "AddLiquidityOneSidePrecise2", "add_liquidity", "add liquidity", "Meteora DLMM precise one-sided deposit; raw discriminator decoded from pool instruction"),
            (&[80, 85, 209, 72, 24, 206, 177, 108], "RemoveLiquidity", "remove_liquidity", "remove liquidity", "Meteora DLMM liquidity withdrawal; raw discriminator decoded from pool instruction"),
            (&[230, 215, 82, 127, 241, 101, 227, 146], "RemoveLiquidity2", "remove_liquidity", "remove liquidity", "Meteora DLMM liquidity withdrawal; raw discriminator decoded from pool instruction"),
            (&[26, 82, 102, 152, 240, 74, 105, 26], "RemoveLiquidityByRange", "remove_liquidity", "remove liquidity", "Meteora DLMM range liquidity withdrawal; raw discriminator decoded from pool instruction"),
            (&[204, 2, 195, 145, 53, 145, 145, 205], "RemoveLiquidityByRange2", "remove_liquidity", "remove liquidity", "Meteora DLMM range liquidity withdrawal; raw discriminator decoded from pool instruction"),
            (&[10, 51, 61, 35, 112, 105, 24, 85], "RemoveAllLiquidity", "remove_liquidity", "remove liquidity", "Meteora DLMM full liquidity withdrawal; raw discriminator decoded from pool instruction"),
            (&[92, 4, 176, 193, 119, 185, 83, 9], "RebalanceLiquidity", "pool_maintenance", "pool / position maintenance", "Meteora DLMM position rebalance; no settled user trade"),
        ],
    )
}

pub(crate) fn decode(program_ids: &[String], logs: &[String]) -> Option<ProtocolEvent> {
    if !has_program(program_ids, PROGRAM_ID) {
        return None;
    }
    let (instruction, event_kind, action, explanation) = if let Some(instruction) = find_instruction(
        logs,
        &["Swap", "Swap2", "SwapExactOut", "SwapWithPriceImpact"],
    ) {
        (
            instruction,
            "swap",
            "swap",
            "Meteora DLMM swap instruction; owner balance deltas determine the settled trade",
        )
    } else if let Some(instruction) = find_instruction(
        logs,
        &[
            "AddLiquidity",
            "AddLiquidityByStrategy",
            "AddLiquidityByWeight",
            "InitializePositionAndAddLiquidity",
            "InitializePositionAndAddLiquidityByStrategy",
            "InitializePositionAndAddLiquidityByWeight",
        ],
    ) {
        (
            instruction,
            "add_liquidity",
            "add liquidity",
            "Meteora DLMM liquidity deposit into a bin-position range",
        )
    } else if let Some(instruction) = find_instruction(
        logs,
        &[
            "RemoveLiquidity",
            "RemoveLiquidityByRange",
            "RemoveAllLiquidity",
            "RemoveLiquiditySingleSide",
        ],
    ) {
        (
            instruction,
            "remove_liquidity",
            "remove liquidity",
            "Meteora DLMM liquidity withdrawal from a bin-position range",
        )
    } else if let Some(instruction) = find_instruction(
        logs,
        &[
            "RebalancePosition",
            "ClaimFee",
            "ClaimFee2",
            "ClaimLMReward",
            "UpdatePosition",
            "SetPairStatus",
            "SetActivationPoint",
        ],
    ) {
        (
            instruction,
            "pool_maintenance",
            "pool / position maintenance",
            "Meteora DLMM position, fee, reward or pool-state maintenance instruction",
        )
    } else {
        return None;
    };

    Some(ProtocolEvent {
        protocol: "Meteora DLMM",
        pool_type: "solana_dlmm",
        program_id: PROGRAM_ID.to_string(),
        instruction: instruction.to_string(),
        event_kind,
        action,
        explanation,
    })
}
