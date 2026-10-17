// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "../src/TinyMonadSessionTrader.sol";
import "../src/TinyMonadPolicy.sol";
import "./mocks/WalletMocks.sol";

contract TinyMonadSessionTraderTest {
    VmTrader constant vm = VmTrader(address(uint160(uint256(keccak256("hevm cheat code")))));
    address constant USER = address(0xBEEF);
    address constant KEEPER = address(0xCAFE);
    MockCoin stable;
    MockCoin wrapped;
    ShowcaseWalletRouter router;
    TinyMonadPolicy policy;
    TinyMonadSessionTrader trader;

    function setUp() public {
        vm.chainId(10143);vm.warp(1800000000);
        stable=new MockCoin();wrapped=new MockCoin();
        router=new ShowcaseWalletRouter(address(wrapped),address(stable));
        policy=new TinyMonadPolicy();
        trader=new TinyMonadSessionTrader(address(policy),address(router),address(stable),address(wrapped),KEEPER);
        stable.mint(USER,2_000_000);
        wrapped.mint(address(router),10 ether);
        stable.mint(address(router),50_000_000);
        vm.prank(USER);stable.approve(address(trader),800_000);
        vm.prank(USER);wrapped.approve(address(trader),200_000_000_000_000_000);
    }
    function input(uint8 round) public pure returns (int32[12] memory x) {
        if(round==0) return [int32(5051),-10891,-11697,-8551,-7757,7702,5001,-9463,-9922,10685,6590,5671];
        if(round==1) return [int32(-251),951,1551,-2262,-421,-2252,-292,1317,-642,-1298,-1447,-5488];
        if(round==2||round==5) return [int32(6013),4669,3793,1865,-2474,1977,5977,4629,1041,554,375,3323];
        if(round==3) return [int32(2684),1454,2123,1595,-336,821,2620,2220,1379,-1707,-2085,5049];
        return [int32(-1166),-311,1769,-1485,-859,-1747,-1190,-211,-1124,-1482,-1429,-1831];
    }
    function testSessionSixAutomaticKeeperExecutions() public {
        vm.prank(USER);trader.start();
        (uint64 sessionId,,,,)=trader.sessions(USER);
        for(uint8 i=0;i<6;i++){
            vm.warp(1800000000+uint256(i)*10);
            vm.prank(KEEPER);
            (uint8 action,) = trader.executeFor(USER,sessionId,i,input(i),
                i==1||i==4?0:i==0||i==3?99e15:396_000);
            require(action==(i==0||i==3?1:i==2||i==5?2:0),"wrong model prediction");
        }
        (, , ,uint8 completed,bool active)=trader.sessions(USER);
        require(completed==6 && !active,"six completed");
        require(stable.balanceOf(USER)==2_000_000,"cash restored");
        require(wrapped.balanceOf(USER)==0,"wrapped restored");
        require(stable.balanceOf(address(trader))==0 && wrapped.balanceOf(address(trader))==0,"funds not retained");
        vm.prank(USER);stable.approve(address(trader),800_000);
        vm.prank(USER);wrapped.approve(address(trader),200_000_000_000_000_000);
        vm.prank(USER);trader.start();
        (uint64 nextId,,,,)=trader.sessions(USER);
        require(nextId==sessionId+1,"new session");
    }
    function testCannotSpendWithoutSessionAndCannotForgeFeatures() public {
        vm.prank(KEEPER);
        (bool ok,)=address(trader).call(abi.encodeCall(trader.executeFor,(USER,uint64(1),uint8(0),input(0),99e15)));
        require(!ok,"spent without Start");
        vm.prank(USER);trader.start();
        vm.prank(KEEPER);
        (ok,)=address(trader).call(abi.encodeCall(trader.executeFor,(USER,uint64(1),uint8(0),input(2),396_000)));
        require(!ok,"keeper changed decision");
        vm.prank(address(0xAB));
        (ok,)=address(trader).call(abi.encodeCall(trader.executeFor,(USER,uint64(1),uint8(0),input(0),99e15)));
        require(!ok,"untrusted relay");
    }
    function testCannotRunTooFastAndCancelStopsKeeper() public {
        vm.prank(USER);trader.start();
        vm.prank(KEEPER);trader.executeFor(USER,1,0,input(0),99e15);
        vm.prank(KEEPER);
        (bool ok,)=address(trader).call(abi.encodeCall(trader.executeFor,(USER,uint64(1),uint8(1),input(1),0)));
        require(!ok,"cooldown");
        vm.prank(USER);trader.cancel();
        vm.warp(1800000011);
        vm.prank(KEEPER);
        (ok,)=address(trader).call(abi.encodeCall(trader.executeFor,(USER,uint64(1),uint8(1),input(1),0)));
        require(!ok,"cancel failed");
    }
    function testExpiredSessionCannotTradeOrReplay() public {
        vm.prank(USER);trader.start();
        vm.warp(1800000111);
        vm.prank(KEEPER);
        (bool ok,)=address(trader).call(abi.encodeCall(trader.executeFor,(USER,uint64(1),uint8(0),input(0),99e15)));
        require(!ok,"expired session");
        vm.prank(USER);trader.start();
        vm.prank(KEEPER);
        (ok,)=address(trader).call(abi.encodeCall(trader.executeFor,(USER,uint64(1),uint8(0),input(0),99e15)));
        require(!ok,"old session replay");
    }
}
