#!/usr/bin/env python3
"""Resumable, event/state-derived Solana pool price-history backfill.

This is intentionally a small operational backfiller rather than an oracle:

* address history is queried from the Solana RPC provider;
* only verified pool transactions are decoded;
* a target hour uses the last verified state at or before that hour;
* a missing seed remains ``unavailable`` instead of being filled from the
  current account or from an external price feed.

The script writes the shared ``pool_price_history`` table.  It is safe to
rerun the same window because rows are keyed by chain, pool, and timestamp.
Use ``--limit`` for a bounded pilot before removing the limit.
"""

from __future__ import annotations

import argparse
import base64
import bisect
from collections import defaultdict
import json
import math
import sqlite3
import time
import urllib.error
import urllib.request


POOL_PROGRAMS = {
    "meteora": "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
}
METEORA_EVENT_CPI = bytes.fromhex("e445a52e51cb9a1d")
METEORA_SWAP_EVENTS = {
    bytes.fromhex("516ce3becdd00ac4"),  # Swap
    bytes.fromhex("2e7452d7941b544d"),  # Swap2Evt
}
ORCA_TRADED = bytes.fromhex("e1ca49af932ba096")
RAYDIUM_CLMM_SWAP = bytes.fromhex("40c6cde8260871e2")


def b58decode(value: str) -> bytes:
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    number = 0
    for char in value:
        number = number * 58 + alphabet.index(char)
    raw = number.to_bytes((number.bit_length() + 7) // 8, "big") if number else b""
    return b"\0" * (len(value) - len(value.lstrip("1"))) + raw


def rpc_url(title: str) -> str:
    return "http://127.0.0.1:8545/rpc/solana?title=" + title


def rpc_batch(title: str, requests: list[dict], retries: int = 5) -> dict[int, dict]:
    """Send a small batch and return responses keyed by request id."""
    if not requests:
        return {}
    body = json.dumps(requests, separators=(",", ":")).encode()
    request = urllib.request.Request(
        rpc_url(title),
        data=body,
        headers={"Content-Type": "application/json", "User-Agent": "solana-history-backfill"},
    )
    last_error = None
    for attempt in range(retries):
        try:
            with urllib.request.urlopen(request, timeout=90) as response:
                payload = json.loads(response.read(64 * 1024 * 1024 + 1))
            if isinstance(payload, dict):
                payload = [payload]
            return {int(item["id"]): item for item in payload if "id" in item}
        except (OSError, ValueError, urllib.error.HTTPError) as exc:
            last_error = exc
            if attempt + 1 < retries:
                time.sleep(min(2 ** attempt, 8))
    raise RuntimeError(f"RPC batch failed after {retries} attempts: {last_error}")


def rpc_call(title: str, method: str, params: list) -> object:
    response = rpc_batch(title, [{"jsonrpc": "2.0", "id": 1, "method": method, "params": params}])
    item = response.get(1) or {}
    if item.get("error"):
        raise RuntimeError(f"{method}: {item['error']}")
    if "result" not in item:
        raise RuntimeError(f"{method}: missing result")
    return item["result"]


def state_json(row: sqlite3.Row) -> dict:
    try:
        value = json.loads(row["state_json"] or "{}")
    except (TypeError, ValueError):
        value = {}
    return value if isinstance(value, dict) else {}


def load_pools(connection: sqlite3.Connection) -> list[dict]:
    connection.row_factory = sqlite3.Row
    rows = connection.execute(
        """
        SELECT pool_address, pool_family, token0_address, token1_address, state_json
        FROM pool_rpc_state
        WHERE chain = 'solana' AND state_status = 'live'
        ORDER BY pool_address
        """
    ).fetchall()
    pools = []
    for row in rows:
        state = state_json(row)
        token0 = state.get("token0_address") or row["token0_address"]
        token1 = state.get("token1_address") or row["token1_address"]
        if not token0 or not token1:
            continue
        pools.append(
            {
                "address": row["pool_address"],
                "family": row["pool_family"],
                "token0": token0,
                "token1": token1,
                "decimals0": int(state.get("mint_decimals_0", 0)),
                "decimals1": int(state.get("mint_decimals_1", 0)),
                "vault0": state.get("token0_vault"),
                "vault1": state.get("token1_vault"),
                "bin_step": int(state.get("bin_step", 0)),
            }
        )
    return pools


def create_price_table(connection: sqlite3.Connection) -> None:
    connection.execute(
        """
        CREATE TABLE IF NOT EXISTS pool_price_history (
          chain TEXT NOT NULL, pool_address TEXT NOT NULL, timestamp INTEGER NOT NULL,
          token0_address TEXT NOT NULL, token1_address TEXT NOT NULL,
          token1_per_token0 REAL, token0_price_usd REAL, token1_price_usd REAL,
          source TEXT NOT NULL, price_status TEXT NOT NULL DEFAULT 'priced',
          status_reason TEXT, observed_at INTEGER NOT NULL,
          PRIMARY KEY(chain, pool_address, timestamp)
        )
        """
    )


def signature_page_response(response: dict) -> list[dict]:
    if response.get("error"):
        return []
    value = response.get("result")
    return value if isinstance(value, list) else []


def fetch_signatures(
    title: str,
    pool: str,
    initial_page: list[dict],
    cutoff: int,
    max_pages: int,
) -> tuple[list[dict], bool, int]:
    """Return successful signatures and whether the page walk reached cutoff."""
    pages = 1
    page = initial_page
    collected: list[dict] = []
    before = None
    while True:
        collected.extend(
            item for item in page
            if item.get("blockTime") is not None and item.get("err") is None
        )
        oldest = next((item.get("blockTime") for item in reversed(page) if item.get("blockTime") is not None), None)
        if not page or (oldest is not None and oldest <= cutoff):
            return collected, True, pages
        if pages >= max_pages:
            return collected, False, pages
        before = page[-1].get("signature")
        if not before:
            return collected, False, pages
        response = rpc_batch(
            title,
            [{
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getSignaturesForAddress",
                "params": [pool, {"limit": 1000, "commitment": "confirmed", "before": before}],
            }],
        ).get(1, {})
        page = signature_page_response(response)
        pages += 1


def account_keys(value: dict) -> list[str]:
    message = ((value.get("transaction") or {}).get("message") or {})
    keys = [item if isinstance(item, str) else item.get("pubkey") for item in message.get("accountKeys") or []]
    loaded = (value.get("meta") or {}).get("loadedAddresses") or {}
    keys.extend(loaded.get("writable") or [])
    keys.extend(loaded.get("readonly") or [])
    return [item for item in keys if item]


def raw_log_events(value: dict, pool: dict) -> list[dict]:
    family = pool["family"]
    if family == "solana_whirlpool":
        discriminator, length, sqrt_offset = ORCA_TRADED, 121, 57
    elif family == "solana_clmm":
        discriminator, length, sqrt_offset = RAYDIUM_CLMM_SWAP, 221, 169
    else:
        return []
    pool_bytes = b58decode(pool["address"])
    result = []
    for log in (value.get("meta") or {}).get("logMessages") or []:
        prefix = "Program data: "
        if not log.startswith(prefix):
            continue
        try:
            raw = base64.b64decode(log[len(prefix):], validate=True)
        except (ValueError, TypeError):
            continue
        if (
            len(raw) != length
            or raw[:8] != discriminator
            or raw[8:40] != pool_bytes
        ):
            continue
        sqrt_price = int.from_bytes(raw[sqrt_offset:sqrt_offset + 16], "little")
        if sqrt_price > 0:
            result.append({"kind": "sqrt", "value": sqrt_price})
    return result


def meteora_events(value: dict, pool: dict) -> list[dict]:
    keys = account_keys(value)
    pool_bytes = b58decode(pool["address"])
    result = []
    for group in (value.get("meta") or {}).get("innerInstructions") or []:
        for instruction in group.get("instructions") or []:
            index = instruction.get("programIdIndex", -1)
            if not (0 <= index < len(keys)) or keys[index] != POOL_PROGRAMS["meteora"]:
                continue
            try:
                raw = b58decode(instruction.get("data", ""))
            except (ValueError, IndexError):
                continue
            if (
                len(raw) < 84
                or raw[:8] != METEORA_EVENT_CPI
                or raw[8:16] not in METEORA_SWAP_EVENTS
                or raw[16:48] != pool_bytes
            ):
                continue
            active_id = int.from_bytes(raw[80:84], "little", signed=True)
            result.append({"kind": "active_id", "value": active_id})
    return result[-1:]  # one transaction can emit duplicate Swap/Swap2 events


def standard_events(value: dict, pool: dict) -> list[dict]:
    if not pool.get("vault0") or not pool.get("vault1"):
        return []
    keys = account_keys(value)
    balances = {}
    for item in (value.get("meta") or {}).get("postTokenBalances") or []:
        index = item.get("accountIndex", -1)
        if not (0 <= index < len(keys)):
            continue
        amount = (item.get("uiTokenAmount") or {}).get("amount")
        if amount is not None:
            balances[keys[index]] = int(amount)
    reserve0, reserve1 = balances.get(pool["vault0"]), balances.get(pool["vault1"])
    if reserve0 and reserve1:
        return [{"kind": "reserves", "value": (reserve0, reserve1)}]
    return []


def transaction_events(value: dict, pool: dict) -> list[dict]:
    if not value or (value.get("meta") or {}).get("err") is not None:
        return []
    if pool["family"] == "solana_standard":
        return standard_events(value, pool)
    if pool["family"] == "solana_dlmm":
        return meteora_events(value, pool)
    return raw_log_events(value, pool)


def event_price(event: dict, pool: dict) -> float | None:
    scale = 10.0 ** (pool["decimals0"] - pool["decimals1"])
    if event["kind"] == "reserves":
        reserve0, reserve1 = event["value"]
        price = reserve1 / reserve0 * scale
    elif event["kind"] == "active_id":
        if not pool["bin_step"]:
            return None
        price = (1.0 + pool["bin_step"] / 10_000.0) ** event["value"] * scale
    else:
        price = (event["value"] / 2**64) ** 2 * scale
    return price if math.isfinite(price) and price > 0.0 else None


def select_signatures(signatures: list[dict], targets: list[int], neighbors: int) -> list[dict]:
    signatures = sorted(signatures, key=lambda item: (item["blockTime"], item.get("slot", 0), item["signature"]))
    times = [item["blockTime"] for item in signatures]
    selected = {}
    for target in targets:
        end = bisect.bisect_right(times, target)
        for item in signatures[max(0, end - neighbors):end]:
            selected[item["signature"]] = item
    return list(selected.values())


def build_rows(pool: dict, events: list[dict], targets: list[int], now: int, covered: bool) -> list[tuple]:
    events.sort(key=lambda item: (item["block_time"], item.get("slot", 0), item["signature"]))
    rows = []
    index = -1
    reason = "" if events else ("signature_history_not_covered_window" if not covered else "no_verified_price_event")
    for target in targets:
        while index + 1 < len(events) and events[index + 1]["block_time"] <= target:
            index += 1
        if index < 0:
            rows.append((
                "solana", pool["address"], target, pool["token0"], pool["token1"],
                None, None, None, "solana_history", "unavailable", reason or "no_verified_seed", now,
            ))
            continue
        current = events[index]
        rows.append((
            "solana", pool["address"], target, pool["token0"], pool["token1"], current["price"],
            None, None, "solana_history_" + pool["family"], "priced", None, now,
        ))
    return rows


def write_rows(connection: sqlite3.Connection, rows: list[tuple]) -> None:
    connection.executemany(
        """
        INSERT OR REPLACE INTO pool_price_history
        (chain, pool_address, timestamp, token0_address, token1_address,
         token1_per_token0, token0_price_usd, token1_price_usd, source,
         price_status, status_reason, observed_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        rows,
    )


def process_group(title: str, pools: list[dict], targets: list[int], cutoff: int, max_pages: int, neighbors: int, connection: sqlite3.Connection, dry_run: bool) -> tuple[int, int, int]:
    requests = []
    for index, pool in enumerate(pools):
        requests.append({
            "jsonrpc": "2.0", "id": index + 1, "method": "getSignaturesForAddress",
            "params": [pool["address"], {"limit": 1000, "commitment": "confirmed"}],
        })
    responses = rpc_batch(title, requests)
    selected_by_signature: dict[str, list[tuple[dict, dict, bool]]] = defaultdict(list)
    stats = {}
    for index, pool in enumerate(pools):
        initial = signature_page_response(responses.get(index + 1, {}))
        signatures, covered, pages = fetch_signatures(title, pool["address"], initial, cutoff, max_pages)
        selected = select_signatures(signatures, targets, neighbors)
        stats[pool["address"]] = (covered, pages, len(signatures))
        for item in selected:
            selected_by_signature[item["signature"]].append((pool, item, covered))

    values = {}
    signatures = list(selected_by_signature)
    for start in range(0, len(signatures), 8):
        chunk = signatures[start:start + 8]
        responses = rpc_batch(title, [{
            "jsonrpc": "2.0", "id": index + 1, "method": "getTransaction",
            "params": [signature, {"encoding": "json", "commitment": "confirmed", "maxSupportedTransactionVersion": 1}],
        } for index, signature in enumerate(chunk)])
        for index, signature in enumerate(chunk):
            values[signature] = (responses.get(index + 1) or {}).get("result")

    now = int(time.time())
    all_rows = []
    priced = 0
    unavailable = 0
    for pool in pools:
        events = []
        covered = stats[pool["address"]][0]
        for signature, selected in selected_by_signature.items():
            for candidate_pool, item, _ in selected:
                if candidate_pool["address"] != pool["address"]:
                    continue
                for parsed in transaction_events(values.get(signature), pool):
                    price = event_price(parsed, pool)
                    if price is not None:
                        events.append({
                            "signature": signature, "slot": item.get("slot", 0),
                            "block_time": item["blockTime"], "price": price,
                        })
        rows = build_rows(pool, events, targets, now, covered)
        all_rows.extend(rows)
        priced += sum(row[9] == "priced" for row in rows)
        unavailable += sum(row[9] == "unavailable" for row in rows)
    if not dry_run:
        write_rows(connection, all_rows)
        connection.commit()
    return len(pools), priced, unavailable


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", default="data/lp_screener.db")
    parser.add_argument("--price-db", default="data/lp_price_history.db")
    parser.add_argument("--title", default="helius-free")
    parser.add_argument("--days", type=int, default=7, choices=(1, 7))
    parser.add_argument("--points", type=int, default=None, help="hourly points; defaults to days*24")
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--offset", type=int, default=0)
    parser.add_argument("--pool", action="append", default=[])
    parser.add_argument("--max-pages", type=int, default=12)
    parser.add_argument("--neighbors", type=int, default=2)
    parser.add_argument("--pool-batch", type=int, default=8)
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    points = args.points or args.days * 24
    if points < 2:
        raise SystemExit("--points must be at least 2")
    screener = sqlite3.connect(args.db)
    pools = load_pools(screener)
    screener.close()
    if args.pool:
        wanted = set(args.pool)
        pools = [pool for pool in pools if pool["address"] in wanted]
    else:
        pools = pools[args.offset:]
        if args.limit:
            pools = pools[:args.limit]
    latest_slot = int(rpc_call(args.title, "getSlot", [{"commitment": "confirmed"}]))
    latest_time = rpc_call(args.title, "getBlockTime", [latest_slot])
    if latest_time is None:
        raise SystemExit("latest confirmed slot has no blockTime")
    end = int(latest_time) - int(latest_time) % 3_600
    targets = [end - 3_600 * (points - 1 - index) for index in range(points)]
    cutoff = targets[0] - 3_600
    price_db = sqlite3.connect(args.price_db)
    create_price_table(price_db)
    print(json.dumps({"pools": len(pools), "points": points, "start": targets[0], "end": targets[-1], "dry_run": args.dry_run}, ensure_ascii=False), flush=True)
    total_priced = total_unavailable = 0
    for start in range(0, len(pools), args.pool_batch):
        group = pools[start:start + args.pool_batch]
        count, priced, unavailable = process_group(
            args.title, group, targets, cutoff, args.max_pages, args.neighbors, price_db, args.dry_run,
        )
        total_priced += priced
        total_unavailable += unavailable
        print(json.dumps({"processed": start + count, "total": len(pools), "priced_rows": priced, "unavailable_rows": unavailable}, ensure_ascii=False), flush=True)
    price_db.close()
    print(json.dumps({"complete": len(pools), "priced_rows": total_priced, "unavailable_rows": total_unavailable}, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
