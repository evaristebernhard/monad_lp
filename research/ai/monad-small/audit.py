"""Independent data lineage and chronology checks for the real-data experiment."""
import hashlib
import json
import os
from pathlib import Path

root = Path(os.environ.get("MONAD_AI_RUN_DIR", Path(__file__).resolve().parent)).resolve()
read = lambda name: json.loads((root / name).read_text())
dataset, model, report, vectors = [read(name) for name in ("dataset.json", "model.json", "report.json", "vectors.json")]
assert dataset["chain_id"] == 143 and dataset["empty_intervals_filled"] is False
assert hashlib.sha256((root / "dataset.json").read_bytes()).hexdigest() == report["dataset_sha256"] == model["dataset_sha256"]
assert hashlib.sha256((root / "model.json").read_bytes()).hexdigest() == report["model_sha256"]
for source in dataset["sources"]:
    assert source["url"].startswith("https://api.geckoterminal.com/api/v2/networks/monad/")
    assert hashlib.sha256((root / source["file"]).read_bytes()).hexdigest() == source["sha256"]
period = dataset["timeframe_seconds"]
for asset in dataset["assets"]:
    original = []
    for source in dataset["sources"]:
        if f'/pools/{asset["pool"]}/ohlcv/' in source["url"]:
            original += json.loads((root / source["file"]).read_text())["data"]["attributes"]["ohlcv_list"]
    rows = {json.dumps(row) for row in original}
    assert all(json.dumps(row) in rows for row in asset["candles"]), "Fabricated or modified candle"
    times = [row[0] for row in asset["candles"]]
    assert times == sorted(set(times))
    assert not set(times).intersection(asset["conflicting_timestamps_excluded"])
    for vector in (v for v in vectors if v["asset"] == asset["symbol"]):
        index = next(i for i, row in enumerate(asset["candles"]) if row[0] + period == vector["timestamp"])
        window = asset["candles"][index - 24:index + 6]
        assert len(window) == 30
        assert all(b[0] - a[0] == period for a, b in zip(window, window[1:]))
        assert asset["candles"][index + 1][0] == vector["timestamp"]
        assert asset["candles"][index + 5][0] == vector["label_end"]
        assert vector["entry_price"] == asset["candles"][index + 1][1]
        assert vector["exit_price"] == asset["candles"][index + 5][1]
splits = report["splits"]
assert splits["train"]["to"] < splits["validation"]["from"] < splits["validation"]["to"] < splits["test"]["from"]
assert report["integer_pytorch_raw_parity"]["matched"] == report["integer_pytorch_raw_parity"]["total"] == sum(s["count"] for s in splits.values())
print(json.dumps({"raw_sources_verified": len(dataset["sources"]), "real_candles_verified": sum(len(a["candles"]) for a in dataset["assets"]),
    "real_vectors_verified": len(vectors), "integer_parity": report["integer_pytorch_raw_parity"]}, indent=2))
