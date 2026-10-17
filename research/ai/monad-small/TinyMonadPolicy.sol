// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// @notice Small policy trained on archived GeckoTerminal Monad mainnet candles.
/// @dev Caller supplies standardized Q12 inputs. No data oracle, custody or swaps.
contract TinyMonadPolicy {
    bytes32 public constant DATASET_SHA256 = hex"06a2f2f4605041b7b1656accf9e1d9bf0bb95bb3b46295288ae5481f3c2cb52f";
    bytes32 public constant MODEL_SHA256 = hex"183fa936ef593253276e4baa3fa8ecb6e1a5334f6dd2df07e38235cd9b8cc8b7";
    uint256 public constant HIDDEN = 8;
    bytes private constant PARAMETERS = hex"3d0f5bf093f6da0f7501ecf2b40564036f06deff7af218eb351df7fec402a6160001e9f33922980c10fc02054b060e02c9045d0baa11f6116526a310adf50b05c60aa70656fb46051811e3fd8ef632f5a4cef0fc560b37055be8e2efb12a50079bf0c50e370f021bfbf451e81bee5afd0616a70cf11632fb8706c2fce1fa5de62df0bdee2206e6fa05e1c5d095d9a4ea3000b5f95efc75f668fee0fe36faeffe80f6ac04a720dce29104cd03f0073307e0fdacffd204a30e930ac00543f3fefcd8ab5100b75db3005967c400dbf653014df384001fb0ea004c954803a41ab8fedd10f502e7f090eddcf6dc2996db260ec0ed1bf6371e6327e3e389e9cb0eef1bdd08d91e0afe771df01adff156058f00f0f1e400a2fa7dff9cd895fe";
    event Decision(address indexed caller, bytes32 indexed inputHash, uint8 action, int256 score0, int256 score1, int256 score2);

    function _dense(int256[] memory x, bytes memory blob, uint256 offset, uint256 rows)
        private pure returns (int256[] memory y, uint256 next)
    {
        y = new int256[](rows);
        uint256 cols = x.length;
        uint256 biasOffset = offset + rows * cols * 2;
        for (uint256 r; r < rows; r++) {
            int256 total;
            assembly ("memory-safe") {
                let bp := add(add(add(blob, 32), biasOffset), mul(r, 4))
                let word := mload(bp)
                total := signextend(3, or(or(byte(0, word), shl(8, byte(1, word))), or(shl(16, byte(2, word)), shl(24, byte(3, word)))))
                let wp := add(add(add(blob, 32), offset), mul(mul(r, cols), 2))
                let xp := add(x, 32)
                let end := add(xp, mul(cols, 32))
                for {} lt(xp, end) { xp := add(xp, 32) wp := add(wp, 2) } {
                    let w := mload(wp)
                    total := add(total, mul(signextend(1, or(byte(0, w), shl(8, byte(1, w)))), mload(xp)))
                }
            }
            y[r] = total / 16384;
        }
        next = biasOffset + rows * 4;
    }

    function predict(int32[12] calldata input) public pure returns (int256[3] memory scores, uint8 action) {
        int256[] memory x = new int256[](12);
        for (uint256 i; i < 12; i++) {
            require(input[i] >= -16384 && input[i] <= 16384, "input bound");
            x[i] = input[i];
        }
        bytes memory blob = PARAMETERS;
        uint256 offset;
        if (HIDDEN != 0) {
            (x, offset) = _dense(x, blob, 0, HIDDEN);
            for (uint256 i; i < HIDDEN; i++) if (x[i] < 0) x[i] = 0;
        }
        (x,) = _dense(x, blob, offset, 3);
        scores = [x[0], x[1], x[2]];
        for (uint8 i = 1; i < 3; i++) if (scores[i] > scores[action]) action = i;
    }

    function decide(int32[12] calldata input) external returns (int256[3] memory scores, uint8 action) {
        (scores, action) = predict(input);
        emit Decision(msg.sender, keccak256(abi.encode(input)), action, scores[0], scores[1], scores[2]);
    }
}
