// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// @notice Research-only quantized actor. No asset custody or swap capability.
/// @dev Pinned 3008->64->64->2 actor, int16 little-endian weights, int32 biases,
/// Q16 inputs and activations. This is NOT Neurom's threshold/LIF engine.
contract ExperimentalPpoActor {
    address[] private chunks;
    address private immutable tableChunk;
    bytes32 public immutable weightHash;
    bytes32 public immutable tableHash;
    int256 private constant Q = 65536;

    constructor(address[] memory weights, address table, bytes32 expectedWeights, bytes32 expectedTable) {
        chunks = weights;
        tableChunk = table;
        require(keccak256(_loadWeights()) == expectedWeights, "weights");
        require(keccak256(_codeData(table)) == expectedTable, "table");
        weightHash = expectedWeights;
        tableHash = expectedTable;
    }

    function _codeData(address chunk) private view returns (bytes memory data) {
        uint256 size = chunk.code.length;
        require(size > 1, "chunk");
        data = new bytes(size - 1);
        assembly ("memory-safe") { extcodecopy(chunk, add(data, 32), 1, sub(size, 1)) }
    }

    function _loadWeights() private view returns (bytes memory data) {
        uint256 size;
        for (uint256 i; i < chunks.length; i++) {
            require(chunks[i].code.length > 1, "chunk");
            size += chunks[i].code.length - 1;
        }
        require(size == 393992, "weight length");
        data = new bytes(size);
        uint256 at;
        for (uint256 i; i < chunks.length; i++) {
            address chunk = chunks[i];
            uint256 len = chunk.code.length - 1;
            assembly ("memory-safe") { extcodecopy(chunk, add(add(data, 32), at), 1, len) }
            at += len;
        }
    }

    function _i32(bytes memory data, uint256 at) private pure returns (int256 value) {
        assembly ("memory-safe") {
            let word := mload(add(add(data, 32), at))
            value := signextend(3, or(or(byte(0, word), shl(8, byte(1, word))), or(shl(16, byte(2, word)), shl(24, byte(3, word)))))
        }
    }

    function _dense(int256[] memory x, bytes memory blob, uint256 at, uint256 biasAt, uint256 rows, uint256 shift)
        private pure returns (int256[] memory y)
    {
        y = new int256[](rows);
        assembly ("memory-safe") {
            let cols := mload(x)
            let base := add(add(blob, 32), at)
            let denom := shl(shift, 1)
            for { let row := 0 } lt(row, rows) { row := add(row, 1) } {
                let wp := add(base, mul(mul(row, cols), 2))
                let xp := add(x, 32)
                let end := add(xp, mul(cols, 32))
                let acc := 0
                // All three pinned layer widths are divisible by four.
                // Decode four weights per memory read and loop iteration.
                for {} lt(xp, end) { xp := add(xp, 128) wp := add(wp, 8) } {
                    let word := mload(wp)
                    acc := add(acc, mul(signextend(1, or(byte(0, word), shl(8, byte(1, word)))), mload(xp)))
                    acc := add(acc, mul(signextend(1, or(byte(2, word), shl(8, byte(3, word)))), mload(add(xp, 32))))
                    acc := add(acc, mul(signextend(1, or(byte(4, word), shl(8, byte(5, word)))), mload(add(xp, 64))))
                    acc := add(acc, mul(signextend(1, or(byte(6, word), shl(8, byte(7, word)))), mload(add(xp, 96))))
                }
                let word := mload(add(add(add(blob, 32), biasAt), mul(row, 4)))
                let bias := signextend(3, or(or(byte(0, word), shl(8, byte(1, word))), or(shl(16, byte(2, word)), shl(24, byte(3, word)))))
                mstore(add(add(y, 32), mul(row, 32)), add(sdiv(acc, denom), bias))
            }
        }
    }

    function _activate(int256[] memory x, bytes memory table) private pure {
        for (uint256 i; i < x.length; i++) {
            int256 value = x[i];
            int256 sign = value < 0 ? int256(-1) : int256(1);
            uint256 mag = uint256(value < 0 ? -value : value);
            if (mag >= 8 * uint256(Q)) { x[i] = sign * Q; continue; }
            uint256 index = mag / 256;
            int256 low = _i32(table, index * 4);
            int256 high = _i32(table, (index + 1) * 4);
            x[i] = sign * (low + (high - low) * int256(mag % 256) / 256);
        }
    }

    /// @param packed Exactly 3008 int32 little-endian, already-normalized Q16 inputs.
    /// @return raw Q16 continuous output, before action clipping and truncation.
    function predict(bytes calldata packed) external view returns (int256[2] memory raw, uint8 action, uint256 sizeQ16) {
        require(packed.length == 12032, "input length");
        bytes memory input = packed;
        int256[] memory x = new int256[](3008);
        for (uint256 i; i < x.length; i++) {
            x[i] = _i32(input, i * 4);
            require(x[i] >= -8 * Q && x[i] <= 8 * Q, "input bound");
        }
        bytes memory blob = _loadWeights();
        bytes memory table = _codeData(tableChunk);
        require(table.length == 8196, "table length");
        x = _dense(x, blob, 0, 385024, 64, 16);
        _activate(x, table);
        x = _dense(x, blob, 385280, 393472, 64, 15);
        _activate(x, table);
        x = _dense(x, blob, 393728, 393984, 2, 17);
        raw = [x[0], x[1]];
        action = uint8(uint256(x[0] <= 0 ? int256(0) : x[0] >= 2 * Q ? 2 * Q : x[0]) / uint256(Q));
        sizeQ16 = uint256(x[1] <= 0 ? int256(0) : x[1] >= Q ? Q : x[1]);
    }
}
