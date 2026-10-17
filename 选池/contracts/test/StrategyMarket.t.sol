// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "../src/StrategyMarket.sol";
interface MarketVm {
    function deal(address who,uint256 amount) external;
    function prank(address who) external;
    function warp(uint256 timestamp) external;
    function expectRevert(bytes calldata reason) external;
}
contract StrategyMarketTest {
    MarketVm constant vm=MarketVm(address(uint160(uint256(keccak256("hevm cheat code")))));
    StrategyMarket market;
    address constant BUYER=address(123);
    address constant TENANT=address(456);
    bool reject;
    receive() external payable { require(!reject); }
    function setUp() public {
        bytes32[] memory ids=new bytes32[](1);ids[0]=bytes32(uint256(1));
        market=new StrategyMarket(ids,1 ether,0.1 ether,30 days,10 ether);
        vm.deal(BUYER,100 ether);vm.deal(TENANT,100 ether);
    }
    function useAs(address who) private {
        MonUsdcQuoteSelector.Context memory c=MonUsdcQuoteSelector.Context(143,1,bytes32(uint256(1)),address(0),0x754704Bc059F8C67012fEd69BC8A327a5aafb603,1);
        MonUsdcQuoteSelector.Quote[] memory q=new MonUsdcQuoteSelector.Quote[](1);q[0]=MonUsdcQuoteSelector.Quote(bytes32(uint256(1)),123);
        vm.prank(who);market.select(c,q);
    }
    function testBuyPermanentAndPay() public {
        uint256 beforeBalance=address(this).balance;
        vm.prank(BUYER);market.buy{value:1 ether}();
        require(market.permanent(BUYER)&&address(this).balance==beforeBalance+1 ether);
        vm.warp(block.timestamp+365 days);useAs(BUYER);
    }
    function testRentExpiryAndRenewal() public {
        vm.prank(TENANT);market.rent{value:0.1 ether}();
        uint256 expiry=market.rentedUntil(TENANT);useAs(TENANT);
        vm.prank(TENANT);market.rent{value:0.1 ether}();require(market.rentedUntil(TENANT)==expiry+30 days);
        vm.warp(expiry+30 days);require(!market.hasAccess(TENANT));
        vm.expectRevert(bytes("license required"));useAs(TENANT);
        vm.prank(TENANT);market.rent{value:0.1 ether}();require(market.rentedUntil(TENANT)==block.timestamp+30 days);
    }
    function testOwnershipTransfersFutureRevenueAndPreservesLicenses() public {
        vm.prank(TENANT);market.rent{value:0.1 ether}();
        vm.prank(BUYER);market.buyOwnership{value:10 ether}();
        require(market.owner()==BUYER&&market.ownershipPrice()==0&&market.hasAccess(address(this)));
        useAs(TENANT);useAs(BUYER);
        uint256 beforeBalance=BUYER.balance;
        vm.prank(TENANT);market.buy{value:1 ether}();require(BUYER.balance==beforeBalance+1 ether);
        vm.expectRevert(bytes("owner only"));market.listOwnership(2 ether);
        vm.prank(BUYER);market.listOwnership(20 ether);require(market.ownershipPrice()==20 ether);
    }
    function testWrongPaymentAndDuplicateBuy() public {
        vm.expectRevert(bytes("exact MON price required"));vm.prank(BUYER);market.buy{value:0.9 ether}();
        vm.prank(BUYER);market.buy{value:1 ether}();
        vm.expectRevert(bytes("already entitled"));vm.prank(BUYER);market.buy{value:1 ether}();
        vm.expectRevert(bytes("already entitled"));vm.prank(BUYER);market.rent{value:0.1 ether}();
    }
    function testDelistedCannotBePurchased() public {
        market.listOwnership(0);
        vm.expectRevert(bytes("not for sale"));vm.prank(BUYER);market.buyOwnership{value:10 ether}();
    }
    function testRejectedPaymentRollsBackOwnershipAndLicense() public {
        reject=true;
        vm.expectRevert(bytes("MON transfer failed"));vm.prank(BUYER);market.buyOwnership{value:10 ether}();
        require(market.owner()==address(this)&&market.ownershipPrice()==10 ether);
        vm.expectRevert(bytes("MON transfer failed"));vm.prank(BUYER);market.buy{value:1 ether}();
        require(!market.permanent(BUYER)&&BUYER.balance==100 ether);
    }
}
