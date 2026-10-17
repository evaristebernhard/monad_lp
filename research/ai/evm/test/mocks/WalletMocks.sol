// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "../../src/TinyMonadSessionTrader.sol";

interface VmTrader {
    function chainId(uint256 newChainId) external;
    function warp(uint256 newTimestamp) external;
    function prank(address actor) external;
}

contract MockCoin {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        return true;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "insufficient");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function transferFrom(address sender, address recipient, uint256 amount) external returns (bool) {
        require(balanceOf[sender] >= amount, "insufficient");
        require(allowance[sender][msg.sender] >= amount, "allowance");
        allowance[sender][msg.sender] -= amount;
        balanceOf[sender] -= amount;
        balanceOf[recipient] += amount;
        return true;
    }
}

contract ShowcaseWalletRouter is ISessionRouter {
    address public immutable override WETH;
    address public immutable usdc;

    constructor(address wrapped, address stable) {
        WETH = wrapped;
        usdc = stable;
    }

    function getAmountsOut(uint256 amount, address[] calldata path)
        public view returns (uint256[] memory amounts)
    {
        amounts = new uint256[](2);
        amounts[0] = amount;
        amounts[1] = path[0] == usdc ? amount * 1e12 / 4 : amount * 4 / 1e12;
    }

    function swapExactTokensForTokens(
        uint256 amount, uint256 minOut, address[] calldata path, address to, uint256
    ) external returns (uint256[] memory amounts) {
        amounts = getAmountsOut(amount, path);
        require(amounts[1] >= minOut, "minimum");
        MockCoin(path[0]).transferFrom(msg.sender, address(this), amount);
        MockCoin(path[1]).transfer(to, amounts[1]);
    }
}
