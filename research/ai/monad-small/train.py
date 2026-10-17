"""Train tiny quantized classifiers on archived, real Monad DEX candles."""
import copy
import datetime as dt
import hashlib
import json
import os
import math
from pathlib import Path
import statistics
import torch
from torch import nn

ROOT = Path(os.environ.get("MONAD_AI_RUN_DIR", Path(__file__).resolve().parent)).resolve()
CFG = json.loads((ROOT / "config.json").read_text())
DATA_BYTES = (ROOT / "dataset.json").read_bytes()
DATA = json.loads(DATA_BYTES)
torch.set_num_threads(2)
torch.manual_seed(CFG["seed"])
Q, WQ = CFG["input_q"], CFG["weight_q"]
NAMES = ["HOLD", "BUY", "SELL"]
PERIOD = DATA["timeframe_seconds"]
LOOKBACK = CFG.get("lookback_bars", CFG["lookback_hours"])
HORIZON = CFG.get("horizon_bars", CFG["horizon_hours"])


def utc(t):
    return dt.datetime.fromtimestamp(t, dt.timezone.utc).isoformat()


def make_samples(asset):
    bars, samples = asset["candles"], []
    for i in range(LOOKBACK, len(bars) - HORIZON - 1):
        end = i + HORIZON + 1
        window = bars[i - 24:end + 1]
        if any(b[0] - a[0] != PERIOD for a, b in zip(window, window[1:])):
            continue
        closes = [b[4] for b in bars[i - 24:i + 1]]
        returns = [math.log(b / a) for a, b in zip(closes, closes[1:])]
        current = bars[i]
        volume_mean = statistics.mean(b[5] for b in bars[i - 23:i + 1])
        features = [math.log(closes[-1] / closes[-1 - lag]) for lag in (1, 3, 6, 12, 24)] + [
            (current[2] - current[3]) / current[4], current[4] / current[1] - 1,
            current[4] / statistics.mean(closes[-6:]) - 1, current[4] / statistics.mean(closes[-24:]) - 1,
            statistics.pstdev(returns[-6:]), statistics.pstdev(returns), math.log1p(current[5]) - math.log1p(volume_mean)]
        future_return = bars[end][1] / bars[i + 1][1] - 1
        threshold = CFG["label_threshold_bps"] / 10000
        label = 1 if future_return > threshold else 2 if future_return < -threshold else 0
        samples.append({"asset": asset["symbol"], "timestamp": current[0] + PERIOD, "label_end": bars[end][0],
            "entry_timestamp": bars[i + 1][0], "exit_timestamp": bars[end][0], "entry_price": bars[i + 1][1],
            "exit_price": bars[end][1], "future_return": future_return, "features": features, "label": label})
    return samples


samples = sorted([s for a in DATA["assets"] for s in make_samples(a)], key=lambda s: (s["timestamp"], s["asset"]))
times = sorted(set(s["timestamp"] for s in samples))
cut1, cut2 = times[int(len(times) * 0.6)], times[int(len(times) * 0.8)]
parts = {
    "train": [s for s in samples if s["label_end"] < cut1],
    "validation": [s for s in samples if cut1 <= s["timestamp"] and s["label_end"] < cut2],
    "test": [s for s in samples if s["timestamp"] >= cut2],
}
if min(len(p) for p in parts.values()) < 100:
    raise ValueError("Insufficient continuous real samples for chronological splits")
means = [statistics.mean(s["features"][j] for s in parts["train"]) for j in range(12)]
scales = [max(statistics.pstdev(s["features"][j] for s in parts["train"]), 1e-9) for j in range(12)]


def quantize_input(features):
    return [round(max(-CFG["input_clip"], min(CFG["input_clip"], (v - m) / scale)) * Q) for v, m, scale in zip(features, means, scales)]


for s in samples:
    s["input_q12"] = quantize_input(s["features"])
tensors = {name: (torch.tensor([s["input_q12"] for s in rows], dtype=torch.float64), torch.tensor([s["label"] for s in rows])) for name, rows in parts.items()}
counts = [sum(s["label"] == k for s in parts["train"]) for k in range(3)]
if not all(counts):
    raise ValueError("Training data must contain all three labels")
class_weights = torch.tensor([len(parts["train"]) / (3 * n) for n in counts], dtype=torch.float64)


def ste_round(value, mode="round"):
    result = torch.round(value) if mode == "round" else torch.trunc(value)
    return value + (result - value).detach()


