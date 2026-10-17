// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "../src/PaidMonUsdcRouteSelector.sol";
interface FeeVm {
    function deal(address who, uint256 amount) external;
    function prank(address who) external;
    function expectRevert(bytes calldata reason) external;
}
contract PaidMonUsdcRouteSelectorTest {
    FeeVm constant vm = FeeVm(address(uint160(uint256(keccak256("hevm cheat code")))));
    PaidMonUsdcRouteSelector paid;
    address constant USER = address(123);
    bool rejectPayment;
    receive() external payable { require(!rejectPayment, "reject"); }
    function setUp() public {
        bytes32[] memory ids = new bytes32[](1); ids[0] = bytes32(uint256(1));
        paid = new PaidMonUsdcRouteSelector(ids, 0.01 ether);
        vm.deal(USER, 1 ether);
    }
    function rows() private pure returns (MonUsdcQuoteSelector.Quote[] memory q) {
        q = new MonUsdcQuoteSelector.Quote[](1);
        q[0] = MonUsdcQuoteSelector.Quote(bytes32(uint256(1)), 100);
    }
    function context() private pure returns (MonUsdcQuoteSelector.Context memory) {
        return MonUsdcQuoteSelector.Context(143, 1, bytes32(uint256(1)), address(0),
            0x754704Bc059F8C67012fEd69BC8A327a5aafb603, 1 ether);
    }
    function testFeeReachesCreator() public {
        uint256 beforeBalance = address(this).balance;
        vm.prank(USER);
        (bytes32 id, uint256 output,) = paid.selectAndPay{value: 0.01 ether}(context(), rows());
        require(id == bytes32(uint256(1)) && output == 100);
        require(address(this).balance == beforeBalance + 0.01 ether);
        require(USER.balance == 0.99 ether && address(paid).balance == 0);
        require(paid.creator() == address(this));
    }
    function testRejectMissingFee() public {
        vm.expectRevert(bytes("exact MON fee required"));
        paid.selectAndPay(context(), rows());
    }
    function testRejectExcessFee() public {
        vm.expectRevert(bytes("exact MON fee required")); vm.prank(USER);
        paid.selectAndPay{value: 0.02 ether}(context(), rows());
    }
    function testBadQuoteRollsBackPayment() public {
        MonUsdcQuoteSelector.Quote[] memory q = rows(); q[0].amountOut = 0;
        vm.expectRevert(bytes("no quote")); vm.prank(USER);
        paid.selectAndPay{value: 0.01 ether}(context(), q);
        require(USER.balance == 1 ether && address(paid).balance == 0);
    }
    function testRejectedTransferRollsBack() public {
        rejectPayment = true;
        vm.expectRevert(bytes("fee transfer failed")); vm.prank(USER);
        paid.selectAndPay{value: 0.01 ether}(context(), rows());
        require(USER.balance == 1 ether && address(paid).balance == 0);
    }
}
