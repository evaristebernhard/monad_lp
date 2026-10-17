"""Verify pinned checkpoint bytes and inspect SB3 JSON without executing pickle."""
import hashlib
import json
import subprocess
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MODEL = ROOT / "third_party/stock-trading-rl-agent"
COMMIT = "a317fec1939eda44d04088b47b48ca3ee158bc1f"
EXPECTED = {
    "final_model.zip": "d714969844c671a04cecbe64c428a23919dbcae35bfcc3078e32d0f0b8fdf366",
    "scaler.pkl": "cd514054432a4391237c70c9e98479a137aaf4dffbf28b768f78d648e1e7451c",
}

def verify():
    commit = subprocess.check_output(
        ["git", "-C", str(MODEL), "rev-parse", "HEAD"], text=True
    ).strip()
    if commit != COMMIT:
        raise ValueError(f"Unexpected model revision: {commit}")
    files = {}
    for filename, expected in EXPECTED.items():
        data = (MODEL / filename).read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        if digest != expected:
            raise ValueError(f"Checksum mismatch: {filename}")
        files[filename] = {"sha256": digest, "bytes": len(data)}
    with zipfile.ZipFile(MODEL / "final_model.zip") as checkpoint:
        bad = checkpoint.testzip()
        if bad:
            raise ValueError(f"Corrupt checkpoint member: {bad}")
        metadata = json.loads(checkpoint.read("data"))
        for filename in ("policy.pth", "policy.optimizer.pth"):
            if len(checkpoint.read(filename)) < 1024:
                raise ValueError(f"Missing weight payload: {filename}")
        if metadata["observation_space"]["_shape"] != [3008]:
            raise ValueError("Unexpected observation shape")
        if metadata["action_space"]["_shape"] != [2]:
            raise ValueError("Unexpected action shape")
        if metadata["num_timesteps"] != 501760:
            raise ValueError("Unexpected checkpoint training step")
        version = checkpoint.read("_stable_baselines3_version").decode().strip()
    return {
        "repository": "https://huggingface.co/Adilbai/stock-trading-rl-agent",
        "download_mirror": "https://hf-mirror.com/Adilbai/stock-trading-rl-agent",
        "revision": commit,
        "license_declared_by_model_card": "MIT",
        "files": files,
        "algorithm": "PPO",
        "stable_baselines3_version": version,
        "num_timesteps": metadata["num_timesteps"],
        "observation": {
            "shape": [3008],
            "market": "60 time steps x 50 features",
            "portfolio": "8 normalized portfolio fields",
            "original_training_assets": ["AAPL", "MSFT", "GOOGL", "AMZN", "TSLA"],
        },
        "action": {
            "shape": [2],
            "type": "continuous Box",
            "low": [0, 0],
            "high": [2, 1],
            "environment_decoding": "int(clip(action[0], 0, 2)); clip(action[1], 0, 1)",
            "labels": {"0": "HOLD", "1": "BUY", "2": "SELL"},
        },
        "verification": {
            "sha256": "passed",
            "zip_integrity": "passed",
            "checkpoint_metadata": "passed",
            "inference": "not_run",
            "solidity_deployment": "not_started",
            "neurom_compatibility": "requires_adapter_or_distillation",
        },
    }

if __name__ == "__main__":
    print(json.dumps(verify(), indent=2, ensure_ascii=False))
