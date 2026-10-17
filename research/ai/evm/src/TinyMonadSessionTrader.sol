// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// @title TinyMonadSessionTrader
/// @notice Testnet demonstration: wallet keeps funds; a bounded one-minute session
///         allows a dedicated keeper to submit SIX model-derived decisions.
///         No key, deposit, live oracle, or admin trading switch lives in this contract.
interface ISessionPolicy {
    function predict(int32[12] calldata features) external view returns (int256[3] memory scores, uint8 action);
}
interface ISessionERC20 {
    function balanceOf(address account) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);
}
interface ISessionRouter {
    function WETH() external view returns (address);
    function getAmountsOut(uint256 amountIn, address[] calldata path) external view returns (uint256[] memory);
    function swapExactTokensForTokens(
        uint256 amountIn, uint256 amountOutMin, address[] calldata path, address to, uint256 deadline
    ) external returns (uint256[] memory amounts);
}

contract TinyMonadSessionTrader {
    uint256 public constant CHAIN_ID = 10143;
    uint8 public constant ROUNDS = 6;
    uint256 public constant INTERVAL = 10 seconds;
    uint256 public constant SESSION_TTL = 110 seconds;
    uint256 public constant BUY_AMOUNT = 400_000; // 0.4 test USDC
    uint256 public constant SELL_AMOUNT = 100_000_000_000_000_000; // 0.1 test WMON
    uint256 public constant MAX_USDC_SPEND = 800_000; // two buys
    uint256 public constant MAX_WMON_SPEND = 200_000_000_000_000_000; // two sells
    uint256 public constant SLIPPAGE_BPS = 100; // 1% versus the fresh on-chain quote

    address public immutable model;
    address public immutable router;
    address public immutable usdc;
    address public immutable wmon;
    address public immutable keeper;

    struct Session {
        uint64 id;
        uint64 startedAt;
        uint64 lastExecution;
        uint8 nextRound;
        bool active;
    }

    mapping(address => Session) public sessions;
    bool private locked;

    event SessionStarted(address indexed wallet, uint64 indexed sessionId, uint64 startedAt, uint64 expiresAt);
    event SessionCancelled(address indexed wallet, uint64 indexed sessionId);
    event Decision(address indexed wallet, uint64 indexed sessionId, uint8 indexed round,
        uint8 action, bytes32 inputHash, uint256 amountIn, uint256 amountOut, int256[3] scores);
    modifier nonReentrant() {
        require(!locked, "reentrant");
        locked = true;
        _;
        locked = false;
    }

    constructor(address _model, address _router, address _usdc, address _wmon, address _keeper) {
        require(block.chainid == CHAIN_ID, "testnet only");
        require(_model.code.length > 0 && _router.code.length > 0, "missing contract");
        require(_usdc.code.length > 0 && _wmon.code.length > 0, "missing asset");
        require(_keeper != address(0) && _usdc != _wmon, "invalid configuration");
        require(ISessionRouter(_router).WETH() == _wmon, "wrong WMON");
        model = _model;
        router = _router;
        usdc = _usdc;
        wmon = _wmon;
        keeper = _keeper;
    }

    /// @notice Exactly one wallet signature to begin the demo after two bounded token approvals.
    /// @dev Each session is separate, expires automatically, and can be cancelled by the wallet.
    function start() external nonReentrant {
        Session storage s = sessions[msg.sender];
        require(!s.active || s.nextRound == ROUNDS ||
            block.timestamp > uint256(s.startedAt) + SESSION_TTL, "session active");
        require(ISessionERC20(usdc).allowance(msg.sender, address(this)) >= MAX_USDC_SPEND, "approve USDC");
        require(ISessionERC20(wmon).allowance(msg.sender, address(this)) >= MAX_WMON_SPEND, "approve WMON");
        require(ISessionERC20(usdc).balanceOf(msg.sender) >= MAX_USDC_SPEND, "need test USDC");
        ++s.id;
        s.startedAt = uint64(block.timestamp);
        s.lastExecution = 0;
        s.nextRound = 0;
        s.active = true;
        emit SessionStarted(msg.sender, s.id, s.startedAt, uint64(block.timestamp + SESSION_TTL));
    }

    function cancel() external {
        Session storage s = sessions[msg.sender];
        require(s.active, "no active session");
        s.active = false;
        emit SessionCancelled(msg.sender, s.id);
    }

    /// @dev Replay inputs are fixed real historical feature vectors. The keeper CANNOT
    ///      choose arbitrary BUY/SELL data, volumes, recipients, or a different DEX.
    function expectedInputHash(uint8 round) public pure returns (bytes32) {
        if (round == 0) return 0x362cfa68642a98186f90bd094a56c1677c15d0e4d17f97816cadd89aba866b33;
        if (round == 1) return 0xd5d4224eba649aaa0dfecb54f6770ad46dee177f65bdb8483f0ce22de82a5b7d;
        if (round == 2) return 0xfd527975743c8f5045fa92c4b5aa77cf8ccbee393ea2f7faa405fa90e65ab215;
        if (round == 3) return 0x98f550c4094401961db56f61a0f8a7af4b37d927bd7b328c399fa65026824e3d;
        if (round == 4) return 0xcf5815f69ed0cd4ebc5ef9f934636b8fea83d8ac759821ac76d63f152f1a4b5d;
        if (round == 5) return 0xfd527975743c8f5045fa92c4b5aa77cf8ccbee393ea2f7faa405fa90e65ab215;
        revert("round out of range");
    }

    /// @notice Call only from the dedicated keeper. Assets settle wallet <-> test DEX.
    function executeFor(address wallet, uint64 sessionId, uint8 round, int32[12] calldata features, uint256 minOut)
        external nonReentrant returns (uint8 action, uint256 amountOut)
    {
        require(block.chainid == CHAIN_ID && msg.sender == keeper, "keeper only");
        Session storage s = sessions[wallet];
        require(s.active && s.id == sessionId && round == s.nextRound && round < ROUNDS, "session or round");
        require(block.timestamp <= uint256(s.startedAt) + SESSION_TTL, "expired");
        require(block.timestamp >= uint256(s.startedAt) + uint256(round) * INTERVAL, "round not due");
        require(s.lastExecution == 0 || block.timestamp >= uint256(s.lastExecution) + INTERVAL, "cooldown");
        bytes32 inputHash = keccak256(abi.encode(features));
        require(inputHash == expectedInputHash(round), "unexpected features");

        (int256[3] memory scores, uint8 predicted) = ISessionPolicy(model).predict(features);
        require(predicted <= 2, "invalid prediction");
        action = predicted;
        uint256 amountIn;
        if (action == 0) {
            require(minOut == 0, "hold minOut");
        } else {
            address from = action == 1 ? usdc : wmon;
            address to = action == 1 ? wmon : usdc;
            amountIn = action == 1 ? BUY_AMOUNT : SELL_AMOUNT;
            require(ISessionERC20(from).balanceOf(wallet) >= amountIn, "wallet balance");
            address[] memory path = new address[](2);
            path[0] = from;
            path[1] = to;
            uint256[] memory amounts = ISessionRouter(router).getAmountsOut(amountIn, path);
            require(amounts.length == 2 && amounts[1] > 0, "no quote");
            require(minOut > 0 && minOut >= amounts[1] * (10000 - SLIPPAGE_BPS) / 10000, "slippage");
            _safeCall(from, abi.encodeWithSignature(
                "transferFrom(address,address,uint256)", wallet, address(this), amountIn));
            _safeCall(from, abi.encodeWithSignature("approve(address,uint256)", router, 0));
            _safeCall(from, abi.encodeWithSignature("approve(address,uint256)", router, amountIn));
            uint256 before = ISessionERC20(to).balanceOf(wallet);
            ISessionRouter(router).swapExactTokensForTokens(amountIn, minOut, path, wallet, block.timestamp);
            amountOut = ISessionERC20(to).balanceOf(wallet) - before;
            require(amountOut >= minOut, "swap output");
        }
        ++s.nextRound;
        s.lastExecution = uint64(block.timestamp);
        if (s.nextRound == ROUNDS) s.active = false;
        emit Decision(wallet, s.id, round, action, inputHash, amountIn, amountOut, scores);
    }

    function _safeCall(address token, bytes memory payload) private {
        (bool ok, bytes memory ret) = token.call(payload);
        require(ok && (ret.length == 0 || (ret.length == 32 && abi.decode(ret, (bool)))), "token failure");
    }
}
