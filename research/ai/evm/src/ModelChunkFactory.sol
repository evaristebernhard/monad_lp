// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// @notice Content-addressed code storage, adapted from Neurom ModuleFactory (MIT).
/// @dev A callback avoids the EIP-3860 initcode size limit for large weight chunks.
contract ModelChunkFactory {
    uint256 public constant MAX_CHUNK_BYTES = 114688;
    bytes public constant CHUNK_INIT = hex"63151ce8865f525f5f6004601c335afa6016575f5ffd5b3d5f5f3e3d5ff3";
    uint256 private constant LENGTH_SLOT = uint256(keccak256("model.chunk.length")) - 1;
    uint256 private constant DATA_SLOT = uint256(keccak256("model.chunk.data")) - 1;
    event ChunkCreated(bytes32 indexed id, address chunk, uint256 size);

    function chunkAddress(bytes32 id) public view returns (address) {
        return address(uint160(uint256(keccak256(abi.encodePacked(
            bytes1(0xff), address(this), id, keccak256(CHUNK_INIT)
        )))));
    }

    function createChunk(bytes calldata data) external returns (address chunk) {
        require(data.length > 0 && data.length <= MAX_CHUNK_BYTES, "chunk length");
        bytes32 id = keccak256(data);
        chunk = chunkAddress(id);
        if (chunk.code.length != 0) return chunk;
        uint256 len = data.length;
        (uint256 lengthSlot, uint256 dataSlot) = (LENGTH_SLOT, DATA_SLOT);
        assembly ("memory-safe") {
            tstore(lengthSlot, len)
            for { let i := 0 } lt(shl(5, i), len) { i := add(i, 1) } {
                tstore(add(dataSlot, i), calldataload(add(data.offset, shl(5, i))))
            }
        }
        bytes memory init = CHUNK_INIT;
        address created;
        assembly ("memory-safe") {
            created := create2(0, add(init, 32), mload(init), id)
            tstore(lengthSlot, 0)
        }
        require(created == chunk && created.code.length == len + 1, "create");
        emit ChunkCreated(id, chunk, len);
    }

    function chunkData() external view {
        (uint256 lengthSlot, uint256 dataSlot) = (LENGTH_SLOT, DATA_SLOT);
        assembly {
            let len := tload(lengthSlot)
            let p := mload(0x40)
            mstore8(p, 0)
            for { let i := 0 } lt(shl(5, i), len) { i := add(i, 1) } {
                mstore(add(add(p, 1), shl(5, i)), tload(add(dataSlot, i)))
            }
            return(p, add(len, 1))
        }
    }
}
