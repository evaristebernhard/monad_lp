// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// @notice Monad testnet only. Six signed demo decisions per session, each >= 10s apart.
/// @dev Replay archived real-market model features; do NOT claim these are live market signals.
interface IShowPolicy {
    function predict(int32[12] calldata x) external view returns (int256[3] memory, uint8);
}
interface IShowToken {
    function balanceOf(address) external view returns (uint256);
}
interface IShowWrap {
    function deposit() external payable;
}
interface IShowRouter {
    function WETH() external view returns (address);
    function getAmountsOut(uint256, address[] calldata) external view returns (uint256[] memory);
    function swapExactTokensForTokens(uint256,uint256,address[] calldata,address,uint256) external returns (uint256[] memory);
}

contract TinyMonadShowcase {
    uint256 public constant CHAIN_ID = 10143;
    uint256 public constant COOLDOWN = 10;
    uint8 public constant ROUNDS = 6;
    uint256 public constant BUY_USDC = 400_000; // 0.4 USDC ~= 0.1 WMON at time of setup.
    uint256 public constant SELL_WMON = 100_000_000_000_000_000; // 0.1 WMON.
    address public immutable owner;
    address public keeper;
    IShowPolicy public immutable model;
    IShowRouter public immutable router;
    address public immutable usdc;
    address public immutable wmon;

    uint64 public session;
    uint8 public step;
    uint256 public lastExecution;
    bool public paused;
    bool private locked;

    event Round(uint64 indexed session, uint8 indexed step, bytes32 indexed inputHash, uint8 action,
        int256 holdScore, int256 buyScore, int256 sellScore, uint256 amountIn, uint256 amountOut);
    event Session(uint64 indexed session);
    event Keeper(address indexed keeper);
    event Pause(bool paused);

    modifier onlyOwner() {require(msg.sender==owner,"owner"); _;}
    modifier nonReentrant() {require(!locked,"reentry");locked=true;_;locked=false;}

    constructor(address _model,address _router,address _usdc,address _wmon,address _keeper) {
        require(block.chainid==CHAIN_ID,"testnet only");
        require(_model.code.length>0 && _router.code.length>0 &&
            _usdc.code.length>0 && _wmon.code.length>0,"invalid code");
        require(_keeper!=address(0) && _usdc!=_wmon,"config");
        require(IShowRouter(_router).WETH()==_wmon,"wrong wrapper");
        owner=msg.sender;keeper=_keeper;model=IShowPolicy(_model);
        router=IShowRouter(_router);usdc=_usdc;wmon=_wmon;
        session=1;
        emit Session(session);
    }

    function _tokenCall(address token,bytes memory payload) private {
        (bool ok,bytes memory ret)=token.call(payload);
        require(ok && (ret.length==0 || (ret.length==32 && abi.decode(ret,(bool)))),"token call");
    }

    function fundUSDC(uint256 amount) external onlyOwner nonReentrant {
        require(amount>0,"amount");
        _tokenCall(usdc,abi.encodeWithSignature("transferFrom(address,address,uint256)",msg.sender,address(this),amount));
    }
    function fundMON() external payable onlyOwner nonReentrant {
        require(msg.value>0,"amount");
        IShowWrap(wmon).deposit{value:msg.value}();
    }
    function withdraw(address token,uint256 amount) external onlyOwner nonReentrant {
        require(token==usdc || token==wmon,"token");
        _tokenCall(token,abi.encodeWithSignature("transfer(address,uint256)",owner,amount));
    }
    function setKeeper(address next) external onlyOwner {
        require(next!=address(0),"keeper");
        keeper=next;emit Keeper(next);
    }
    function setPaused(bool value) external onlyOwner {
        paused=value;emit Pause(value);
    }
    function restart() external onlyOwner {
        require(step==ROUNDS,"unfinished");
        ++session;
        step=0;
        lastExecution=0;
        emit Session(session);
    }
    // The keeper submits actual archived features. The contract NEVER accepts a buy/sell flag.
    function execute(int32[12] calldata features,uint8 expectedStep,uint256 minOut)
        external nonReentrant returns(uint8 action,uint256 amountOut)
    {
        require(block.chainid==CHAIN_ID && msg.sender==keeper && !paused,"keeper");
        require(step<ROUNDS && expectedStep==step,"step");
        require(lastExecution==0 || block.timestamp>=lastExecution+COOLDOWN,"cooldown");
        (int256[3] memory scores,uint8 result)=model.predict(features);
        require(result<=2,"bad action");
        action=result;
        uint256 amountIn;
        if(action!=0) {
            address from=action==1?usdc:wmon;
            address to=action==1?wmon:usdc;
            amountIn=action==1?BUY_USDC:SELL_WMON;
            require(IShowToken(from).balanceOf(address(this))>=amountIn,"inventory");
            address[] memory path=new address[](2);
            path[0]=from;path[1]=to;
            uint256[] memory quote=router.getAmountsOut(amountIn,path);
            require(quote.length==2 && quote[1]>0,"quote");
            require(minOut>0 && minOut>=quote[1]*97/100,"slippage");
            _tokenCall(from,abi.encodeWithSignature("approve(address,uint256)",address(router),0));
            _tokenCall(from,abi.encodeWithSignature("approve(address,uint256)",address(router),amountIn));
            uint256 balanceBefore=IShowToken(to).balanceOf(address(this));
            router.swapExactTokensForTokens(amountIn,minOut,path,address(this),block.timestamp);
            amountOut=IShowToken(to).balanceOf(address(this))-balanceBefore;
            require(amountOut>=minOut,"no output");
        } else {
            require(minOut==0,"hold minimum");
        }
        uint8 current=step;
        step=current+1;
        lastExecution=block.timestamp;
        emit Round(session,current,keccak256(abi.encode(features)),action,
            scores[0],scores[1],scores[2],amountIn,amountOut);
    }
}
