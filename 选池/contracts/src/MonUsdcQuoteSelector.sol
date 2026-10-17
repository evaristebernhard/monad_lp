// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// @notice K=1 deterministic selection over a fixed, hook-free mainnet pool universe.
/// @dev TESTNET REPLAY DEMO. Quotes are caller-supplied mainnet observations.
/// This contract does NOT authenticate quotes, call a DEX, trade, or custody funds.
/// It is a single-pool V4 quote baseline, not the V2 DCAR split optimizer.
contract MonUsdcQuoteSelector {
    address public constant USDC = 0x754704Bc059F8C67012fEd69BC8A327a5aafb603;
    uint256 public immutable poolCount;
    bytes32 public immutable universeHash;
    mapping(bytes32 => bool) public allowedPool;

    struct Context {
        uint256 sourceChainId;
        uint256 sourceBlock;
        bytes32 sourceBlockHash;
        address tokenIn;
        address tokenOut;
        uint256 amountIn;
    }
    struct Quote { bytes32 poolId; uint256 amountOut; }

    constructor(bytes32[] memory poolIds) {
        require(poolIds.length > 0 && poolIds.length <= 32, "universe size");
        for (uint256 i; i < poolIds.length; i++) {
            require(poolIds[i] != bytes32(0) && !allowedPool[poolIds[i]], "duplicate pool");
            allowedPool[poolIds[i]] = true;
        }
        poolCount = poolIds.length;
        universeHash = keccak256(abi.encode(poolIds));
    }

    function selectBest(Context calldata context, Quote[] calldata quotes)
        external view returns (bytes32 poolId, uint256 amountOut, bytes32 replayHash)
    {
        require(context.sourceChainId == 143, "source chain");
        require(context.sourceBlock > 0 && context.sourceBlockHash != bytes32(0), "source block");
        require(context.amountIn > 0, "amount");
        require((context.tokenIn == address(0) && context.tokenOut == USDC)
             || (context.tokenIn == USDC && context.tokenOut == address(0)), "pair");
        require(quotes.length == poolCount, "complete universe");
        for (uint256 i; i < quotes.length; i++) {
            Quote calldata quote = quotes[i];
            require(allowedPool[quote.poolId], "unknown pool");
            for (uint256 j; j < i; j++) require(quotes[j].poolId != quote.poolId, "duplicate quote");
            if (quote.amountOut > amountOut || (quote.amountOut > 0 && quote.amountOut == amountOut && quote.poolId < poolId)) {
                poolId = quote.poolId;
                amountOut = quote.amountOut;
            }
        }
        require(amountOut > 0, "no quote");
        replayHash = keccak256(abi.encode(universeHash, context, quotes));
    }
}
