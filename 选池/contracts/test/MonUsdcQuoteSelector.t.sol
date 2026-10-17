// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "../src/MonUsdcQuoteSelector.sol";
interface Vm {function expectRevert(bytes calldata reason) external;}
contract MonUsdcQuoteSelectorTest {
    Vm constant vm=Vm(address(uint160(uint256(keccak256("hevm cheat code")))));
    MonUsdcQuoteSelector selector;
    function setUp() public {
        bytes32[] memory ids=new bytes32[](2);ids[0]=bytes32(uint256(1));ids[1]=bytes32(uint256(2));
        selector=new MonUsdcQuoteSelector(ids);
    }
    function context() private pure returns(MonUsdcQuoteSelector.Context memory) {
        return MonUsdcQuoteSelector.Context(143,100,bytes32(uint256(123)),address(0),0x754704Bc059F8C67012fEd69BC8A327a5aafb603,1 ether);
    }
    function quotes(uint256 a,uint256 b) private pure returns(MonUsdcQuoteSelector.Quote[] memory rows) {
        rows=new MonUsdcQuoteSelector.Quote[](2);
        rows[0]=MonUsdcQuoteSelector.Quote(bytes32(uint256(1)),a);
        rows[1]=MonUsdcQuoteSelector.Quote(bytes32(uint256(2)),b);
    }
    function testBestAndReplay() public view {
        (bytes32 pool,uint256 output,bytes32 hash)=selector.selectBest(context(),quotes(10,11));
        require(pool==bytes32(uint256(2)) && output==11 && hash!=bytes32(0));
    }
    function testTiesDoNotDependOnQuoteOrder() public view {
        MonUsdcQuoteSelector.Quote[] memory rows=quotes(11,11);
        (bytes32 first,,)=selector.selectBest(context(),rows);
        (rows[0],rows[1])=(rows[1],rows[0]);
        (bytes32 second,,)=selector.selectBest(context(),rows);
        require(first==bytes32(uint256(1)) && first==second);
    }
    function testDuplicateCannotReplaceMissingPool() public {
        MonUsdcQuoteSelector.Quote[] memory rows=quotes(10,11);rows[1].poolId=rows[0].poolId;
        vm.expectRevert(bytes("duplicate quote"));selector.selectBest(context(),rows);
    }
    function testRejectWrongSourceChain() public {
        MonUsdcQuoteSelector.Context memory c=context();c.sourceChainId=10143;
        vm.expectRevert(bytes("source chain"));selector.selectBest(c,quotes(10,11));
    }
    function testRejectUnknownPool() public {
        MonUsdcQuoteSelector.Quote[] memory rows=quotes(10,11);rows[1].poolId=bytes32(uint256(3));
        vm.expectRevert(bytes("unknown pool"));selector.selectBest(context(),rows);
    }
    function testRejectNoQuote() public {vm.expectRevert(bytes("no quote"));selector.selectBest(context(),quotes(0,0));}
    function testRejectWrongPair() public {
        MonUsdcQuoteSelector.Context memory c=context();c.tokenIn=address(123);
        vm.expectRevert(bytes("pair"));selector.selectBest(c,quotes(10,11));
    }
    function testRejectMissingQuote() public {
        MonUsdcQuoteSelector.Quote[] memory rows=new MonUsdcQuoteSelector.Quote[](1);
        rows[0]=MonUsdcQuoteSelector.Quote(bytes32(uint256(1)),10);
        vm.expectRevert(bytes("complete universe"));selector.selectBest(context(),rows);
    }
}
