// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// @notice TESTNET ONLY: integer AI inference and fixed USDC/WMON V2 swaps.
/// @dev Candle features come from a trusted keeper. This contract does not verify market data.
interface ITinyPolicy {
    function predict(int32[12] calldata input) external view returns (int256[3] memory scores, uint8 action);
}
interface IERC20Tiny {
    function balanceOf(address) external view returns (uint256);
}
interface IWMONTiny {
    function deposit() external payable;
    function withdraw(uint256 wad) external;
}
interface IV2RouterTiny {
    function WETH() external view returns (address);
    function getAmountsOut(uint256 amountIn, address[] calldata path) external view returns (uint256[] memory);
    function swapExactTokensForTokens(
        uint256 amountIn, uint256 amountOutMin, address[] calldata path,
        address to, uint256 deadline
    ) external returns (uint256[] memory amounts);
}

/// @notice Fixed model, assets, router, and testnet chain to prevent unsafe reuse.
contract TinyMonadTrader {
    address public immutable owner;
    address public keeper;
    ITinyPolicy public immutable model;
    IV2RouterTiny public immutable router;
    address public immutable usdc;
    address public immutable wmon;
    uint256 public immutable maxBuyUSDC;
    uint256 public immutable maxSellWMON;
    uint64 public lastCandleEnd;
    bool public paused;
    bool private locked;

    uint256 public constant MAX_SLIPPAGE_BPS = 300;
    uint256 public constant MAX_CANDLE_AGE = 3 hours;

    event Decision(uint64 indexed candleEnd, bytes32 indexed inputHash, uint8 action, int256 score0, int256 score1, int256 score2);
    event Swap(uint64 indexed candleEnd, uint8 indexed action, uint256 amountIn, uint256 amountOut);
    event Fund(address indexed token, uint256 amount);
    event Withdraw(address indexed token, uint256 amount);
    event KeeperChanged(address indexed keeper);
    event PauseChanged(bool paused);

    modifier onlyOwner() { require(msg.sender == owner, "owner"); _; }
    modifier nonReentrant() {
        require(!locked, "reentrant");
        locked = true;
        _;
        locked = false;
    }

    constructor(
        address _model, address _keeper, address _router, address _usdc, address _wmon,
        uint256 _maxBuyUSDC, uint256 _maxSellWMON
    ) {
        require(block.chainid == 10143, "testnet only");
        require(_model.code.length > 0 && _router.code.length > 0, "invalid contracts");
        require(_usdc.code.length > 0 && _wmon.code.length > 0, "invalid tokens");
        require(_keeper != address(0) && _usdc != _wmon, "invalid config");
        require(_maxBuyUSDC > 0 && _maxSellWMON > 0, "zero cap");
        require(IV2RouterTiny(_router).WETH() == _wmon, "wrong wrapped MON");
        owner = msg.sender;
        keeper = _keeper;
        model = ITinyPolicy(_model);
        router = IV2RouterTiny(_router);
        usdc = _usdc;
        wmon = _wmon;
        maxBuyUSDC = _maxBuyUSDC;
        maxSellWMON = _maxSellWMON;
    }

    function _safeTokenCall(address token, bytes memory payload) private {
        (bool ok, bytes memory data) = token.call(payload);
        require(ok && (data.length == 0 || (data.length == 32 && abi.decode(data, (bool)))), "ERC20 call failed");
    }

    function fundUSDC(uint256 amount) external onlyOwner nonReentrant {
        require(amount > 0, "zero amount");
        _safeTokenCall(usdc, abi.encodeWithSignature("transferFrom(address,address,uint256)", msg.sender, address(this), amount));
        emit Fund(usdc, amount);
    }

    /// @notice Convert native test MON to the router's redeemable WMON.
    function fundMON() external payable onlyOwner nonReentrant {
        require(msg.value > 0, "zero amount");
        IWMONTiny(wmon).deposit{value: msg.value}();
        emit Fund(wmon, msg.value);
    }

    function withdrawToken(address token, uint256 amount) external onlyOwner nonReentrant {
        require(token == usdc || token == wmon, "not strategy token");
        _safeTokenCall(token, abi.encodeWithSignature("transfer(address,uint256)", owner, amount));
        emit Withdraw(token, amount);
    }

    /// @notice Receive MON when unwrapping WMON, then pay it to the owner.
    function withdrawMON(uint256 amount) external onlyOwner nonReentrant {
        IWMONTiny(wmon).withdraw(amount);
        (bool ok,) = payable(owner).call{value: amount}("");
        require(ok, "MON transfer failed");
        emit Withdraw(wmon, amount);
    }

    receive() external payable {
        require(msg.sender == wmon, "only WMON unwrap");
    }

    function setKeeper(address next) external onlyOwner {
        require(next != address(0), "zero keeper");
        keeper = next;
        emit KeeperChanged(next);
    }
    function setPaused(bool value) external onlyOwner {
        paused = value;
        emit PauseChanged(value);
    }

    /// @param candleEnd UTC close time of the last *completed* hourly candle.
    /// @param amountIn Maximum actual fixed-side token input. Must also satisfy immutable size cap.
    /// @param minOut Caller-specified slippage floor checked against the current router quote.
    /// @return action 0=HOLD, 1=BUY WMON with USDC, 2=SELL WMON for USDC.
    function execute(int32[12] calldata input, uint64 candleEnd, uint256 amountIn, uint256 minOut)
        external nonReentrant returns (uint8 action)
    {
        require(block.chainid == 10143, "testnet only");
        require(msg.sender == keeper && !paused, "keeper or paused");
        require(candleEnd % 3600 == 0 && candleEnd > lastCandleEnd, "stale or duplicate candle");
        require(block.timestamp >= candleEnd && block.timestamp - candleEnd <= MAX_CANDLE_AGE, "bad candle time");

        (int256[3] memory scores, uint8 direction) = model.predict(input);
        require(direction <= 2, "bad action");
        action = direction;
        // Set before any token/router calls. Reverts restore the timestamp.
        lastCandleEnd = candleEnd;
        emit Decision(candleEnd, keccak256(abi.encode(input)), action, scores[0], scores[1], scores[2]);
        if (action == 0) return 0;

        address from = action == 1 ? usdc : wmon;
        address to = action == 1 ? wmon : usdc;
        require(amountIn > 0 && amountIn <= (action == 1 ? maxBuyUSDC : maxSellWMON), "trade cap");
        require(IERC20Tiny(from).balanceOf(address(this)) >= amountIn, "no inventory");
        address[] memory path = new address[](2);
        path[0] = from;
        path[1] = to;
        uint256[] memory quote = router.getAmountsOut(amountIn, path);
        require(quote.length == 2 && quote[1] > 0, "no quote");
        require(minOut > 0 && minOut >= quote[1] * (10000 - MAX_SLIPPAGE_BPS) / 10000, "slippage floor");

        _safeTokenCall(from, abi.encodeWithSignature("approve(address,uint256)", address(router), 0));
        _safeTokenCall(from, abi.encodeWithSignature("approve(address,uint256)", address(router), amountIn));
        uint256 beforeBalance = IERC20Tiny(to).balanceOf(address(this));
        router.swapExactTokensForTokens(amountIn, minOut, path, address(this), block.timestamp);
        uint256 received = IERC20Tiny(to).balanceOf(address(this)) - beforeBalance;
        require(received >= minOut, "not received");
        emit Swap(candleEnd, action, amountIn, received);
    }
}