class Tiny(nn.Module):
    def __init__(self, hidden):
        super().__init__()
        self.layers = nn.ModuleList([nn.Linear(12, hidden), nn.Linear(hidden, 3)] if hidden else [nn.Linear(12, 3)])
        self.double()

    def forward(self, value):
        for i, layer in enumerate(self.layers):
            weights = ste_round(layer.weight.clamp(-1.999, 1.999) * WQ)
            bias = ste_round(layer.bias.clamp(-16, 16) * WQ * Q)
            value = ste_round(nn.functional.linear(value, weights, bias) / WQ, "trunc")
            if i < len(self.layers) - 1:
                value = torch.relu(value)
        return value / Q


def metrics(model, name):
    x, y = tensors[name]
    with torch.no_grad():
        logits = model(x)
        predicted = logits.argmax(1)
        loss = nn.functional.cross_entropy(logits, y, weight=class_weights).item()
    matrix = [[int(((y == i) & (predicted == j)).sum()) for j in range(3)] for i in range(3)]
    recall = [matrix[i][i] / sum(matrix[i]) if sum(matrix[i]) else None for i in range(3)]
    return {"loss": loss, "accuracy": float((predicted == y).double().mean()),
        "balanced_accuracy": statistics.mean(r for r in recall if r is not None), "confusion_matrix": matrix,
        "predicted_counts": [int((predicted == i).sum()) for i in range(3)], "actual_counts": [int((y == i).sum()) for i in range(3)]}


models, candidate_reports = [], []
for hidden in (0, CFG["hidden_width"]):
    torch.manual_seed(CFG["seed"])
    model = Tiny(hidden)
    optimizer = torch.optim.Adam(model.parameters(), lr=CFG["learning_rate"], weight_decay=0.001)
    best_loss, best_state, best_epoch = float("inf"), None, None
    for epoch in range(CFG["epochs"]):
        optimizer.zero_grad()
        loss = nn.functional.cross_entropy(model(tensors["train"][0]), tensors["train"][1], weight=class_weights)
        loss.backward()
        optimizer.step()
        if epoch % 5 == 0:
            validation_loss = metrics(model, "validation")["loss"]
            if validation_loss < best_loss:
                best_loss, best_state, best_epoch = validation_loss, copy.deepcopy(model.state_dict()), epoch + 1
    model.load_state_dict(best_state)
    models.append(model)
    report = {"architecture": [12, hidden, 3] if hidden else [12, 3], "parameters": sum(p.numel() for p in model.parameters()),
        "selected_epoch": best_epoch, "validation": metrics(model, "validation")}
    candidate_reports.append(report)
    print(json.dumps(report), flush=True)
selected = min(range(len(models)), key=lambda i: candidate_reports[i]["validation"]["loss"])
model = models[selected]
layers = [{"rows": l.out_features, "cols": l.in_features,
    "weights": torch.round(l.weight.detach().clamp(-1.999, 1.999) * WQ).to(torch.int64).tolist(),
    "bias": torch.round(l.bias.detach().clamp(-16, 16) * WQ * Q).to(torch.int64).tolist()} for l in model.layers]


