// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "../src/TinyMonadWalletShowcase.sol";
import "../src/TinyMonadPolicy.sol";
import "./TinyMonadTrader.t.sol";

contract ShowcaseWalletRouter is IWalletDemoRouter {
    address public immutable override WETH;
    address public immutable usdc;
    constructor(address _wmon,address _usdc) {WETH=_wmon;usdc=_usdc;}
    function getAmountsOut(uint256 amount,address[] calldata path) public view returns(uint256[] memory q) {
        q=new uint256[](2);
        q[0]=amount;
        q[1]=path[0]==usdc ? amount*1e12/4 : amount*4/1e12;
    }
    function swapExactTokensForTokens(uint256 amount,uint256 minOut,address[] calldata path,address to,uint256)
      external returns(uint256[] memory q)
    {
        q=getAmountsOut(amount,path);
        require(q[1]>=minOut,"min");
        MockCoin(path[0]).transferFrom(msg.sender,address(this),amount);
        MockCoin(path[1]).transfer(to,q[1]);
    }
}
contract TinyMonadWalletShowcaseTest {
    VmTrader constant vm=VmTrader(address(uint160(uint256(keccak256("hevm cheat code")))));
    address constant USER=address(0xBEEF);
    MockCoin usdc;
    MockCoin wmon;
    ShowcaseWalletRouter router;
    TinyMonadPolicy model;
    TinyMonadWalletShowcase demo;

    function setUp() public {
        vm.chainId(10143);
        vm.warp(1800000000);
        usdc=new MockCoin();
        wmon=new MockCoin();
        router=new ShowcaseWalletRouter(address(wmon),address(usdc));
        model=new TinyMonadPolicy();
        demo=new TinyMonadWalletShowcase(address(model),address(router),address(usdc),address(wmon));
        usdc.mint(USER,2_000_000);
        wmon.mint(address(router),10 ether);
        usdc.mint(address(router),100_000_000);
        vm.prank(USER);usdc.approve(address(demo),800_000);
        vm.prank(USER);wmon.approve(address(demo),2e17);
    }
    function vector(uint8 action) private pure returns(int32[12] memory input) {
        if(action==1) return [int32(5051),-10891,-11697,-8551,-7757,7702,5001,-9463,-9922,10685,6590,5671];
        if(action==2) return [int32(6013),4669,3793,1865,-2474,1977,5977,4629,1041,554,375,3323];
        return [int32(-251),951,1551,-2262,-421,-2252,-292,1317,-642,-1298,-1447,-5488];
    }
    function testSixRoundsFromEOAWallet() public {
        uint8[6] memory plan=[uint8(1),0,2,1,0,2];
        for(uint8 i=0;i<6;i++) {
            vm.warp(1800000000 + uint256(i)*10);
            vm.prank(USER);
            (uint8 action,) = demo.execute(vector(plan[i]),i,plan[i]==0?0:plan[i]==1?99e15:396_000);
            require(action==plan[i],"onchain prediction");
        }
        require(demo.nextRound(USER)==6,"round count");
        require(usdc.balanceOf(address(demo))==0 && wmon.balanceOf(address(demo))==0,"contract retains inventory");
        require(usdc.balanceOf(USER)==2_000_000,"USDC round trip");
        require(wmon.balanceOf(USER)==0,"WMON round trip");
        vm.prank(USER);demo.restart();
        require(demo.nextRound(USER)==0 && demo.session(USER)==1,"restart");
    }
    function testCooldownAndUserSeparation() public {
        vm.prank(USER);demo.execute(vector(0),0,0);
        vm.prank(USER);
        (bool ok,)=address(demo).call(abi.encodeCall(demo.execute,(vector(0),uint8(1),uint256(0))));
        require(!ok,"cooldown missed");
        require(demo.nextRound(address(this))==0,"other wallet step");
    }
    function testInsufficientAllowanceDoesNotConsumeRound() public {
        MockCoin another=new MockCoin(); // account with no wallet inventory
        require(address(another)!=address(0));
        (bool ok,)=address(demo).call(abi.encodeCall(demo.execute,(vector(1),uint8(0),uint256(99e15))));
        require(!ok,"unfunded account bought");
        require(demo.nextRound(address(this))==0,"advanced");
    }
}
