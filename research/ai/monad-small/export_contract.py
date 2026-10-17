"""Embed the selected tiny model without changing its integer semantics."""
import hashlib
import json
import os
from pathlib import Path
import struct

ROOT = Path(os.environ.get("MONAD_AI_RUN_DIR", Path(__file__).resolve().parent)).resolve()
model_bytes = (ROOT / "model.json").read_bytes()
model = json.loads(model_bytes)
hidden = model["architecture"][1] if len(model["layers"]) == 2 else 0
blob = b""
for layer in model["layers"]:
    for row in layer["weights"]:
        blob += struct.pack("<" + "h" * len(row), *row)
    blob += struct.pack("<" + "i" * len(layer["bias"]), *layer["bias"])
source = '''// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// @notice Small policy trained on archived GeckoTerminal Monad mainnet candles.
/// @dev Caller supplies standardized Q12 inputs. No data oracle, custody or swaps.
contract TinyMonadPolicy {
    bytes32 public constant DATASET_SHA256 = hex"DATA_HASH";
    bytes32 public constant MODEL_SHA256 = hex"MODEL_HASH";
    uint256 public constant HIDDEN = HIDDEN_WIDTH;
    bytes private constant PARAMETERS = hex"MODEL_BYTES";
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
'''.replace("DATA_HASH", model["dataset_sha256"]).replace("MODEL_HASH", hashlib.sha256(model_bytes).hexdigest()).replace("HIDDEN_WIDTH", str(hidden)).replace("MODEL_BYTES", blob.hex())
(ROOT.parent / "evm/src/TinyMonadPolicy.sol").write_text(source)
vectors = json.loads((ROOT / "vectors.json").read_text())
test_rows = []
for i, vector in enumerate(vectors):
    assignment = "\n".join(f"        input[{j}] = {value};" for j, value in enumerate(vector["input_q12"]))
    checks = "\n".join(f'        require(scores[{j}] == {value}, "score-{j}");' for j, value in enumerate(vector["scores_q12"]))
    test_rows.append(f'''    function testRealVector{i}() public {{
        int32[12] memory input;
{assignment}
        (int256[3] memory scores, uint8 action) = model.predict(input);
{checks}
        require(action == {vector["action"]}, "action");
    }}''')
tests = '''// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "../src/TinyMonadPolicy.sol";
contract TinyMonadPolicyTest {
    TinyMonadPolicy model;
    function setUp() public { model = new TinyMonadPolicy(); }
TEST_ROWS
    function testRejectOutOfBoundInput() public {
        int32[12] memory input;
        input[0] = 16385;
        (bool ok,) = address(model).call(abi.encodeCall(model.predict, (input)));
        require(!ok, "out of bound accepted");
    }
}
'''.replace("TEST_ROWS", "\n".join(test_rows))
(ROOT.parent / "evm/test/TinyMonadPolicy.t.sol").write_text(tests)
print(json.dumps({"parameter_bytes": len(blob), "vectors": len(vectors), "hidden": hidden}))
