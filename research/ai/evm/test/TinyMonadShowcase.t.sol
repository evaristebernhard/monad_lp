// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "../src/TinyMonadShowcase.sol";
import "../src/TinyMonadPolicy.sol";
import "./TinyMonadTrader.t.sol";

contract TinyMonadShowcaseTest {
    VmTrader constant vm=VmTrader(address(uint160(uint256(keccak256("hevm cheat code")))));
    MockCoin usdc;
    MockWrap wmon;
    MockRouter router;
    TinyMonadPolicy policy;
    TinyMonadShowcase showcase;
    address constant KEEPER=address(0xCAFE);
    function setUp() public {
        vm.chainId(10143);
        vm.warp(1800000000);
        usdc=new MockCoin();wmon=new MockWrap();router=new MockRouter(address(wmon));
        policy=new TinyMonadPolicy();
        showcase=new TinyMonadShowcase(address(policy),address(router),address(usdc),address(wmon),KEEPER);
        usdc.mint(address(showcase),2 ether);
        wmon.mint(address(showcase),1 ether);
        usdc.mint(address(router),3 ether);
        wmon.mint(address(router),3 ether);
    }
    function input(uint256 kind) internal pure returns(int32[12] memory x) {
        if(kind==1) return [int32(5051),-10891,-11697,-8551,-7757,7702,5001,-9463,-9922,10685,6590,5671];
        if(kind==0) return [int32(-251),951,1551,-2262,-421,-2252,-292,1317,-642,-1298,-1447,-5488];
        return [int32(6013),4669,3793,1865,-2474,1977,5977,4629,1041,554,375,3323];
    }
    function round(uint8 index,uint8 kind) internal {
        vm.prank(KEEPER);
        (uint8 actual,) = showcase.execute(input(kind), index, kind==0?0:kind==1?388000:97e15);
        require(actual==kind,"wrong policy action");
    }
    function testSixRealModelDecisions() public {
        uint8[6] memory plan=[uint8(1),0,2,1,0,2];
        for(uint8 i=0;i<6;i++){
            vm.warp(1800000000+uint256(i)*10);
            round(i,plan[i]);
        }
        require(showcase.step()==6,"six");
        require(usdc.balanceOf(address(showcase))==2 ether-800000+2*1e17,"stable delta");
        require(wmon.balanceOf(address(showcase))==1 ether+800000-2*1e17,"wrapped delta");
        vm.warp(1800000100);
        vm.prank(KEEPER);
        (bool ok,)=address(showcase).call(abi.encodeCall(showcase.execute,(input(0),uint8(6),uint256(0))));
        require(!ok,"more than six");
        showcase.restart();
        require(showcase.step()==0 && showcase.session()==2,"restart");
    }
    function testNoRepeatAndCooldown() public {
        round(0,0);
        vm.prank(KEEPER);
        (bool ok,)=address(showcase).call(abi.encodeCall(showcase.execute,(input(1),uint8(1),uint256(388000))));
        require(!ok,"cooldown");
        vm.warp(1800000010);
        vm.prank(KEEPER);
        (ok,)=address(showcase).call(abi.encodeCall(showcase.execute,(input(1),uint8(0),uint256(388000))));
        require(!ok,"replay");
    }
    function testNoOtherSenderAndSlippageRevert() public {
        (bool ok,)=address(showcase).call(abi.encodeCall(showcase.execute,(input(1),uint8(0),uint256(388000))));
        require(!ok,"unauthorized");
        vm.prank(KEEPER);
        (ok,)=address(showcase).call(abi.encodeCall(showcase.execute,(input(1),uint8(0),uint256(1))));
        require(!ok,"unsafe slippage");
        require(showcase.step()==0,"failed round advanced");
    }
}
