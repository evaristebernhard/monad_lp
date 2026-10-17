// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "./MonUsdcRouteSelector.sol";

/// @notice Testnet strategy licensing and ownership marketplace.
/// @dev Licenses authorize this entry point, not exclusive access to public code.
/// Caller supplies quotes; this contract does not authenticate prices or swap.
contract StrategyMarket {
    address payable public owner;
    MonUsdcRouteSelector public immutable selector;
    uint256 public immutable buyPrice;
    uint256 public immutable rentPrice;
    uint256 public immutable rentDuration;
    uint256 public ownershipPrice;
    mapping(address => bool) public permanent;
    mapping(address => uint256) public rentedUntil;
    bool private entered;

    event LicensePurchased(address indexed buyer, address indexed recipient, uint256 price);
    event LicenseRented(address indexed buyer, address indexed recipient, uint256 price, uint256 expiresAt);
    event OwnershipPurchased(address indexed previousOwner, address indexed newOwner, uint256 price);
    event OwnershipListed(uint256 price);
    event StrategyUsed(address indexed caller, bytes32 poolId, uint256 amountOut, bytes32 replayHash);

    modifier guarded() {
        require(!entered, "reentrant"); entered = true; _; entered = false;
    }
    constructor(bytes32[] memory ids, uint256 buy_, uint256 rent_, uint256 duration_, uint256 own_) {
        require(buy_ > 0 && rent_ > 0 && duration_ > 0, "invalid terms");
        owner = payable(msg.sender);
        selector = new MonUsdcRouteSelector(ids);
        buyPrice = buy_; rentPrice = rent_; rentDuration = duration_; ownershipPrice = own_;
    }
    function hasAccess(address user) public view returns (bool) {
        return user == owner || permanent[user] || rentedUntil[user] > block.timestamp;
    }
    function buy() external payable guarded {
        require(!permanent[msg.sender] && msg.sender != owner, "already entitled");
        require(msg.value == buyPrice, "exact MON price required");
        permanent[msg.sender] = true;
        address payable recipient = owner;
        pay(recipient);
        emit LicensePurchased(msg.sender, recipient, msg.value);
    }
    function rent() external payable guarded {
        require(!permanent[msg.sender] && msg.sender != owner, "already entitled");
        require(msg.value == rentPrice, "exact MON price required");
        uint256 start = rentedUntil[msg.sender] > block.timestamp ? rentedUntil[msg.sender] : block.timestamp;
        rentedUntil[msg.sender] = start + rentDuration;
        address payable recipient = owner;
        pay(recipient);
        emit LicenseRented(msg.sender, recipient, msg.value, rentedUntil[msg.sender]);
    }
    function buyOwnership() external payable guarded {
        require(msg.sender != owner, "already owner");
        require(ownershipPrice > 0, "not for sale");
        require(msg.value == ownershipPrice, "exact MON price required");
        address payable previous = owner;
        // Seller keeps permanent access. Existing licenses and rentals survive transfer.
        permanent[previous] = true;
        owner = payable(msg.sender);
        ownershipPrice = 0;
        pay(previous);
        emit OwnershipPurchased(previous, msg.sender, msg.value);
    }
    /// @notice Only current owner can list; zero delists. Prices of existing licenses are immutable.
    function listOwnership(uint256 price) external guarded {
        require(msg.sender == owner, "owner only");
        ownershipPrice = price;
        emit OwnershipListed(price);
    }
    function select(MonUsdcQuoteSelector.Context calldata context, MonUsdcQuoteSelector.Quote[] calldata quotes)
        external guarded returns (bytes32 poolId, uint256 amountOut, bytes32 replayHash)
    {
        require(hasAccess(msg.sender), "license required");
        (poolId, amountOut, replayHash) = selector.selectBest(context, quotes);
        emit StrategyUsed(msg.sender, poolId, amountOut, replayHash);
    }
    function pay(address payable recipient) private {
        (bool sent,) = recipient.call{value: msg.value}("");
        require(sent, "MON transfer failed");
    }
}