def integer_predict(values):
    for i, layer in enumerate(layers):
        output = []
        for weights, bias in zip(layer["weights"], layer["bias"]):
            total = sum(w * x for w, x in zip(weights, values)) + bias
            value = (abs(total) // WQ) * (-1 if total < 0 else 1)
            output.append(max(0, value) if i < len(layers) - 1 else value)
        values = output
    return values, max(range(3), key=lambda i: values[i])


parity_count = 0
for name, rows in parts.items():
    with torch.no_grad():
        expected = torch.round(model(tensors[name][0]) * Q).to(torch.int64).tolist()
    for sample, logits in zip(rows, expected):
        actual, action = integer_predict(sample["input_q12"])
        if actual != logits:
            raise AssertionError("Integer/PyTorch raw logit mismatch")
        sample["scores_q12"], sample["action"] = actual, action
        parity_count += 1


def backtest(asset):
    rows = [s for s in parts["test"] if s["asset"] == asset["symbol"]]
    equity, position, last_exit, peak, drawdown = 1.0, 0, None, 1.0, 0.0
    fee = (asset["fee_bps_per_side"] + CFG["slippage_bps_per_side"]) / 10000
    history, curve = [], []
    last_exit_price = None
    used = []
    for s in rows:
        if last_exit is not None and s["entry_timestamp"] < last_exit:
            continue
        if position and last_exit is not None and s["entry_timestamp"] != last_exit:
            equity *= 1 - fee
            history.append({"timestamp": last_exit, "action": "SELL", "reason": "close_before_data_gap", "fee_fraction": fee, "price_usd": last_exit_price})
            position = 0
        target = 1 if s["action"] == 1 else 0 if s["action"] == 2 else position
        if target != position:
            equity *= 1 - fee
            history.append({"timestamp": s["entry_timestamp"], "action": NAMES[s["action"]], "price_usd": s["entry_price"], "fee_fraction": fee,
                "source": "held_out_real_candle_simulation", "scores_q12": s["scores_q12"]})
        position = target
        equity *= 1 + position * s["future_return"]
        last_exit = s["exit_timestamp"]
        last_exit_price = s["exit_price"]
        peak = max(peak, equity)
        drawdown = max(drawdown, 1 - equity / peak)
        curve.append({"timestamp": last_exit, "equity": equity, "price_usd": s["exit_price"]})
        used.append(s)
    if position:
        equity *= 1 - fee
        history.append({"timestamp": last_exit, "action": "SELL", "reason": "end_of_test", "fee_fraction": fee, "price_usd": last_exit_price})
        curve[-1]["equity"] = equity
        drawdown = max(drawdown, 1 - equity / peak)
    # The comparison uses the same observed prediction windows, liquidating across gaps.
    baseline, baseline_last = 1.0, None
    for s in used:
        if baseline_last != s["entry_timestamp"]:
            baseline *= (1 - fee) ** 2
        baseline *= 1 + s["future_return"]
        baseline_last = s["exit_timestamp"]
    return {"symbol": asset["symbol"], "windows": len(used), "return_after_assumed_costs": equity - 1,
        "observed_windows_buy_hold_return": baseline - 1, "max_drawdown": drawdown, "trades": len(history),
        "fee_bps_per_side": asset["fee_bps_per_side"], "slippage_bps_per_side": CFG["slippage_bps_per_side"],
        "gas_included": False, "history": history, "equity_curve": curve}


model_artifact = {"format": "monad-tiny-q12-v1", "architecture": candidate_reports[selected]["architecture"], "layers": layers,
    "input_q": Q, "weight_q": WQ, "input_clip": CFG["input_clip"], "features": CFG["features"], "mean": means, "scale": scales,
    "actions": NAMES, "rounding": "inputs/parameters: ties-to-even; dense: truncation toward zero; argmax: first tie",
    "timeframe_seconds": PERIOD,
    "dataset_sha256": hashlib.sha256(DATA_BYTES).hexdigest()}
model_bytes = json.dumps(model_artifact, indent=2).encode()
(ROOT / "model.json").write_bytes(model_bytes)
test_metrics = metrics(model, "test")
report = {"created_at": dt.datetime.now(dt.timezone.utc).isoformat(), "dataset_sha256": hashlib.sha256(DATA_BYTES).hexdigest(),
    "model_sha256": hashlib.sha256(model_bytes).hexdigest(), "config": CFG, "source": DATA["source"],
    "selection_rule": "Lowest validation weighted cross-entropy; test is evaluated only after model selection",
    "selected_candidate": selected, "candidates": candidate_reports, "test": test_metrics,
    "hold_only_test_accuracy": sum(s["label"] == 0 for s in parts["test"]) / len(parts["test"]),
    "integer_pytorch_raw_parity": {"matched": parity_count, "total": parity_count},
    "splits": {name: {"count": len(rows), "from": utc(min(s["timestamp"] for s in rows)), "to": utc(max(s["timestamp"] for s in rows)),
        "assets": {a["symbol"]: sum(s["asset"] == a["symbol"] for s in rows) for a in DATA["assets"]}} for name, rows in parts.items()},
    "assets": [{k: v for k, v in a.items() if k != "candles"} | {"candles": len(a["candles"]), "from": utc(a["candles"][0][0]), "to": utc(a["candles"][-1][0])} for a in DATA["assets"]],
    "backtests": [backtest(a) for a in DATA["assets"]],
    "latest_observations": [max((s for s in samples if s["asset"] == a["symbol"]), key=lambda s: s["timestamp"]) for a in DATA["assets"]],
    "limitations": ["GeckoTerminal indexed USD candles, not raw self-verified swap logs", "Historical replay, not real wallet trades",
        "No fabricated candles; all feature/label windows require consecutive bars", "No profitability guarantee; gas is excluded from backtests",
        "Pool selection uses present liquidity and creates selection bias", "Past pooled assets are correlated and the history is short"]}
(ROOT / "report.json").write_text(json.dumps(report, indent=2) + "\n")
vectors = []
for asset in DATA["assets"]:
    rows = [s for s in parts["test"] if s["asset"] == asset["symbol"]]
    vectors += rows[-2:]
for action in range(3):
    row = next((s for s in parts["test"] if s["action"] == action), None)
    if row is not None and row not in vectors:
        vectors.append(row)
(ROOT / "vectors.json").write_text(json.dumps(vectors, indent=2) + "\n")
print(json.dumps({"selected": candidate_reports[selected], "test": test_metrics, "splits": report["splits"], "integer_parity": parity_count,
    "backtests": [{k: v for k, v in b.items() if k not in ("history", "equity_curve")} for b in report["backtests"]]}, indent=2), flush=True)
