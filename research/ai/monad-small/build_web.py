"""Publish only public research results to the existing web application's bundle."""
import json
import os
from pathlib import Path

root = Path(os.environ.get("MONAD_AI_RUN_DIR", Path(__file__).resolve().parent)).resolve()
read = lambda name: json.loads((root / name).read_text())
dataset, report, model = read("dataset.json"), read("report.json"), read("model.json")
old = json.loads((root.parent / "model-study/testnet-deployment.json").read_text())
deployment = read("deployment.json") if (root / "deployment.json").exists() else None
artifact = json.loads((root.parent / "evm/out/TinyMonadPolicy.sol/TinyMonadPolicy.json").read_text())
payload = {"report": report, "model": model, "vectors": read("vectors.json"), "deployment": deployment, "abi": artifact["abi"],
    "sources": dataset["sources"], "candles": {a["symbol"]: a["candles"] for a in dataset["assets"]},
    "previous": {k: old[k] for k in ("actor", "explorer", "totalChargedFeeMon", "operations")}}
path = root.parents[2] / "frontend/src/generated/monad-ai.json"
path.write_text(json.dumps(payload, separators=(",", ":")) + "\n")
public = root.parents[2] / "frontend/public/ai"
public.mkdir(parents=True, exist_ok=True)
(public / "monad-small-report.json").write_text(json.dumps(report, indent=2) + "\n")
(public / "monad-small-model.json").write_text(json.dumps(model, indent=2) + "\n")
print(path)
