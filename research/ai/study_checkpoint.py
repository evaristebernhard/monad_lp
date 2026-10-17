"""Inspect pinned weights and test a fixed-point actor, without model pickle execution.

The normalized stress inputs below are synthetic numerical tests, not MON signals.
This script uses only the Python standard library.
"""
import array
import collections
import hashlib
import io
import json
import math
import pickle
import random
import struct
import sys
import zipfile
from pathlib import Path

from verify_pretrained_model import verify

ROOT = Path(__file__).resolve().parents[2]
MODEL = ROOT / "third_party/stock-trading-rl-agent"
OUT = Path(__file__).resolve().parent / "model-study"
Q = 65536
LUT_STEP = 256  # Q16 / 256, step = 1/256 in real units
LUT_BOUND = 8
SEED = 143
SAMPLES = 64


class FloatStorage:
    pass


def rebuild_tensor(storage, offset, shape, stride, *unused):
    return dict(storage=storage, offset=offset, shape=shape, stride=stride)


class TensorReader(pickle.Unpickler):
    def find_class(self, module, name):
        allowed = {
            ("collections", "OrderedDict"): collections.OrderedDict,
            ("torch", "FloatStorage"): FloatStorage,
            ("torch._utils", "_rebuild_tensor_v2"): rebuild_tensor,
        }
        if (module, name) not in allowed:
            raise ValueError(f"Unsupported tensor metadata: {module}.{name}")
        return allowed[module, name]

    def persistent_load(self, value):
        tag, kind, key, location, count = value
        if tag != "storage" or kind is not FloatStorage:
            raise ValueError("Unexpected tensor storage")
        return dict(key=key, location=location, count=count)


class DType:
    def __init__(self, name, *unused):
        if name not in ("O8", "f8", "i8"):
            raise ValueError(f"Unexpected scaler dtype: {name}")
        self.name = name

    def __setstate__(self, state):
        if state[1] not in ("|", "<", "="):
            raise ValueError("Unsupported scaler endianness")


class NDArray:
    def __setstate__(self, state):
        version, shape, dtype, fortran, data = state
        if version != 1 or len(shape) != 1 or fortran:
            raise ValueError("Unexpected scaler array layout")
        if dtype.name == "O8":
            if not isinstance(data, list) or not all(isinstance(v, str) for v in data):
                raise ValueError("Unexpected scaler feature names")
            self.values = data
        else:
            self.values = list(struct.unpack("<" + ("d" if dtype.name == "f8" else "q") * shape[0], data))
        if len(self.values) != shape[0]:
            raise ValueError("Scaler length mismatch")


class Scaler:
    pass


def reconstruct(kind, shape, code):
    if kind is not NDArray:
        raise ValueError("Unexpected scaler array type")
    return NDArray()


def scalar(dtype, data):
    return struct.unpack("<q" if dtype.name == "i8" else "<d", data)[0]


class ScalerReader(pickle.Unpickler):
    def find_class(self, module, name):
        allowed = {
            ("sklearn.preprocessing._data", "StandardScaler"): Scaler,
            ("numpy", "ndarray"): NDArray,
            ("numpy", "dtype"): DType,
            ("numpy.core.multiarray", "_reconstruct"): reconstruct,
            ("numpy.core.multiarray", "scalar"): scalar,
        }
        if (module, name) not in allowed:
            raise ValueError(f"Unsupported scaler metadata: {module}.{name}")
        return allowed[module, name]


def read_weights():
    with zipfile.ZipFile(MODEL / "final_model.zip") as checkpoint:
        metadata = json.loads(checkpoint.read("data"))
        with zipfile.ZipFile(io.BytesIO(checkpoint.read("policy.pth"))) as tensors:
            descriptors = TensorReader(io.BytesIO(tensors.read("archive/data.pkl"))).load()
            weights = {}
            for name, spec in descriptors.items():
                shape = spec["shape"]
                if spec["offset"] != 0 or spec["stride"][-1] != 1:
                    raise ValueError("Unsupported tensor view")
                values = array.array("f")
                values.frombytes(tensors.read("archive/data/" + spec["storage"]["key"]))
                if sys.byteorder != "little":
                    values.byteswap()
                if len(values) != math.prod(shape) or not all(math.isfinite(v) for v in values):
                    raise ValueError("Invalid tensor values")
                weights[name] = dict(shape=list(shape), values=values)
    return metadata, weights


def layers_from(weights):
    names = ["mlp_extractor.policy_net.0", "mlp_extractor.policy_net.2", "action_net"]
    result = []
    for name in names:
        weight, bias = weights[name + ".weight"], weights[name + ".bias"]
        rows, cols = weight["shape"]
        if bias["shape"] != [rows]:
            raise ValueError("Weight/bias shape mismatch")
        result.append(dict(name=name, rows=rows, cols=cols, weight=weight["values"], bias=bias["values"]))
    if [(x["cols"], x["rows"]) for x in result] != [(3008, 64), (64, 64), (64, 2)]:
        raise ValueError("Unexpected actor architecture")
    return result


