// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "../src/ExperimentalPpoActor.sol";

interface Vm {
    function readFileBinary(string calldata path) external view returns (bytes memory);
    function readFile(string calldata path) external view returns (string memory);
    function parseJsonInt(string calldata json, string calldata key) external pure returns (int256);
    function etch(address target, bytes calldata code) external;
    function toString(uint256 value) external pure returns (string memory);
    function expectRevert(bytes calldata reason) external;
}

contract ExperimentalPpoActorTest {
    Vm private constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));
    ExperimentalPpoActor private actor;
    event log_named_uint(string key, uint256 value);

    function setUp() public {
        bytes memory blob = vm.readFileBinary("../model-study/actor-int16.bin");
        bytes memory table = vm.readFileBinary("../model-study/tanh-q16.bin");
        uint256 chunkSize = 114688;
        uint256 count = (blob.length + chunkSize - 1) / chunkSize;
        address[] memory chunks = new address[](count);
        for (uint256 i; i < count; i++) {
            uint256 offset = i * chunkSize;
            uint256 len = blob.length - offset;
            if (len > chunkSize) len = chunkSize;
            bytes memory code = new bytes(len + 1);
            assembly ("memory-safe") {
                for { let j := 0 } lt(j, len) { j := add(j, 32) } {
                    mstore(add(add(code, 33), j), mload(add(add(blob, 32), add(offset, j))))
                }
            }
            address target = address(uint160(0x1000 + i));
            vm.etch(target, code);
            chunks[i] = target;
        }
        address lookup = address(0x2000);
        vm.etch(lookup, bytes.concat(hex"00", table));
        actor = new ExperimentalPpoActor(chunks, lookup, keccak256(blob), keccak256(table));
    }

    function _vector(uint256 index) private {
        string memory base = string.concat("../model-study/evm-vectors/", vm.toString(index));
        bytes memory observation = vm.readFileBinary(string.concat(base, ".bin"));
        string memory expected = vm.readFile(string.concat(base, ".json"));
        uint256 beforeGas = gasleft();
        (int256[2] memory output, uint8 action, uint256 size) = actor.predict(observation);
        emit log_named_uint("inference_call_gas", beforeGas - gasleft());
        require(output[0] == vm.parseJsonInt(expected, ".raw0"), "raw0 parity");
        require(output[1] == vm.parseJsonInt(expected, ".raw1"), "raw1 parity");
        require(action == uint256(vm.parseJsonInt(expected, ".action")), "action parity");
        require(size == uint256(vm.parseJsonInt(expected, ".size_q16")), "size parity");
    }

    function testZero() public { _vector(0); }
    function testInitialPortfolio() public { _vector(1); }
    function testPositive() public { _vector(2); }
    function testNegative() public { _vector(3); }
    function testBuy() public { _vector(4); }
    function testSell() public { _vector(5); }
    function testWrongLength() public {
        vm.expectRevert(bytes("input length"));
        actor.predict(new bytes(32));
    }
    function testOutOfBounds() public {
        bytes memory observation = new bytes(12032);
        // 524289 is above +8 in Q16. Little-endian int32.
        observation[0] = 0x01; observation[2] = 0x08;
        vm.expectRevert(bytes("input bound"));
        actor.predict(observation);
    }
}
