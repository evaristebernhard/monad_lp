// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "../src/ModelChunkFactory.sol";
import "../src/PpoDecisionRecorder.sol";

contract ModelChunkFactoryTest {
    function testRealLargeChunkCreationAndReuse() public {
        ModelChunkFactory factory = new ModelChunkFactory();
        bytes memory data = new bytes(114688);
        data[0] = 0xab;
        data[data.length - 1] = 0xcd;
        address chunk = factory.createChunk(data);
        require(chunk == factory.chunkAddress(keccak256(data)), "address");
        require(keccak256(chunk.code) == keccak256(bytes.concat(hex"00", data)), "code");
        require(factory.createChunk(data) == chunk, "reuse");
    }

    function testChunkDataClearedAfterCreation() public {
        ModelChunkFactory factory = new ModelChunkFactory();
        factory.createChunk(hex"010203");
        (bool ok, bytes memory data) = address(factory).staticcall(abi.encodeCall(factory.chunkData, ()));
        require(ok && data.length == 1 && data[0] == 0, "pending cleared");
    }
}
