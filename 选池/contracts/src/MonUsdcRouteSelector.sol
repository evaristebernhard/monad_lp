// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "./MonUsdcQuoteSelector.sol";

/// @notice Same-pair quote ranking over a fixed multi-protocol route universe.
/// @dev V4 routes use PoolIds. V2/V3 routes use zero-padded pool addresses.
/// Native MON and wrapped MON quote inputs normalize to native MON in Context.
/// Caller supplies observations. No oracle authentication, wrapping or swaps.
contract MonUsdcRouteSelector is MonUsdcQuoteSelector {
    constructor(bytes32[] memory routeIds) MonUsdcQuoteSelector(routeIds) {}
}
