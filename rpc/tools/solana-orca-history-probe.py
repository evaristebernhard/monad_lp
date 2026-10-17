#!/usr/bin/env python3
"""Read-only Solana pool price-history experiments.

The probe uses Solana transaction history, not current account state and not an
oracle.  It parses Orca's Anchor Traded event and uses post_sqrt_price.  It
does not write either SQLite database.
"""
import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import json
import math
import statistics
import time
import urllib.error
import urllib.request


FAMILIES = {
    "orca": {
        "protocol": "Orca Whirlpools",
        "pair": "MPLX/USDC",
        "pool": "78QBVQz3EnpuzmY42LRcCkn7gSyuFEMmQn1m5bY1yHgw",
        "discriminator": bytes.fromhex("e1ca49af932ba096"),
        "event_length": 121,
        "pool_offset": 8,
        "sqrt_offset": 57,
        "token0_decimals": 6,
        "token1_decimals": 6,
        "event_name": "Traded",
    },
    "raydium-clmm": {
        "protocol": "Raydium",
        "pair": "AVAX/USDC",
        "pool": "4w1x54wDEyxC9TxEAQWzdz98EgMk2YFhcHm1i4owaM6f",
        "discriminator": bytes.fromhex("40c6cde8260871e2"),
        "event_length": 221,
        "pool_offset": 8,
        "sqrt_offset": 169,
        "token0_decimals": 6,
        "token1_decimals": 6,
        "event_name": "SwapEvent",
    },
    "meteora-dlmm": {
        "protocol": "Meteora DLMM",
        "pair": "SOL/USDC",
        "pool": "5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6",
        "event_source": "event_cpi",
        "event_cpi_discriminator": bytes.fromhex("e445a52e51cb9a1d"),
        "event_discriminators": {
            bytes.fromhex("516ce3becdd00ac4"),  # Swap
            bytes.fromhex("2e7452d7941b544d"),  # Swap2Evt
        },
        "pool_offset": 16,
        "active_id_offset": 80,
        "token0_decimals": 9,
        "token1_decimals": 6,
        "bin_step": 4,
        "event_name": "Swap/Swap2Evt",
    },
    "raydium-standard": {
        "protocol": "Raydium",
        "pair": "WSOL/AU79",
        "pool": "CqmwcyB7fGor8z7As56yXBQzWjy7MnACtmj6wuLHgV5H",
        "event_source": "post_balances",
        "vault0": "2DAjjoVJwj4mRGFjWfANncza532UX6RJWCwAhKTAPwwG",
        "vault1": "BsmhRYdoWAyhc9iE2gdykrw5pQkNJTpkrGyMdgcWsVsf",
        "token0_decimals": 9,
        "token1_decimals": 9,
        "event_name": "Raydium vault postTokenBalances",
    },
}


