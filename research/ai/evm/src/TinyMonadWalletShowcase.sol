// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// @notice Wallet-owned trades, with all model decisions computed on Monad Testnet.
/// @dev Every round requires a transaction signed by the participant. Archived test vectors
///      are supplied by the frontend for a short demonstration, not a live price oracle.
interface IWalletDemoPolicy {
    function predict(int32[12] calldata features) external view returns(int256[3] memory scores,uint8 action);
}
interface IWalletDemoToken {
    function balanceOf(address holder) external view returns(uint256);
}
interface IWalletDemoRouter {
    function WETH() external view returns(address);
    function getAmountsOut(uint256 amountIn,address[] calldata path) external view returns(uint256[] memory);
    function swapExactTokensForTokens(
        uint256 amountIn,uint256 amountOutMin,address[] calldata path,address recipient,uint256 deadline
    ) external returns(uint256[] memory);
}

contract TinyMonadWalletShowcase {
    uint256 public constant COOLDOWN = 10;
    uint8 public constant ROUNDS = 6;
    uint256 public constant BUY_USDC = 400_000;  // 0.40 test USDC
    uint256 public constant SELL_WMON = 100_000_000_000_000_000; // 0.10 test WMON
    uint256 public constant SLIPPAGE_BPS = 300; // never worse than 3% of instant on-chain quote

    address public immutable model;
    address public immutable router;
    address public immutable usdc;
    address public immutable wmon;
    mapping(address => uint8) public nextRound;
    mapping(address => uint64) public lastRoundTime;
    mapping(address => uint64) public session;
    bool private locked;

    event Decision(address indexed wallet,uint64 indexed session,uint8 indexed round,
        bytes32 inputHash,uint8 action,int256 holdScore,int256 buyScore,int256 sellScore,
        uint256 amountIn,uint256 amountOut);
    event Restart(address indexed wallet,uint64 indexed session);

    modifier nonReentrant() {
        require(!locked,"reentrant");
        locked=true;
        _;
        locked=false;
    }

    constructor(address _model,address _router,address _usdc,address _wmon) {
        require(block.chainid==10143,"Monad testnet only");
        require(_model.code.length>0 && _router.code.length>0 &&
            _usdc.code.length>0 && _wmon.code.length>0,"missing code");
        require(_usdc!=_wmon && IWalletDemoRouter(_router).WETH()==_wmon,"wrong pair");
        model=_model;router=_router;usdc=_usdc;wmon=_wmon;
    }

    function restart() external {
        require(nextRound[msg.sender]==ROUNDS,"unfinished");
        nextRound[msg.sender]=0;
        lastRoundTime[msg.sender]=0;
        ++session[msg.sender];
        emit Restart(msg.sender,session[msg.sender]);
    }

    // No deposit, no keeper. The wallet owns each swap and signs each execution.
    // The owner must first allow *this contract* to spend only demo-sized ERC20 amounts.
    // HOLD is also a real transaction (event + round progression) but has no swap.
    function execute(int32[12] calldata features,uint8 expectedRound,uint256 minOut)
        external nonReentrant returns(uint8 action,uint256 amountOut)
    {
        require(block.chainid==10143,"testnet only");
        require(nextRound[msg.sender]<ROUNDS && expectedRound==nextRound[msg.sender],"round");
        uint64 last=lastRoundTime[msg.sender];
        require(last==0 || block.timestamp>=uint256(last)+COOLDOWN,"10 second cooldown");

        (int256[3] memory scores,uint8 prediction)=IWalletDemoPolicy(model).predict(features);
        require(prediction<=2,"invalid action");
        action=prediction;
        uint256 amountIn;
        if(action==0) {
            require(minOut==0,"HOLD output");
        } else {
            address from=action==1?usdc:wmon;
            address to=action==1?wmon:usdc;
            amountIn=action==1?BUY_USDC:SELL_WMON;
            require(IWalletDemoToken(from).balanceOf(msg.sender)>=amountIn,"wallet balance");
            address[] memory path=new address[](2);
            path[0]=from;path[1]=to;
            uint256[] memory quote=IWalletDemoRouter(router).getAmountsOut(amountIn,path);
            require(quote.length==2 && quote[1]>0,"empty quote");
            require(minOut>0 && minOut>=quote[1]*(10000-SLIPPAGE_BPS)/10000,"low minOut");
            _safeCall(from,abi.encodeWithSignature(
                "transferFrom(address,address,uint256)",msg.sender,address(this),amountIn));
            _safeCall(from,abi.encodeWithSignature("approve(address,uint256)",router,0));
            _safeCall(from,abi.encodeWithSignature("approve(address,uint256)",router,amountIn));
            uint256 balanceBefore=IWalletDemoToken(to).balanceOf(msg.sender);
            IWalletDemoRouter(router).swapExactTokensForTokens(amountIn,minOut,path,msg.sender,block.timestamp);
            amountOut=IWalletDemoToken(to).balanceOf(msg.sender)-balanceBefore;
            require(amountOut>=minOut,"swap output");
        }
        uint8 round=nextRound[msg.sender];
        nextRound[msg.sender]=round+1;
        lastRoundTime[msg.sender]=uint64(block.timestamp);
        emit Decision(msg.sender,session[msg.sender],round,keccak256(abi.encode(features)),
            action,scores[0],scores[1],scores[2],amountIn,amountOut);
    }

    function _safeCall(address token,bytes memory calldata_) private {
        (bool ok,bytes memory data)=token.call(calldata_);
        require(ok && (data.length==0 || (data.length==32 && abi.decode(data,(bool)))),"ERC20 failure");
    }
}