def forward_float(layers, obs):
    # Float64 accumulation over original float32 weights. PyTorch parity is separate.
    value = obs
    for index, layer in enumerate(layers):
        cols, weight = layer["cols"], layer["weight"]
        value = [
            sum(weight[row * cols + col] * value[col] for col in range(cols)) + layer["bias"][row]
            for row in range(layer["rows"])
        ]
        if index < 2:
            value = [math.tanh(v) for v in value]
    return value


def round_away(value):
    return math.floor(value + 0.5) if value >= 0 else -math.floor(-value + 0.5)


def trunc_div(value, denominator):
    return value // denominator if value >= 0 else -((-value) // denominator)


def quantize(layers, bits):
    result = []
    limit = 2 ** (bits - 1) - 1
    for layer in layers:
        max_weight = max(abs(v) for v in layer["weight"])
        shift = math.floor(math.log2(limit / max_weight))
        weights = [round_away(v * (2 ** shift)) for v in layer["weight"]]
        assert max(map(abs, weights)) <= limit
        result.append(dict(
            name=layer["name"], rows=layer["rows"], cols=layer["cols"], shift=shift,
            weight=weights, bias=[round_away(v * Q) for v in layer["bias"]]
        ))
    return result


def tanh_fixed(value, table):
    sign = -1 if value < 0 else 1
    v = abs(value)
    if v >= LUT_BOUND * Q:
        return sign * Q
    index, rem = divmod(v, LUT_STEP)
    low = table[index]
    return sign * (low + trunc_div((table[index + 1] - low) * rem, LUT_STEP))


def forward_int(layers, obs, table):
    if len(obs) != 3008 or not all(math.isfinite(v) and abs(v) <= 8 for v in obs):
        raise ValueError("Expected 3008 finite normalized inputs within [-8,8]")
    value = [round_away(v * Q) for v in obs]
    for index, layer in enumerate(layers):
        cols, weight = layer["cols"], layer["weight"]
        sums = [
            sum(weight[row * cols + col] * value[col] for col in range(cols))
            for row in range(layer["rows"])
        ]
        value = [trunc_div(v, 2 ** layer["shift"]) + bias for v, bias in zip(sums, layer["bias"])]
        if index < 2:
            value = [tanh_fixed(v, table) for v in value]
    return [v / Q for v in value]


def decode(action):
    return [int(min(2, max(0, action[0]))), min(1, max(0, action[1]))]