def b58decode(value):
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    number = 0
    for char in value:
        number = number * 58 + alphabet.index(char)
    raw = number.to_bytes((number.bit_length() + 7) // 8, "big") if number else b""
    return b"\0" * (len(value) - len(value.lstrip("1"))) + raw


def rpc_call(title, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    request = urllib.request.Request(
        "http://127.0.0.1:8545/rpc/solana?title=" + title,
        data=body,
        headers={"Content-Type": "application/json", "User-Agent": "probe"},
    )
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            data = json.loads(response.read(8 * 1024 * 1024 + 1))
    except urllib.error.HTTPError as exc:
        raise RuntimeError("HTTP " + str(exc.code) + ": " + exc.read(800).decode(errors="replace")) from None
    if data.get("error"):
        raise RuntimeError(str(data["error"]))
    if data.get("result") is None:
        raise RuntimeError("null result for " + method)
    return data["result"]


def signatures(title, pool, end_time, lookback_seconds=172800):
    """Page the address index until a timestamp before the required seed."""
    cutoff = end_time - lookback_seconds
    result, before, pages = [], None, 0
    while pages < 12:
        options = {"limit": 1000, "commitment": "confirmed"}
        if before:
            options["before"] = before
        page = rpc_call(title, "getSignaturesForAddress", [pool, options])
        pages += 1
        if not page:
            break
        result.extend(item for item in page if item.get("blockTime") is not None and item.get("err") is None)
        before = page[-1]["signature"]
        if page[-1].get("blockTime") is not None and page[-1]["blockTime"] <= cutoff:
            break
    if not result:
        raise RuntimeError("no successful address signatures returned")
    result.sort(key=lambda item: (item["blockTime"], item["slot"], item.get("transactionIndex", -1)))
    return result, {"pages": pages, "signatures_returned": len(result), "oldest_block_time": result[0]["blockTime"]}


def transaction_events(title, signature, pool_bytes, family):
    value = rpc_call(title, "getTransaction", [signature, {
        "encoding": "json",
        "maxSupportedTransactionVersion": 1,
        "commitment": "confirmed",
    }])
    meta = value.get("meta") or {}
    if meta.get("err") is not None:
        return []
    events = []
    if family.get("event_source") == "post_balances":
        message = (value.get("transaction") or {}).get("message") or {}
        account_keys = []
        for key in message.get("accountKeys") or []:
            account_keys.append(key.get("pubkey") if isinstance(key, dict) else key)
        loaded = meta.get("loadedAddresses") or {}
        account_keys.extend(loaded.get("writable") or [])
        account_keys.extend(loaded.get("readonly") or [])
        balances = {}
        for balance in meta.get("postTokenBalances") or []:
            index = balance.get("accountIndex", -1)
            if not (0 <= index < len(account_keys)):
                continue
            amount = (balance.get("uiTokenAmount") or {}).get("amount")
            if amount is not None:
                balances[account_keys[index]] = int(amount)
        reserve0 = balances.get(family["vault0"])
        reserve1 = balances.get(family["vault1"])
        if reserve0 and reserve1:
            events.append({
                "signature": signature,
                "reserve0_raw": reserve0,
                "reserve1_raw": reserve1,
                "event_name": family["event_name"],
            })
        return events
    if family.get("event_source") == "event_cpi":
        message = (value.get("transaction") or {}).get("message") or {}
        account_keys = []
        for key in message.get("accountKeys") or []:
            account_keys.append(key.get("pubkey") if isinstance(key, dict) else key)
        loaded = meta.get("loadedAddresses") or {}
        account_keys.extend(loaded.get("writable") or [])
        account_keys.extend(loaded.get("readonly") or [])
        for group in meta.get("innerInstructions") or []:
            for instruction in group.get("instructions") or []:
                program_id_index = instruction.get("programIdIndex", -1)
                if not (0 <= program_id_index < len(account_keys)):
                    continue
                if account_keys[program_id_index] != "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo":
                    continue
                try:
                    raw = b58decode(instruction.get("data", ""))
                except Exception:
                    continue
                if (
                    len(raw) < 84
                    or raw[:8] != family["event_cpi_discriminator"]
                    or raw[8:16] not in family["event_discriminators"]
                    or raw[family["pool_offset"]:family["pool_offset"] + 32] != pool_bytes
                ):
                    continue
                active_id = int.from_bytes(
                    raw[family["active_id_offset"]:family["active_id_offset"] + 4],
                    "little",
                    signed=True,
                )
                events.append({
                    "signature": signature,
                    "active_id": active_id,
                    "event_name": family["event_name"],
                })
        return events
    for log in meta.get("logMessages") or []:
        prefix = "Program data: "
        if not log.startswith(prefix):
            continue
        try:
            raw = base64.b64decode(log[len(prefix):], validate=True)
        except Exception:
            continue
        if (len(raw) != family["event_length"]
                or raw[:8] != family["discriminator"]
                or raw[family["pool_offset"]:family["pool_offset"] + 32] != pool_bytes):
            continue
        post_sqrt_price = int.from_bytes(raw[family["sqrt_offset"]:family["sqrt_offset"] + 16], "little")
        if post_sqrt_price <= 0:
            continue
        events.append({
            "signature": signature,
            "post_sqrt_price_x64": post_sqrt_price,
            "event_name": family["event_name"],
        })
    return events


def price_from_event(event, family):
    decimal_scale = 10 ** (family["token0_decimals"] - family["token1_decimals"])
    if "reserve0_raw" in event:
        return event["reserve1_raw"] / event["reserve0_raw"] * decimal_scale
    if "active_id" in event:
        return (1.0 + family["bin_step"] / 10_000.0) ** event["active_id"] * decimal_scale
    return (event["post_sqrt_price_x64"] / 2**64) ** 2 * decimal_scale


def run(title, pool, family_name, days=1):
    family = FAMILIES[family_name]
    started = time.monotonic()
    pool_bytes = b58decode(pool)
    if len(pool_bytes) != 32:
        raise RuntimeError("pool is not a 32-byte Solana public key")
    slot = rpc_call(title, "getSlot", [{"commitment": "confirmed"}])
    end_time = rpc_call(title, "getBlockTime", [slot])
    if end_time is None:
        raise RuntimeError("latest confirmed slot has no blockTime")
    seconds = days * 86400
    start_time = end_time - seconds
    target_times = [start_time + 3600 * i for i in range(days * 24 + 1)]
    sigs, sig_stats = signatures(title, pool, end_time, lookback_seconds=seconds + 86400)
    # Select candidates around each hourly target. Transaction details are
    # fetched only for the local neighbourhood, not every pool transaction.
    selected = set()
    for target in target_times:
        candidates = [item for item in sigs if item["blockTime"] <= target]
        for item in candidates[-8:]:
            selected.add(item["signature"])
    selected_items = {item["signature"]: item for item in sigs if item["signature"] in selected}
    # Helius free tier is deliberately kept below its advertised throughput;
    # this is a bounded probe, not a parallel history backfiller.
    workers = 1 if title == "helius-free" else 2
    with ThreadPoolExecutor(max_workers=workers) as executor:
        pairs = list(executor.map(
            lambda item: (item["signature"], transaction_events(title, item["signature"], pool_bytes, family)),
            selected_items.values(),
        ))
    events = []
    for signature, parsed in pairs:
        block_time = selected_items[signature]["blockTime"]
        slot_number = selected_items[signature]["slot"]
        for event in parsed:
            events.append({**event, "block_time": block_time, "slot": slot_number})
    events.sort(key=lambda item: (item["block_time"], item["slot"], item["signature"]))
    if not events:
        raise RuntimeError("selected transactions contained no verified Orca Traded event")
    # A price at every target is the last proven post-trade state at or before
    # that target. No event after a target is used and no current price is backfilled.
    series = []
    index = 0
    for target in target_times:
        while index + 1 < len(events) and events[index + 1]["block_time"] <= target:
            index += 1
        if events[index]["block_time"] > target:
            raise RuntimeError("missing proven seed before first one-day sample")
        event = events[index]
        event_values = {}
        if "post_sqrt_price_x64" in event:
            event_values["post_sqrt_price_x64"] = event["post_sqrt_price_x64"]
        elif "active_id" in event:
            event_values["active_id"] = event["active_id"]
        else:
            event_values["reserve0_raw"] = event["reserve0_raw"]
            event_values["reserve1_raw"] = event["reserve1_raw"]
        series.append({
            "sample_timestamp": target,
            "sample_time_utc": datetime.fromtimestamp(target, timezone.utc).isoformat(),
            "source_event_timestamp": event["block_time"],
            "source_event_time_utc": datetime.fromtimestamp(event["block_time"], timezone.utc).isoformat(),
            "source_slot": event["slot"],
            "source_signature": event["signature"],
            **event_values,
            "token1_per_token0": price_from_event(event, family),
        })
    prices = [item["token1_per_token0"] for item in series]
    expected_points = days * 24 + 1
    if len(prices) != expected_points or any(not math.isfinite(value) or value <= 0 for value in prices):
        raise RuntimeError(f"invalid or incomplete {days}-day series")
    returns = [math.log(b / a) for a, b in zip(prices, prices[1:])]
    volatility = statistics.stdev(returns)
    return {
        "status": f"complete_{days}d_probe",
        "chain": "solana",
        "protocol": family["protocol"],
        "pool": pool,
        "pair": family["pair"],
        "price_definition": f"{family['pair'].split('/')[1]} per {family['pair'].split('/')[0]} from {family['event_name']} post_sqrt_price_x64; pool-local spot state",
        "provider_title": title,
        "confirmed_slot": slot,
        "window": {
            "start_timestamp": start_time,
            "end_timestamp": end_time,
            "start_utc": datetime.fromtimestamp(start_time, timezone.utc).isoformat(),
            "end_utc": datetime.fromtimestamp(end_time, timezone.utc).isoformat(),
            "seconds": seconds,
        },
        "signature_index": sig_stats,
        "transaction_details_fetched": len(selected_items),
        "verified_traded_events": len(events),
        "unique_event_signatures": len({item["signature"] for item in events}),
        "sample_points": len(series),
        "returns": len(returns),
        "hourly_log_return_sample_stddev": volatility,
        f"{days}_day_volatility_percent": volatility * 100,
        "volatility_note": f"{len(returns)} one-hour log returns; no annualization; unchanged intervals use the last proven event state",
        "series": series,
        "elapsed_seconds": round(time.monotonic() - started, 3),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", default="publicnode-free", choices=("publicnode-free", "alchemy-free", "helius-free"))
    parser.add_argument("--family", default="orca", choices=FAMILIES)
    parser.add_argument("--pool", default=None)
    parser.add_argument("--days", type=int, default=1, choices=(1, 7))
    args = parser.parse_args()
    family = FAMILIES[args.family]
    print(json.dumps(run(args.provider, args.pool or family["pool"], args.family, args.days), ensure_ascii=False, allow_nan=False, indent=2))
