// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "../src/TinyMonadTrader.sol";
import "../src/TinyMonadPolicy.sol";

interface VmTrader { function chainId(uint256) external; function warp(uint256) external; function prank(address) external; function deal(address,uint256) external; }

contract MockCoin {
    mapping(address=>uint256) public balanceOf;
    mapping(address=>mapping(address=>uint256)) public allowance;
    function mint(address who,uint256 amount) external { balanceOf[who]+=amount; }
    function transfer(address to,uint256 amount) external returns(bool) { require(balanceOf[msg.sender]>=amount);balanceOf[msg.sender]-=amount;balanceOf[to]+=amount;return true; }
    function approve(address guy,uint256 amount) external returns(bool) { allowance[msg.sender][guy]=amount;return true; }
    function transferFrom(address src,address to,uint256 amount) external returns(bool) { require(balanceOf[src]>=amount && allowance[src][msg.sender]>=amount);balanceOf[src]-=amount;allowance[src][msg.sender]-=amount;balanceOf[to]+=amount;return true; }
}
contract MockWrap is MockCoin {
    function deposit() external payable { balanceOf[msg.sender]+=msg.value; }
    function withdraw(uint256 wad) external {require(balanceOf[msg.sender]>=wad); balanceOf[msg.sender]-=wad;(bool ok,)=payable(msg.sender).call{value:wad}("");require(ok); }
}
contract MockRouter is IV2RouterTiny {
    address public immutable override WETH;
    constructor(address wrapped) {WETH=wrapped;}
    function getAmountsOut(uint256 n,address[] calldata) external pure returns(uint256[] memory q) {q=new uint256[](2);q[0]=n;q[1]=n;}
    function swapExactTokensForTokens(uint256 n,uint256 minOut,address[] calldata path,address to,uint256) external returns(uint256[] memory q) {
        require(n>=minOut,"too little");MockCoin(path[0]).transferFrom(msg.sender,address(this),n);
        MockCoin(path[1]).transfer(to,n);
        q=new uint256[](2);q[0]=n;q[1]=n;
    }
}
contract MockSellPolicy is ITinyPolicy {
    function predict(int32[12] calldata) external pure returns (int256[3] memory scores, uint8 action) {
        return ([int256(0), int256(0), int256(1)], 2);
    }
}
contract TinyMonadTraderTest {
    VmTrader constant vm=VmTrader(address(uint160(uint256(keccak256("hevm cheat code")))));
    TinyMonadTrader trader;
    TinyMonadPolicy model;
    MockCoin stable;
    MockWrap wrapped;
    MockRouter router;
    address constant KEEPER=address(0xBEEF);
    uint64 constant T=1792209600;
    function setUp() public {
        vm.chainId(10143);vm.warp(uint256(T)+100);
        model=new TinyMonadPolicy();
        stable=new MockCoin();
        wrapped=new MockWrap();
        router=new MockRouter(address(wrapped));
        trader=new TinyMonadTrader(address(model),KEEPER,address(router),address(stable),address(wrapped),100 ether,100 ether);
        stable.mint(address(trader),100 ether);
        wrapped.mint(address(trader),100 ether);
        stable.mint(address(router),1000 ether);
        wrapped.mint(address(router),1000 ether);
    }
    function buyInput() private pure returns(int32[12] memory a) {
        a=[int32(5051),-10891,-11697,-8551,-7757,7702,5001,-9463,-9922,10685,6590,5671];
    }
    function holdInput() private pure returns(int32[12] memory a) {
        a=[int32(-251),951,1551,-2262,-421,-2252,-292,1317,-642,-1298,-1447,-5488];
    }
    function testBuyAndHold() public {
        vm.prank(KEEPER);uint8 b=trader.execute(buyInput(),T,1 ether,97 ether/100);
        require(b==1,"buy");require(wrapped.balanceOf(address(trader))==101 ether,"balance");
        vm.warp(uint256(T)+3600+100);vm.prank(KEEPER);
        uint8 h=trader.execute(holdInput(),T+3600,0,0);
        require(h==0,"hold");
    }
    function testRejectRepeatAndUnauthorized() public {
        (bool unauthorized,)=address(trader).call(abi.encodeCall(trader.execute,(buyInput(),T,1 ether,97 ether/100)));
        require(!unauthorized,"anyone can trade");
        vm.prank(KEEPER);trader.execute(buyInput(),T,1 ether,97 ether/100);
        vm.prank(KEEPER);(bool repeated,)=address(trader).call(abi.encodeCall(trader.execute,(buyInput(),T,1 ether,97 ether/100)));
        require(!repeated,"replay");
    }
    function testSellDirectionAndBalances() public {
        TinyMonadTrader selling = new TinyMonadTrader(address(new MockSellPolicy()), KEEPER, address(router), address(stable), address(wrapped), 100 ether, 100 ether);
        wrapped.mint(address(selling), 2 ether);
        uint256 beforeStable = stable.balanceOf(address(selling));
        vm.prank(KEEPER);
        uint8 action = selling.execute(holdInput(), T, 1 ether, 97 ether / 100);
        require(action == 2, "sell direction");
        require(stable.balanceOf(address(selling)) == beforeStable + 1 ether, "sell received");
        require(wrapped.balanceOf(address(selling)) == 1 ether, "sell inventory");
    }
    function testSlippageRollsBackCandle() public {
        vm.prank(KEEPER);(bool ok,)=address(trader).call(abi.encodeCall(trader.execute,(buyInput(),T,1 ether,1)));
        require(!ok,"slippage bypass");require(trader.lastCandleEnd()==0,"advanced on failure");
    }
}