def main():
    verification = verify()
    OUT.mkdir(parents=True, exist_ok=True)
    metadata, weights = read_weights()
    layers = layers_from(weights)
    scaler = ScalerReader(io.BytesIO((MODEL / "scaler.pkl").read_bytes())).load()
    features = scaler.feature_names_in_.values
    assert len(features) == scaler.n_features_in_ == 50
    scale_info = dict(
        features=features, mean=scaler.mean_.values, scale=scaler.scale_.values,
        variance=scaler.var_.values, samples_seen=scaler.n_samples_seen_,
        source_sha256=verification["files"]["scaler.pkl"]["sha256"]
    )
    assert all(math.isfinite(v) and v > 0 for v in scale_info["scale"])
    (OUT / "scaler.json").write_text(json.dumps(scale_info, indent=2) + "\n")
    table = [round_away(math.tanh(i / 256) * Q) for i in range(8 * 256 + 1)]
    (OUT / "tanh-q16.json").write_text(json.dumps(table) + "\n")
    generator = random.Random(SEED)
    inputs = [
        [0.0] * 3008,
        [0.0] * 3000 + [1, 0, 1, 0, 0, 0, 0, 0],
        [1.0] * 3008,
        [-1.0] * 3008,
    ]
    for index in range(SAMPLES - len(inputs)):
        amplitude = [0.1, 1.0, 3.0, 8.0][index % 4]
        inputs.append([generator.uniform(-amplitude, amplitude) for _ in range(3008)])
    reference = [forward_float(layers, obs) for obs in inputs]
    quantization = {}
    for bits in [8, 16]:
        qlayers = quantize(layers, bits)
        result = [forward_int(qlayers, obs, table) for obs in inputs]
        errors = [abs(a - b) for ref, row in zip(reference, result) for a, b in zip(ref, row)]
        flips = [i for i, (ref, row) in enumerate(zip(reference, result)) if decode(ref)[0] != decode(row)[0]]
        quantization[str(bits)] = dict(
            per_layer_weight_shift=[v["shift"] for v in qlayers],
            max_raw_output_error=max(errors), mean_raw_output_error=sum(errors) / len(errors),
            action_disagreements=len(flips), disagreement_samples=flips,
            max_position_size_error=max(abs(decode(a)[1] - decode(b)[1]) for a, b in zip(reference, result)),
            experimental=True,
        )
        if bits == 16:
            blob = bytearray()
            spec = []
            for layer in qlayers:
                offset = len(blob)
                blob.extend(struct.pack("<" + "h" * len(layer["weight"]), *layer["weight"]))
                bias_offset = len(blob)
                blob.extend(struct.pack("<" + "i" * len(layer["bias"]), *layer["bias"]))
                spec.append({k: layer[k] for k in ["name", "rows", "cols", "shift"]} | dict(weight_offset=offset, bias_offset=bias_offset))
            (OUT / "actor-int16.bin").write_bytes(blob)
            (OUT / "actor-int16-layout.json").write_text(json.dumps(dict(
                source_revision=verification["revision"], q=Q, byteorder="little",
                blob_sha256=hashlib.sha256(blob).hexdigest(),
                layers=spec, tanh="tanh-q16.json", input_bound=8,
                status="experimental_not_deployed"
            ), indent=2) + "\n")
    # Save exact integer fixtures for the independent Solidity implementation.
    (OUT / "tanh-q16.bin").write_bytes(struct.pack("<" + "i" * len(table), *table))
    fixture_dir = OUT / "evm-vectors"
    fixture_dir.mkdir(exist_ok=True)
    chosen = list(range(4))
    for action in [1, 2]:
        chosen.append(next(i for i, row in enumerate(reference) if decode(row)[0] == action))
    fixtures = []
    for index, sample in enumerate(chosen):
        observation = inputs[sample]
        actual = forward_int(quantize(layers, 16), observation, table)
        raw = [round_away(v * Q) for v in actual]
        fixture = dict(sample=sample, raw0=raw[0], raw1=raw[1],
                       action=decode(actual)[0], size_q16=round_away(decode(actual)[1] * Q))
        (fixture_dir / f"{index}.bin").write_bytes(struct.pack("<" + "i" * 3008, *[round_away(v * Q) for v in observation]))
        (fixture_dir / f"{index}.json").write_text(json.dumps(fixture) + "\n")
        fixtures.append(fixture)
    (OUT / "evm-vectors.json").write_text(json.dumps(fixtures, indent=2) + "\n")
    float_bytes = b"".join(
        struct.pack("<" + "f" * len(v), *v)
        for layer in layers for v in [layer["weight"], layer["bias"]]
    )
    (OUT / "actor-float32.bin").write_bytes(float_bytes)
    summaries = [
        dict(name=name, shape=v["shape"], parameters=len(v["values"]))
        for name, v in weights.items()
    ]
    report = dict(
        checkpoint_sha256=verification["files"]["final_model.zip"]["sha256"],
        checkpoint_training_steps=metadata["num_timesteps"],
        tensors=summaries, actor_architecture=[3008, 64, 64, 2],
        hidden_activation="Tanh (SB3 2.1.0 default ActorCriticPolicy; checkpoint policy_kwargs is empty)",
        policy_kwargs=metadata["policy_kwargs"],
        actor_parameters=sum(len(x["weight"]) + len(x["bias"]) for x in layers),
        actor_multiply_accumulates=sum(len(x["weight"]) for x in layers),
        total_checkpoint_parameters=sum(len(v["values"]) for v in weights.values()),
        actor_float32_bytes=len(float_bytes),
        actor_int16_weight_int32_bias_bytes=(OUT / "actor-int16.bin").stat().st_size,
        deterministic_inference_excludes=["critic", "optimizer", "log_std"],
        source_action_rule="int(clip(raw[0],0,2)); clip(raw[1],0,1)",
        normalized_input_test=dict(seed=SEED, count=len(inputs), kind="synthetic normalized stress inputs, not MON observations"),
        floating_reference="float64 accumulation over float32 checkpoint weights, not yet PyTorch parity",
        reference_action_counts=dict(collections.Counter(str(decode(row)[0]) for row in reference)),
        quantization=quantization,
        first_samples=[
            dict(index=i, raw_output=out, decoded_action=decode(out)) for i, out in enumerate(reference[:4])
        ],
        limitations=[
            "No live MON/USDC inference or Solidity deployment.",
            "Synthetic action agreement does not certify all valid observations.",
            "Hourly MON data does not equal the daily stock training distribution.",
            "Input bound [-8,8] is an experimental contract precondition, not a property of the original Box.",
        ],
    )
    (OUT / "analysis.json").write_text(json.dumps(report, indent=2) + "\n")
    (OUT / "normalized-test-vectors.json").write_text(json.dumps(dict(
        kind="synthetic normalized stress inputs", seed=SEED,
        observations=inputs[:4], reference_outputs=reference[:4]
    )) + "\n")
    print(json.dumps({k:report[k] for k in [
        "actor_parameters", "actor_multiply_accumulates", "actor_float32_bytes",
        "actor_int16_weight_int32_bias_bytes", "reference_action_counts", "quantization"
    ]}, indent=2))


if __name__ == "__main__":
    main()
