// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "./ExperimentalPpoActor.sol";

/// @notice Records model decisions on supplied inputs. No swaps or asset custody.
/// @dev Inputs are public and caller-supplied, with no market-data authentication.
contract PpoDecisionRecorder {
    ExperimentalPpoActor public immutable actor;
    event Decision(address indexed caller, bytes32 indexed inputHash, int256 raw0, int256 raw1, uint8 action, uint256 sizeQ16);

    constructor(ExperimentalPpoActor actor_) {
        require(address(actor_).code.length != 0, "actor");
        actor = actor_;
    }

    function decide(bytes calldata packed) external returns (int256[2] memory raw, uint8 action, uint256 sizeQ16) {
        (raw, action, sizeQ16) = actor.predict(packed);
        emit Decision(msg.sender, keccak256(packed), raw[0], raw[1], action, sizeQ16);
    }
}
