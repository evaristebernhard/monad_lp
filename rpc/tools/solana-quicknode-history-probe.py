#!/usr/bin/env python3
"""Single-pool QuickNode getTransactionsForAddress volatility probe.

This probe deliberately does not write SQLite.  It fetches full transaction
pages for one pool, decodes the same protocol-local events as the backfiller,
then carries the last verified event into empty hourly buckets.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import math
import statistics
import time


def load_backfill_module():
    spec = importlib.util.spec_from_file_location(
        "solana_history_backfill", "rpc/tools/solana-history-backfill.py"
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load solana-history-backfill.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def fetch_transactions(module, title, pool, start, end, page_limit):
    token = None
    pages = 0
    records = []
    while True:
        options = {
            "transactionDetails": "full",
            "sortOrder": "desc",
            "limit": page_limit,
            "encoding": "json",
            "maxSupportedTransactionVersion": 1,
        }
        if token:
            options["paginationToken"] = token
        result = module.rpc_call(title, "getTransactionsForAddress", [pool, options])
        data = result.get("data") or []
        pages += 1
        records.extend(
            item for item in data
            if item.get("blockTime") is not None
            and start - 3_600 <= item["blockTime"] <= end
            and (item.get("meta") or {}).get("err") is None
        )
        oldest = min((item.get("blockTime") for item in data if item.get("blockTime") is not None), default=None)
        token = result.get("paginationToken")
        if not data or oldest is None or oldest < start - 3_600 or not token:
            break
        if pages > 200:
            raise RuntimeError("pagination exceeded 200 pages; stop rather than scan unbounded history")
    deduped = {}
    for item in records:
        signatures = ((item.get("transaction") or {}).get("signatures") or [])
        signature = signatures[0] if signatures else f"{item.get('slot')}:{item.get('transactionIndex')}"
        deduped[signature] = item
    return list(deduped.values()), pages


def run(args):
    module = load_backfill_module()
    pool = args.pool
    connection = __import__("sqlite3").connect(args.db)
    connection.row_factory = __import__("sqlite3").Row
    pools = [item for item in module.load_pools(connection) if item["address"] == pool]
    connection.close()
    if not pools:
        raise RuntimeError(f"live Solana pool not found: {pool}")
    metadata = pools[0]
    slot = int(module.rpc_call(args.title, "getSlot", [{"commitment": "confirmed"}]))
    latest = int(module.rpc_call(args.title, "getBlockTime", [slot]))
    end = latest - latest % 3_600
    points = args.days * 24
    targets = [end - 3_600 * (points - 1 - index) for index in range(points)]
    started = time.monotonic()
    transactions, pages = fetch_transactions(module, args.title, pool, targets[0], end, args.page_limit)
    events = []
    for item in transactions:
        signatures = ((item.get("transaction") or {}).get("signatures") or [])
        signature = signatures[0] if signatures else f"{item.get('slot')}:{item.get('transactionIndex')}"
        for parsed in module.transaction_events(item, metadata):
            price = module.event_price(parsed, metadata)
            if price is not None:
                events.append({
                    "signature": signature,
                    "slot": item.get("slot", 0),
                    "block_time": item["blockTime"],
                    "price": price,
                })
    events.sort(key=lambda item: (item["block_time"], item["slot"], item["signature"]))
    if not events:
        raise RuntimeError("no verified protocol-local swap/state events in the QuickNode window")
    prices = []
    index = -1
    for target in targets:
        while index + 1 < len(events) and events[index + 1]["block_time"] <= target:
            index += 1
        if index < 0:
            raise RuntimeError(f"missing verified seed before target {target}")
        prices.append(events[index]["price"])
    returns = [math.log(right / left) for left, right in zip(prices, prices[1:])]
    return {
        "status": "complete_probe",
        "provider": args.title,
        "pool": pool,
        "family": metadata["family"],
        "points": len(prices),
        "returns": len(returns),
        "window": {"start": targets[0], "end": targets[-1], "latest_provider_time": latest},
        "transaction_pages": pages,
        "transactions_in_window": len(transactions),
        "verified_events": len(events),
        "hourly_log_return_sample_stddev": statistics.stdev(returns),
        "volatility_percent": statistics.stdev(returns) * 100,
        "min_price": min(prices),
        "max_price": max(prices),
        "elapsed_seconds": round(time.monotonic() - started, 3),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--title", default="quicknode-paid")
    parser.add_argument("--pool", default="J27e5izvX4nbaaRDjMKv7DogQzcPidCAECxzE6rK4bF7")
    parser.add_argument("--db", default="data/lp_screener.db")
    parser.add_argument("--days", type=int, default=7, choices=(1, 7))
    parser.add_argument("--page-limit", type=int, default=300)
    args = parser.parse_args()
    print(json.dumps(run(args), ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
