"""Archive real Monad DEX candles. Never manufacture or forward-fill bars."""
import datetime as dt
import hashlib
import json
import math
from pathlib import Path
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parent
CONFIG = json.loads((ROOT / "config.json").read_text())
BASE = "https://api.geckoterminal.com/api/v2/networks/monad"
RAW = ROOT / "raw"
RAW.mkdir(exist_ok=True)
last_request = 0.0


def fetch(path, name, params=None):
    global last_request
    url = BASE + path + ("?" + urllib.parse.urlencode(params) if params else "")
    for attempt in range(4):
        time.sleep(max(0, 7 - (time.monotonic() - last_request)))
        last_request = time.monotonic()
        try:
            request = urllib.request.Request(url, headers={"Accept": "application/json", "User-Agent": "MonadSmallModel/1.0"})
            with urllib.request.urlopen(request, timeout=35) as response:
                body = response.read()
            data = json.loads(body)
            if "errors" in data:
                raise ValueError(str(data["errors"]))
            (RAW / name).write_bytes(body)
            return data, {"url": url, "file": f"raw/{name}", "sha256": hashlib.sha256(body).hexdigest(), "fetched_at": dt.datetime.now(dt.timezone.utc).isoformat()}
        except urllib.error.HTTPError as error:
            if error.code not in (429, 500, 502, 503, 504) or attempt == 3:
                raise
            time.sleep(15 * (attempt + 1))
    raise RuntimeError("request exhausted")


def main():
    sources, assets = [], []
    for asset in CONFIG["assets"]:
        symbol, token = asset["symbol"], asset["address"].lower()
        listing, source = fetch(f"/tokens/{token}/pools", f"{symbol}-pools.json", {"page": 1})
        sources.append(source)
        candidates = []
        for pool in listing["data"]:
            sides = [pool["relationships"][f"{side}_token"]["data"]["id"].split("_", 1)[1].lower() for side in ("base", "quote")]
            if token not in sides:
                continue
            a = pool["attributes"]
            liquidity, volume = float(a.get("reserve_in_usd") or 0), float(a.get("volume_usd", {}).get("h24") or 0)
            if liquidity >= 10000 and volume > 0:
                candidates.append((liquidity, pool))
        if not candidates:
            raise ValueError(f"{symbol}: no verified active pool with >= $10k liquidity")
        pool = max(candidates, key=lambda item: item[0])[1]
        p = pool["attributes"]
        address = p["address"]
        rows, conflicting_timestamps = {}, set()
        before = int(time.time()) // 3600 * 3600
        for page in range(CONFIG["pages_per_asset"]):
            payload, source = fetch(f"/pools/{address}/ohlcv/hour", f"{symbol}-ohlcv-{page}.json", {
                "aggregate": 1, "limit": CONFIG["history_limit_per_page"], "currency": "usd", "token": token,
                "before_timestamp": before, "include_empty_intervals": "false"})
            sources.append(source)
            meta = payload.get("meta", {})
            if not any(str(meta.get(side, {}).get("address", "")).lower() == token for side in ("base", "quote")):
                raise ValueError(f"{symbol}: candle token metadata mismatch")
            candles = payload["data"]["attributes"]["ohlcv_list"]
            for row in candles:
                if len(row) != 6 or not all(isinstance(v, (int, float)) and math.isfinite(v) for v in row):
                    raise ValueError(f"{symbol}: invalid candle")
                t, o, h, l, c, v = row
                if not (t % 3600 == 0 and min(o, h, l, c) > 0 and h >= max(o, c) and l <= min(o, c) and v >= 0):
                    raise ValueError(f"{symbol}: invalid OHLCV values")
                if t + 3600 <= time.time():
                    if t in conflicting_timestamps:
                        continue
                    if t in rows and rows[t] != row:
                        # Preserve both provider rows in the raw archive, but exclude this timestamp.
                        conflicting_timestamps.add(t)
                        del rows[t]
                        continue
                    rows[t] = row
            if not candles:
                break
            before = min(row[0] for row in candles) - 1
            print(json.dumps({"asset": symbol, "page": page, "candles": len(rows), "pool": address}), flush=True)
            if len(candles) < CONFIG["history_limit_per_page"]:
                break
        ordered = sorted(rows.values())
        if len(ordered) < 200:
            raise ValueError(f"{symbol}: insufficient real candles ({len(ordered)})")
        import re
        fee = re.search(r"(\d+(?:\.\d+)?)%", p["name"])
        fee_bps = float(fee.group(1)) * 100 if fee else CONFIG["fallback_fee_bps_per_side"]
        assets.append({**asset, "pool": address, "pool_name": p["name"], "dex": pool["relationships"]["dex"]["data"]["id"],
            "liquidity_usd_at_fetch": float(p["reserve_in_usd"]), "fee_bps_per_side": fee_bps,
            "fee_source": "pool_name" if fee else "conservative_config_assumption", "candles": ordered,
            "conflicting_timestamps_excluded": sorted(conflicting_timestamps),
            "gap_count": sum(b[0] - a[0] != 3600 for a, b in zip(ordered, ordered[1:]))})
    dataset = {"source": "GeckoTerminal index of Monad mainnet DEX trades", "chain_id": 143, "currency": "USD",
        "timeframe_seconds": 3600, "empty_intervals_filled": False, "assets": assets, "sources": sources,
        "fetched_at": dt.datetime.now(dt.timezone.utc).isoformat()}
    body = json.dumps(dataset, indent=2).encode()
    (ROOT / "dataset.json").write_bytes(body)
    print(json.dumps({"dataset_sha256": hashlib.sha256(body).hexdigest(), "assets": [{"symbol": a["symbol"], "candles": len(a["candles"]), "gaps": a["gap_count"]} for a in assets]}), flush=True)


if __name__ == "__main__":
    main()
