// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "./MonUsdcRouteSelector.sol";

/// @notice Paid quote replay demonstration; does not authenticate quotes or swap.
/// @dev Read-only eth_call remains free. Only a mined selectAndPay pays the creator.
contract PaidMonUsdcRouteSelector {
    address payable public immutable creator;
    uint256 public immutable serviceFee;
    MonUsdcRouteSelector public immutable selector;
    bool private entered;

    event SelectionPaid(address indexed caller, address indexed recipient, uint256 fee,
        bytes32 poolId, uint256 amountOut, bytes32 replayHash);

    constructor(bytes32[] memory ids, uint256 fee) {
        require(fee > 0, "zero fee");
        creator = payable(msg.sender);
        serviceFee = fee;
        selector = new MonUsdcRouteSelector(ids);
    }

    function selectAndPay(MonUsdcQuoteSelector.Context calldata context,
        MonUsdcQuoteSelector.Quote[] calldata quotes)
        external payable returns (bytes32 poolId, uint256 amountOut, bytes32 replayHash)
    {
        require(!entered, "reentrant");
        require(msg.value == serviceFee, "exact MON fee required");
        entered = true;
        (poolId, amountOut, replayHash) = selector.selectBest(context, quotes);
        (bool sent,) = creator.call{value: msg.value}("");
        require(sent, "fee transfer failed");
        emit SelectionPaid(msg.sender, creator, msg.value, poolId, amountOut, replayHash);
        entered = false;
    }
}
