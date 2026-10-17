#!/usr/bin/env python3
"""Bounded, read-only experiments; never writes the application databases.

169 boundaries yield 168 hourly returns. Fixed free providers make comparisons
reproducible; no paid fallback, unbounded retries or silently filled failures.
Print JSON evidence to stdout, with endpoint URLs redacted from errors.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import json
import math
import re
import statistics
import threading
import time
import urllib.error
import urllib.parse
import urllib.request


SWAP = "0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67"
SYNC = "0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"
SAMPLES = {
    "base": ("public-catalog-1", "0xd0b53d9277642d899df5c87a3966a349a798f224", "ETH/USDC"),
    "arbitrum": ("public-catalog-1", "0x80a9ae39310abf666a87c743d6ebbd0e8c42158e", "WETH/GMX"),
    "bsc": ("public-catalog-2", "0x58f876857a02d6762e0101bb5c46a8c1ed44dc16", "WBNB/BUSD"),
}


def safe(value):
    return re.sub(r"https?://[^\s\"<>]+", "[redacted endpoint]", str(value))[:700]


class RPC:
    def __init__(self, chain, provider):
        self.chain, self.provider = chain, provider
        self.header_provider = "public-catalog-2" if chain in ("base", "arbitrum") else provider
        self.next_request = {}
        self.lock = threading.Lock()
        self.http = self.methods = self.response_bytes = 0
        self.method_names = {}
        self.by_provider = {}
        self.headers = {}
        self.blocks = {}

    def snapshot(self):
        return {"http_requests": self.http, "rpc_methods": self.methods, "response_bytes": self.response_bytes,
                "methods": dict(self.method_names), "rpc_methods_by_provider": dict(self.by_provider)}

    def batch(self, requests):
        if not requests:
            return []
        provider = self.header_provider if all(m == "eth_getBlockByNumber" for m, _ in requests) else self.provider
        with self.lock:
            self.http += 1
            self.methods += len(requests)
            for method, _ in requests:
                self.method_names[method] = self.method_names.get(method, 0) + 1
            self.by_provider[provider] = self.by_provider.get(provider, 0) + len(requests)
            send_at = max(time.monotonic(), self.next_request.get(provider, 0))
            interval = .35 if provider == self.header_provider or provider == "drpc-public" else (.8 if self.chain == "base" else .65)
            self.next_request[provider] = send_at + interval
            if self.http > 400 or self.methods > 2400:
                raise RuntimeError("experiment request budget exhausted")
        time.sleep(max(0, send_at - time.monotonic()))
        payload = [{"jsonrpc": "2.0", "id": i, "method": m, "params": p} for i, (m, p) in enumerate(requests)]
        # Single calls stay single calls: some providers reject one-element batches.
        body = json.dumps(payload[0] if len(payload) == 1 else payload).encode()
        url = ("https://base.drpc.org" if provider == "drpc-public" else
               "http://127.0.0.1:8545/history/" + self.chain + "?" + urllib.parse.urlencode({"title": provider}))
        # Match the working HTTP client profile used in the public-provider probe.
        request = urllib.request.Request(url, data=body, headers={"Content-Type": "application/json", "User-Agent": "reqwest"})
        try:
            with urllib.request.urlopen(request, timeout=15) as response:
                raw = response.read(8 * 1024 * 1024 + 1)
        except urllib.error.HTTPError as exc:
            raise RuntimeError("HTTP " + str(exc.code) + " " + safe(exc.read(1200).decode(errors="replace"))) from None
        with self.lock:
            self.response_bytes += len(raw)
        if len(raw) > 8 * 1024 * 1024:
            raise RuntimeError("response exceeds 8MiB experiment limit")
        decoded = json.loads(raw)
        items = decoded if isinstance(decoded, list) else [decoded]
        if any(not isinstance(item, dict) or "id" not in item for item in items):
            raise RuntimeError("malformed RPC response: " + safe(decoded))
        by_id = {item["id"]: item for item in items}
        if len(items) != len(requests) or set(by_id) != set(range(len(requests))):
            raise RuntimeError("duplicate/missing RPC response IDs: " + safe([v.get("error") for v in items]))
        result = []
        for i in range(len(requests)):
            item = by_id[i]
            if "error" in item:
                raise RuntimeError(safe(item["error"]))
            if item.get("result") is None:
                raise RuntimeError("null RPC result, not valid historical data")
            result.append(item["result"])
        return result

    def one(self, method, params):
        return self.batch([(method, params)])[0]

    def many(self, requests):
        price_calls = all(m == "eth_call" for m, _ in requests)
        size = 3 if self.provider == "drpc-public" and price_calls else 10
        if self.chain == "base" and self.provider == "public-catalog-1" and price_calls:
            size = 1
        chunks = [requests[i:i + size] for i in range(0, len(requests), size)]
        with ThreadPoolExecutor(max_workers=4) as executor:
            return [item for values in executor.map(self.batch, chunks) for item in values]

    def get_headers(self, numbers):
        missing = sorted(set(numbers) - self.headers.keys())
        results = self.many([("eth_getBlockByNumber", [hex(n), False]) for n in missing])
        for n, block in zip(missing, results):
            if int(block["number"], 16) != n:
                raise RuntimeError("block number mismatch")
            self.blocks[n] = block
            self.headers[n] = int(block["timestamp"], 16)

    def locate(self, targets, head):
        """Verify last block <= each target AND the next block > it."""
        n, ts = int(head["number"], 16), int(head["timestamp"], 16)
        self.blocks[n], self.headers[n] = head, ts
        self.get_headers([n - 1000])
        speed = max((ts - self.headers[n - 1000]) / 1000, .01)
        lowest = max(0, n - math.ceil((ts - min(targets)) / speed * 1.15) - 1000)
        self.get_headers([lowest])
        for _ in range(4):
            if self.headers[lowest] <= min(targets):
                break
            lowest = max(0, n - 2 * (n - lowest))
            self.get_headers([lowest])
        if self.headers[lowest] > min(targets) or max(targets) >= ts:
            raise RuntimeError("cannot bracket requested timestamps")
        bounds = {t: [lowest, n] for t in targets}
        resolved = {}
        for wave in range(32):
            guesses = {}
            for t, (lo, hi) in bounds.items():
                if t in resolved:
                    continue
                duration = self.headers[hi] - self.headers[lo]
                guess = (lo + hi) // 2 if wave >= 8 or duration <= 0 else lo + int((t - self.headers[lo]) * (hi - lo) / duration)
                guesses[t] = max(lo, min(hi - 1, guess))
            if not guesses:
                return resolved
            self.get_headers([v for g in guesses.values() for v in (g, g + 1)])
            for t, g in guesses.items():
                if self.headers[g] <= t < self.headers[g + 1]:
                    resolved[t] = g
                elif self.headers[g] > t:
                    bounds[t][1] = g
                else:
                    bounds[t][0] = g + 1
        raise RuntimeError("timestamp locator exceeded bounded iterations")


def word(data, index):
    if not isinstance(data, str) or not data.startswith("0x") or len(data) < 2 + 64 * (index + 1):
        raise RuntimeError("short/malformed contract data")
    return int(data[2 + index * 64:2 + (index + 1) * 64], 16)


def price(state, chain, scale):
    if chain == "bsc":
        a, b = word(state, 0), word(state, 1)
        if a <= 0 or b <= 0:
            raise RuntimeError("zero reserves")
        return b / a * scale
    sqrt = word(state, 0)
    if sqrt <= 0:
        raise RuntimeError("zero sqrt price")
    return (sqrt / 2 ** 96) ** 2 * scale


def measure(rpc, fn):
    before = rpc.snapshot()
    start = time.monotonic()
    value = fn()
    after = rpc.snapshot()
    return value, {"wall_seconds": round(time.monotonic() - start, 4),
                   **{k: after[k] - before[k] for k in ("http_requests", "rpc_methods", "response_bytes")}}


def metadata(rpc, address):
    tokens = rpc.batch([("eth_call", [{"to": address, "data": data}, "latest"]) for data in ("0x0dfe1681", "0xd21220a7")])
    tokens = ["0x" + str(v)[-40:] for v in tokens]
    values = rpc.batch([("eth_call", [{"to": token, "data": "0x313ce567"}, "latest"]) for token in tokens])
    decimals = [word(v, 0) for v in values]
    if any(v > 36 for v in decimals):
        raise RuntimeError("unexpected token decimals")
    return tokens, decimals, 10 ** (decimals[0] - decimals[1])


def get_logs(rpc, address, chain, start, end):
    logs = rpc.one("eth_getLogs", [{"address": address, "topics": [SYNC if chain == "bsc" else SWAP],
                                   "fromBlock": hex(start), "toBlock": hex(end)}])
    if not isinstance(logs, list) or len(logs) > 12000:
        raise RuntimeError("invalid/oversized log set")
    seen = set()
    for log in logs:
        block = int(log["blockNumber"], 16)
        key = (log["blockHash"], log["transactionHash"], log["logIndex"])
        if (log.get("removed") or not start <= block <= end or log["address"].lower() != address.lower()
                or log.get("topics", [None])[0] != (SYNC if chain == "bsc" else SWAP) or key in seen):
            raise RuntimeError("noncanonical/out-of-range/duplicate log")
        seen.add(key)
    logs.sort(key=lambda v: (int(v["blockNumber"], 16), int(v["transactionIndex"], 16), int(v["logIndex"], 16)))
    return logs


def canonical_check(rpc, numbers):
    for n in set(numbers):
        block = rpc.one("eth_getBlockByNumber", [hex(n), False])
        if block["hash"] != rpc.blocks[n]["hash"]:
            raise RuntimeError("chain reorganized during probe")


def run(chain, provider_override=None):
    provider, address, pair = SAMPLES[chain]
    provider = provider_override or provider
    if provider == "drpc-public" and chain != "base":
        raise ValueError("drpc-public experiment is restricted to the verified Base endpoint")
    rpc = RPC(chain, provider)
    out = {"chain": chain, "pool": address, "pair": pair, "provider_title": provider,
           "header_provider_title": rpc.header_provider,
           "provider_tier": "free", "price_definition": "token1 per token0; pool state spot",
           "chain_timestamp_basis": "latest confirmed RPC execution blocks, not finalized"}
    started = time.monotonic()
    try:
        head = rpc.one("eth_getBlockByNumber", ["latest", False])
        n, ts = int(head["number"], 16), int(head["timestamp"], 16)
        out["head"] = {"number": n, "hash": head["hash"], "timestamp": ts}
        tokens, decimals, scale = metadata(rpc, address)
        out.update({"token0": tokens[0], "token1": tokens[1], "decimals": decimals})
        anchor = ts  # No wall-clock-hour alignment requirement.
        targets = [anchor - 3600 * i for i in range(168, -1, -1)]
        out["window_utc"] = [datetime.fromtimestamp(t, timezone.utc).isoformat() for t in (targets[0], targets[-1])]
        selector = "0x0902f1ac" if chain == "bsc" else "0x3850c7bd"
        if chain == "base":
            def approximate_mapping():
                rpc.blocks[n], rpc.headers[n] = head, ts
                rpc.get_headers([n - 1000])
                speed = (ts - rpc.headers[n - 1000]) / 1000
                if speed <= 0:
                    raise RuntimeError("invalid block speed estimate")
                mapping = {t: n - round((ts - t) / speed) for t in targets}
                rpc.get_headers(mapping.values())
                if max(abs(rpc.headers[b] - t) for t, b in mapping.items()) > 120:
                    raise RuntimeError("approximate mapping exceeds two-minute tolerance; needs refinement")
                actual = [rpc.headers[mapping[t]] for t in targets]
                if any(a >= b for a, b in zip(actual, actual[1:])):
                    raise RuntimeError("actual sample timestamps are not strictly increasing")
                return mapping
            blocks, metric = measure(rpc, approximate_mapping)
            out["shared_chain_hour_mapping"] = metric
            requests = [("eth_call", [{"to": address, "data": selector}, hex(blocks[t])]) for t in targets]
            states, metric = measure(rpc, lambda: rpc.many(requests))
            out["minimal_history_view"] = metric
            values = [price(v, chain, scale) for v in states]
            # Same fixed block set, same provider, full-refresher-style V3 payload.
            control_indexes = [i * 24 for i in range(8)]
            old_requests = [("eth_call", [{"to": address, "data": data}, hex(blocks[t])])
                            for t in [targets[i] for i in control_indexes] for data in (selector, "0x1a686502", "0xddca3f43")]
            full, metric = measure(rpc, lambda: rpc.many(old_requests))
            out["three_field_control"] = metric
            if any(word(states[source], 0) != word(full[i * 3], 0) for i, source in enumerate(control_indexes)):
                raise RuntimeError("minimal and three-field prices disagree")
            out["control_price_matches"] = len(control_indexes)
            out["control_note"] = "eight sampled blocks only; not a full-window timing comparison"
            canonical_check(rpc, [blocks[targets[0]], blocks[targets[-1]]])
            out["maximum_sample_time_error_seconds"] = max(abs(t - rpc.headers[blocks[t]]) for t in targets)
            sample_times = [rpc.headers[blocks[t]] for t in targets]
            out["series"] = [{"target_sample_timestamp": t, "sample_timestamp": rpc.headers[blocks[t]], "block": blocks[t], "actual_block_timestamp": rpc.headers[blocks[t]],
                              "block_hash": rpc.blocks[blocks[t]]["hash"], "price": p} for t, p in zip(targets, values)]
        elif chain == "arbitrum":
            current_state = rpc.one("eth_call", [{"to": address, "data": selector}, hex(n)])
            boundaries, metric = measure(rpc, lambda: rpc.locate([targets[0]], head))
            out["shared_window_mapping"] = metric
            first = boundaries[targets[0]]
            def approximate_event_grid():
                blocks = {t: first + round((t - targets[0]) * (n - first) / (ts - targets[0])) for t in targets}
                rpc.get_headers(blocks.values())
                actual = [rpc.headers[blocks[t]] for t in targets]
                if any(not 1800 <= b - a <= 5400 for a, b in zip(actual, actual[1:])):
                    raise RuntimeError("sample intervals deviate too far from one hour")
                return blocks
            sample_blocks, metric = measure(rpc, approximate_event_grid)
            out["shared_chain_sample_mapping"] = metric
            logs, metric = measure(rpc, lambda: get_logs(rpc, address, chain, first + 1, n))
            out["window_logs"] = {**metric, "swap_count": len(logs)}
            def partition_check():
                middle = (first + n) // 2
                partitioned = get_logs(rpc, address, chain, first + 1, middle) + get_logs(rpc, address, chain, middle + 1, n)
                identity = lambda v: (v["blockHash"], v["transactionHash"], v["logIndex"])
                if [identity(v) for v in partitioned] != [identity(v) for v in logs]:
                    raise RuntimeError("whole-window and split-window logs disagree")
            _, metric = measure(rpc, partition_check)
            out["split_window_coverage_check"] = {**metric, "matches": True}
            seed_start = max(0, first - (n - first))
            seed, metric = measure(rpc, lambda: get_logs(rpc, address, chain, seed_start, first))
            out["seed_search_logs"] = {**metric, "swap_count": len(seed), "lookback_blocks": first - seed_start + 1}
            if not seed:
                raise RuntimeError("no proven initial price in bounded lookback; do not fill missing history")
            all_logs = [seed[-1]] + logs
            def usable_native_ts(log):
                return targets[0] - 604800 <= int(log.get("blockTimestamp", "0x0"), 16) <= ts
            sample_logs = all_logs[::max(1, len(all_logs) // 8)] + all_logs[-1:]
            _, metric = measure(rpc, lambda: rpc.get_headers([int(v["blockNumber"], 16) for v in sample_logs]))
            out["event_timestamp_audit"] = metric
            for log in sample_logs:
                b = int(log["blockNumber"], 16)
                if rpc.blocks[b]["hash"] != log["blockHash"] or (usable_native_ts(log) and int(log["blockTimestamp"], 16) != rpc.headers[b]):
                    raise RuntimeError("event timestamp/hash disagrees with execution block")
            timed = []
            for log in all_logs:
                b = int(log["blockNumber"], 16)
                timestamp = int(log["blockTimestamp"], 16) if usable_native_ts(log) else rpc.headers.get(b)
                if b in rpc.blocks and rpc.blocks[b]["hash"] != log["blockHash"]:
                    raise RuntimeError("event hash disagrees with fetched block")
                sqrt = word(log["data"], 2)
                timed.append((b, sqrt, log, timestamp))
            if timed[0][0] > first:
                raise RuntimeError("invalid seed block")
            if word(current_state, 0) != timed[-1][1]:
                raise RuntimeError("event replay does not match exact same-block slot0")
            out["latest_slot0_matches_event_replay"] = True
            out["usable_native_timestamp_log_count"] = sum(usable_native_ts(v) for v in all_logs)
            out["zero_or_invalid_timestamp_log_count"] = sum(not usable_native_ts(v) for v in all_logs)
            out["timestamp_hash_samples_verified"] = len({v["blockNumber"] for v in sample_logs})
            index, series = 0, []
            for t in targets:
                sample_block = sample_blocks[t]
                while index + 1 < len(timed) and timed[index + 1][0] <= sample_block:
                    index += 1
                _, sqrt, log, timestamp = timed[index]
                series.append({"sample_timestamp": rpc.headers[sample_block], "sample_block": sample_block,
                               "sample_block_hash": rpc.blocks[sample_block]["hash"], "source_event_timestamp": timestamp, "block": int(log["blockNumber"], 16),
                               "block_hash": log["blockHash"], "transaction_hash": log["transactionHash"],
                               "log_index": int(log["logIndex"], 16), "price": (sqrt / 2 ** 96) ** 2 * scale})
            out["series"] = series
            values = [row["price"] for row in series]
            sample_times = [row["sample_timestamp"] for row in series]
            out["maximum_sample_time_error_seconds"] = max(abs(a - b) for a, b in zip(sample_times, targets))
            out["source_event_timestamp_note"] = "null means not fetched; statistics use actual sample-block timestamps, not unknown event times"
            canonical_check(rpc, [first, n])
            try:
                old_state = rpc.one("eth_call", [{"to": address, "data": selector}, hex(first)])
                out["oldest_history_view"] = "available"
                out["oldest_slot0_matches_event_seed"] = word(old_state, 0) == timed[0][1]
            except Exception as exc:
                out["oldest_history_view"] = safe(exc)
            if out.get("oldest_slot0_matches_event_seed") is False:
                raise RuntimeError("oldest historical slot0 disagrees with event seed")
        else:
            # Bound BSC to capability evidence plus a recent event/view consistency check.
            rpc.blocks[n], rpc.headers[n] = head, ts
            state = rpc.one("eth_call", [{"to": address, "data": selector}, hex(n)])
            logs, metric = measure(rpc, lambda: get_logs(rpc, address, chain, n - 999, n))
            out["recent_sync_logs"] = {**metric, "sync_count": len(logs)}
            out["recent_reserves_match"] = bool(logs) and all(word(state, i) == word(logs[-1]["data"], i) for i in (0, 1))
            boundaries = rpc.locate([targets[0]], head)
            first = boundaries[targets[0]]
            for label, request in (("seven_day_view", ("eth_call", [{"to": address, "data": selector}, hex(first)])),
                                   ("seven_day_logs", ("eth_getLogs", [{"address": address, "topics": [SYNC], "fromBlock": hex(first), "toBlock": hex(n)}]))):
                try:
                    result, metric = measure(rpc, lambda: rpc.one(*request))
                    out[label] = {"available": True, **metric, "log_count": len(result) if isinstance(result, list) else None}
                except Exception as exc:
                    out[label] = {"available": False, "error": safe(exc)}
            out["status"] = "capability_probe_only_no_hourly_series"
            return out
        if len(values) != 169 or any(not math.isfinite(v) or v <= 0 for v in values):
            raise RuntimeError("incomplete/invalid seven-day price series")
        returns = [math.log(b / a) for a, b in zip(values, values[1:])]
        durations = [b - a for a, b in zip(sample_times, sample_times[1:])]
        drift_per_second = sum(returns) / sum(durations)
        variance_per_second = sum((r - drift_per_second * dt) ** 2 / dt for r, dt in zip(returns, durations)) / (len(returns) - 1)
        out.update({"status": "complete_7d_probe", "boundary_prices": 169, "hourly_close_points": 168,
                    "hourly_returns": len(returns), "mean_absolute_hourly_log_return": statistics.mean(abs(v) for v in returns),
                    "hourly_log_return_sample_stddev": statistics.stdev(returns),
                    "actual_span_seconds": sum(durations), "min_sample_interval_seconds": min(durations), "max_sample_interval_seconds": max(durations),
                    "time_adjusted_hourly_volatility_estimate": math.sqrt(variance_per_second * 3600),
                    "seven_day_aggregate_volatility_estimate": math.sqrt(variance_per_second * sum(durations)),
                    "volatility_note": "diffusion estimate using actual sampling intervals; event state persists between proven changes; not annualized or oracle/TWAP"})
    except Exception as exc:
        out.update({"status": "failed_not_complete", "error": safe(exc)})
    finally:
        out["total"] = {**rpc.snapshot(), "wall_seconds": round(time.monotonic() - started, 4)}
    return out


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("chain", choices=SAMPLES)
    parser.add_argument("--provider", choices=("public-catalog-1", "public-catalog-2", "drpc-public"), default=None)
    args = parser.parse_args()
    print(json.dumps(run(args.chain, args.provider), ensure_ascii=False, allow_nan=False), flush=True)
