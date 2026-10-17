#!/usr/bin/env python3
"""Fill token USD history from low-cost market-data sources.

Source order for tokens that do not already have sufficient DefiLlama history:

1. Binance 1h spot klines for an explicitly mapped canonical asset;
2. GeckoTerminal hourly OHLCV from the best known representative pool;
3. no value when the token identity or market history cannot be proven.

The script writes the existing price database schema and a small provenance
table. It never writes pool-local prices and never calls a chain RPC.
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import json
import math
import os
import re
import sqlite3
import time
import urllib.error
import urllib.parse
import urllib.request


HOUR = 3_600
BINANCE_EXCHANGE_INFO = "https://data-api.binance.vision/api/v3/exchangeInfo"
BINANCE_KLINES = "https://data-api.binance.vision/api/v3/klines"
GECKO_OHLCV = "https://api.geckoterminal.com/api/v2/networks/solana/pools/{pool}/ohlcv/hour"
GECKO_TIMEOUT = 5
DEFILLAMA_CHART = "https://coins.llama.fi/chart/"
GECKO_API_KEY = ""
USD_QUOTES = ("USDT", "USDC", "FDUSD", "BUSD", "TUSD")
KNOWN_ASSETS = {
    "So11111111111111111111111111111111111111112": "SOL",
    "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v": "USDC",
    "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB": "USDT",
    "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R": "RAY",
}


def http_json(url: str, timeout: int = 45) -> object:
    headers = {"accept": "application/json", "user-agent": "lp-market-history/0.1"}
    if GECKO_API_KEY and url.startswith(GECKO_OHLCV.split("{", 1)[0]):
        headers["x-cg-pro-api-key"] = GECKO_API_KEY
    request = urllib.request.Request(
        url,
        headers=headers,
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read(32 * 1024 * 1024 + 1))


def normalize_symbol(value: str | None) -> str:
    value = (value or "").strip().upper()
    if value in {"WSOL", "W-SOL"}:
        return "SOL"
    if value in {"USDC.E", "USDC-E", "USDC_E"}:
        return "USDC"
    return value.replace("$", "").replace("-", "")


def aligned_window(days: int = 7) -> tuple[int, list[int]]:
    end = int(time.time()) // HOUR * HOUR
    start = end - days * 86_400
    return end, list(range(start, end, HOUR))


def ensure_schema(db: sqlite3.Connection) -> None:
    db.executescript(
        """
        CREATE TABLE IF NOT EXISTS token_price_history (
          chain TEXT NOT NULL,
          address TEXT NOT NULL,
          timestamp INTEGER NOT NULL,
          price_usd REAL NOT NULL,
          source TEXT NOT NULL,
          observed_at INTEGER NOT NULL,
          PRIMARY KEY (chain, address, timestamp)
        );
        CREATE TABLE IF NOT EXISTS token_market_source (
          chain TEXT NOT NULL,
          address TEXT NOT NULL,
          canonical_asset TEXT,
          source TEXT NOT NULL,
          market_identifier TEXT,
          representative_pool TEXT,
          confidence TEXT NOT NULL,
          checked_at INTEGER NOT NULL,
          status TEXT NOT NULL,
          error TEXT,
          PRIMARY KEY (chain, address, source)
        )
        """
    )
    db.commit()


def load_catalog(db: sqlite3.Connection, addresses: set[str] | None) -> tuple[dict, dict]:
    rows = db.execute(
        """
        SELECT pool_address, token_a_address, token_a_symbol, token_b_address,
               token_b_symbol, coalesce(liquidity_usd, 0), coalesce(volume_24h_usd, 0)
        FROM pools WHERE chain = 'solana'
        """
    ).fetchall()
    symbols: dict[str, set[str]] = {}
    candidates: dict[str, list[tuple[float, str, str]]] = {}
    for pool, a, sa, b, sb, liquidity, volume in rows:
        score = (float(liquidity) if liquidity else 0.0) * 1_000_000 + (float(volume) if volume else 0.0)
        for address, symbol, other in ((a, sa, b), (b, sb, a)):
            if not address:
                continue
            symbols.setdefault(address, set()).add(normalize_symbol(symbol))
            candidates.setdefault(address, []).append((score, pool, other or ""))
    if addresses:
        symbols = {key: value for key, value in symbols.items() if key in addresses}
        candidates = {key: value for key, value in candidates.items() if key in addresses}
    return symbols, candidates


def existing_defillama(db: sqlite3.Connection, addresses: set[str], start: int, end: int) -> set[str]:
    if not addresses:
        return set()
    placeholders = ",".join("?" for _ in addresses)
    rows = db.execute(
        f"""
        SELECT address, count(*), min(timestamp), max(timestamp)
        FROM token_price_history
        WHERE chain = 'solana' AND source = 'defillama'
          AND address IN ({placeholders}) AND timestamp >= ? AND timestamp < ?
        GROUP BY address
        """,
        [*addresses, start, end],
    ).fetchall()
    # DefiLlama is often daily/sparse. Four points spanning most of the week
    # are enough for an approximate low-frequency token volatility estimate;
    # otherwise prefer a higher-resolution fallback.
    return {
        address for address, count, first, last in rows
        if count >= 4 and first is not None and last - first >= 5 * 86_400
    }


def existing_market_covered(db: sqlite3.Connection, addresses: set[str], start: int, end: int) -> set[str]:
    if not addresses:
        return set()
    placeholders = ",".join("?" for _ in addresses)
    rows = db.execute(
        f"""
        SELECT address, count(*), min(timestamp), max(timestamp)
        FROM token_price_history
        WHERE chain = 'solana' AND source IN ('binance_1h', 'geckoterminal_pool_1h')
          AND address IN ({placeholders}) AND timestamp >= ? AND timestamp < ?
        GROUP BY address
        """,
        [*addresses, start, end],
    ).fetchall()
    # Gecko may legitimately miss the first candle, so use a broad minimum
    # while still requiring most of the seven-day span.
    return {
        address for address, count, first, last in rows
        if count >= 120 and first is not None and last - first >= 5 * 86_400
    }


def load_defillama_points(db: sqlite3.Connection, addresses: set[str], start: int, end: int) -> dict[str, list[tuple[int, float]]]:
    if not addresses:
        return {}
    placeholders = ",".join("?" for _ in addresses)
    rows = db.execute(
        f"""
        SELECT address, timestamp, price_usd
        FROM token_price_history
        WHERE chain = 'solana' AND source = 'defillama'
          AND address IN ({placeholders}) AND timestamp < ?
        ORDER BY address, timestamp
        """,
        [*addresses, end],
    ).fetchall()
    result: dict[str, list[tuple[int, float]]] = {}
    for address, timestamp, price in rows:
        if price is not None and math.isfinite(price) and price > 0:
            result.setdefault(address, []).append((int(timestamp), float(price)))
    return result


def fetch_defillama_batch(addresses: list[str], span_days: int) -> dict[str, list[tuple[int, float]]]:
    if not addresses:
        return {}
    identifiers = ",".join(f"solana:{address}" for address in addresses)
    payload = http_json(
        f"{DEFILLAMA_CHART}{identifiers}?span={max(span_days, 30)}d",
        timeout=30,
    )
    result: dict[str, list[tuple[int, float]]] = {}
    for key, coin in (payload.get("coins") or {}).items():
        address = key.split(":", 1)[-1]
        points = []
        for point in coin.get("prices") or []:
            try:
                if isinstance(point, dict):
                    timestamp, price = int(point["timestamp"]), float(point["price"])
                else:
                    timestamp, price = int(point[0]), float(point[1])
            except (KeyError, IndexError, TypeError, ValueError):
                continue
            if timestamp > 0 and math.isfinite(price) and price > 0:
                points.append((timestamp, price))
        if points:
            result[address] = points
    return result


def fetch_defillama_resilient(addresses: list[str], span_days: int) -> tuple[dict[str, list[tuple[int, float]]], int]:
    try:
        return fetch_defillama_batch(addresses, span_days), 0
    except Exception:
        if len(addresses) <= 1:
            return {}, 1
        midpoint = len(addresses) // 2
        left, left_errors = fetch_defillama_resilient(addresses[:midpoint], span_days)
        right, right_errors = fetch_defillama_resilient(addresses[midpoint:], span_days)
        left.update(right)
        return left, left_errors + right_errors


def carry_points(points: list[tuple[int, float]], targets: list[int]) -> list[tuple[int, float]]:
    result = []
    index = 0
    previous = None
    points = sorted(points)
    for target in targets:
        while index < len(points) and points[index][0] <= target:
            previous = points[index][1]
            index += 1
        if previous is not None:
            result.append((target, previous))
    return result


def fetch_exchange_info() -> tuple[set[str], dict[str, str]]:
    payload = http_json(BINANCE_EXCHANGE_INFO, timeout=12)
    symbols = set()
    chosen: dict[str, str] = {}
    for item in (payload.get("symbols") or []):
        if item.get("status") != "TRADING" or not item.get("isSpotTradingAllowed", True):
            continue
        symbol = item.get("symbol")
        base = item.get("baseAsset")
        quote = item.get("quoteAsset")
        if not symbol or not base or not quote:
            continue
        symbols.add(symbol)
        if quote in USD_QUOTES and base not in chosen:
            chosen[base] = symbol
        elif quote in USD_QUOTES and quote == "USDT":
            chosen[base] = symbol
    return symbols, chosen


def token_asset(address: str, symbols: dict[str, set[str]]) -> tuple[str | None, str]:
    if address in KNOWN_ASSETS:
        return KNOWN_ASSETS[address], "known_address"
    values = {normalize_symbol(item) for item in symbols.get(address, set()) if item}
    values.discard("")
    if len(values) == 1:
        return next(iter(values)), "catalog_symbol"
    return None, "ambiguous_or_missing_symbol"


def fetch_binance(asset: str, market: str, targets: list[int]) -> list[tuple[int, float]]:
    if asset == "USDT":
        return [(timestamp, 1.0) for timestamp in targets]
    start = targets[0] * 1_000
    end = (targets[-1] + HOUR) * 1_000 - 1
    query = urllib.parse.urlencode({
        "symbol": market, "interval": "1h", "startTime": start,
        "endTime": end, "limit": len(targets),
    })
    payload = http_json(BINANCE_KLINES + "?" + query, timeout=12)
    result = []
    for row in payload if isinstance(payload, list) else []:
        try:
            timestamp = int(row[0]) // 1_000
            price = float(row[4])
        except (IndexError, TypeError, ValueError):
            continue
        if timestamp in targets and math.isfinite(price) and price > 0:
            result.append((timestamp, price))
    return result


def fetch_gecko(pool: str, address: str, targets: list[int]) -> list[tuple[int, float]]:
    query = urllib.parse.urlencode({
        "aggregate": 1, "limit": len(targets), "currency": "usd", "token": address,
    })
    payload = http_json(GECKO_OHLCV.format(pool=pool) + "?" + query, timeout=GECKO_TIMEOUT)
    rows = (((payload.get("data") or {}).get("attributes") or {}).get("ohlcv_list") or [])
    values = {}
    for row in rows:
        try:
            timestamp, price = int(row[0]), float(row[4])
        except (IndexError, TypeError, ValueError):
            continue
        if timestamp in targets and math.isfinite(price) and price > 0:
            values[timestamp] = price
    # Fill only after the first observed candle. This carries a known market
    # price through a no-trade hour but never invents a seed before coverage.
    result = []
    previous = None
    for timestamp in targets:
        if timestamp in values:
            previous = values[timestamp]
        if previous is not None:
            result.append((timestamp, previous))
    return result


def load_gecko_api_key(path: str) -> str:
    if not path:
        return os.environ.get("GECKOTERMINAL_API_KEY", "")
    text = open(path, encoding="utf-8").read()
    match = re.search(r"API_KEY\s*=\s*[\"']([^\"']+)", text)
    if not match:
        raise ValueError(f"no API_KEY assignment found in {path}")
    return match.group(1).strip()


def write_provenance(db: sqlite3.Connection, address: str, asset: str | None, source: str, market: str | None, pool: str | None, confidence: str, status: str, error: str | None) -> None:
    db.execute(
        """
        INSERT OR REPLACE INTO token_market_source
        (chain,address,canonical_asset,source,market_identifier,representative_pool,confidence,checked_at,status,error)
        VALUES ('solana',?,?,?,?,?,?,?,?,?)
        """,
        (address, asset, source, market, pool, confidence, int(time.time()), status, error),
    )


def write_prices(db: sqlite3.Connection, address: str, points: list[tuple[int, float]], source: str) -> int:
    count = 0
    for timestamp, price in points:
        count += db.execute(
            """
            INSERT INTO token_price_history(chain,address,timestamp,price_usd,source,observed_at)
            VALUES ('solana',?,?,?,?,?)
            ON CONFLICT(chain,address,timestamp) DO UPDATE SET
              price_usd=excluded.price_usd, source=excluded.source, observed_at=excluded.observed_at
            WHERE token_price_history.source NOT IN ('defillama')
            """,
            (address, timestamp, price, source, int(time.time())),
        ).rowcount
    return count


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", default="data/lp_screener.db")
    parser.add_argument("--price-db", default="data/lp_price_history.db")
    parser.add_argument("--days", type=int, default=7)
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--address", action="append", default=[])
    parser.add_argument("--gecko-delay", type=float, default=3.0)
    parser.add_argument("--gecko-api-key-file", default="")
    parser.add_argument("--source", choices=("all", "defillama", "binance", "gecko"), default="all")
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args()


def main() -> None:
    global GECKO_API_KEY
    args = parse_args()
    GECKO_API_KEY = load_gecko_api_key(args.gecko_api_key_file)
    end, targets = aligned_window(args.days)
    start = targets[0]
    addresses_filter = set(args.address) or None
    catalog = sqlite3.connect(args.db)
    symbols, candidates = load_catalog(catalog, addresses_filter)
    addresses = set(symbols)
    if args.limit:
        addresses = set(sorted(addresses)[:args.limit])
    price_db = sqlite3.connect(args.price_db)
    ensure_schema(price_db)
    defillama_covered = existing_defillama(price_db, addresses, start, end)
    market_covered = existing_market_covered(price_db, addresses, start, end)
    defillama_fetched = 0
    defillama_errors = 0
    if args.source in ("all", "defillama") and not args.dry_run:
        llama_candidates = sorted(addresses - defillama_covered)
        for offset in range(0, len(llama_candidates), 10):
            batch = llama_candidates[offset : offset + 10]
            batch_points, batch_errors = fetch_defillama_resilient(batch, args.days)
            defillama_errors += batch_errors
            for address, points in batch_points.items():
                write_prices(price_db, address, points, "defillama")
                write_provenance(price_db, address, None, "defillama", None, None, "defillama", "priced", None)
                defillama_fetched += 1
            price_db.commit()
            if offset and offset % 100 == 0:
                print(json.dumps({"defillama_batches": offset // 10, "defillama_fetched": defillama_fetched}, ensure_ascii=False), flush=True)
        defillama_covered = existing_defillama(price_db, addresses, start, end)
    elif args.dry_run and args.source in ("all", "defillama"):
        defillama_fetched = len(addresses - defillama_covered)
    market_covered = existing_market_covered(price_db, addresses, start, end)
    covered = defillama_covered | market_covered
    pending = addresses - covered
    print(json.dumps({"addresses": len(addresses), "defillama_covered": len(defillama_covered), "defillama_fetched": defillama_fetched, "defillama_errors": defillama_errors, "market_covered": len(market_covered), "pending": len(pending), "start": start, "end": end, "dry_run": args.dry_run}, ensure_ascii=False), flush=True)

    defillama_carry = 0
    for address, points in load_defillama_points(price_db, covered, start, end).items():
        carried = carry_points(points, targets)
        if not args.dry_run:
            write_prices(price_db, address, carried, "defillama_hourly_carry")
            write_provenance(price_db, address, None, "defillama_hourly_carry", None, None, "defillama", "priced" if carried else "missing", None)
        defillama_carry += bool(carried)
    price_db.commit()

    markets = {}
    if args.source in ("all", "binance") and pending:
        try:
            _, choices = fetch_exchange_info()
        except Exception as error:
            print(json.dumps({"binance_exchange_info_error": str(error)}, ensure_ascii=False), flush=True)
            choices = {}
        for address in pending:
            asset, confidence = token_asset(address, symbols)
            if asset and asset in choices:
                markets[address] = (asset, choices[asset], confidence)

    done = 0
    binance_priced_addresses: set[str] = set()
    binance_points_cache: dict[tuple[str, str], list[tuple[int, float]]] = {}
    binance_error_cache: dict[tuple[str, str], str] = {}
    if markets and not args.dry_run:
        unique_markets = sorted({(asset, market) for asset, market, _ in markets.values()})
        with ThreadPoolExecutor(max_workers=8) as executor:
            futures = {
                executor.submit(fetch_binance, asset, market, targets): (asset, market)
                for asset, market in unique_markets
            }
            for future in as_completed(futures):
                cache_key = futures[future]
                try:
                    binance_points_cache[cache_key] = future.result()
                except Exception as error:
                    binance_error_cache[cache_key] = str(error)[:500]
                if (len(binance_points_cache) + len(binance_error_cache)) % 25 == 0:
                    print(json.dumps({"binance_market_fetches": len(binance_points_cache) + len(binance_error_cache), "binance_market_total": len(unique_markets)}, ensure_ascii=False), flush=True)
    for address, (asset, market, confidence) in markets.items():
        cache_key = (asset, market)
        try:
            points = binance_points_cache.get(cache_key, [])
            if not args.dry_run:
                write_prices(price_db, address, points, "binance_1h")
            if not args.dry_run:
                status = "error" if cache_key in binance_error_cache else ("priced" if points else "missing")
                write_provenance(price_db, address, asset, "binance_1h", market, None, confidence, status, binance_error_cache.get(cache_key))
            if points:
                done += 1
                binance_priced_addresses.add(address)
        except Exception as error:
            binance_error_cache[cache_key] = str(error)[:500]
            if not args.dry_run:
                write_provenance(price_db, address, asset, "binance_1h", market, None, confidence, "error", str(error)[:500])
        if done and done % 25 == 0:
            print(json.dumps({"binance_priced": done}, ensure_ascii=False), flush=True)
    price_db.commit()

    # A Binance symbol mapping is only a candidate.  If its klines fail or
    # return no usable candles, continue to the representative-pool fallback.
    gecko_pending = pending - binance_priced_addresses
    gecko_done = 0
    gecko_attempted = 0
    if args.source in ("all", "gecko") and not args.dry_run:
        ranked = {}
        for address in gecko_pending:
            rows = sorted(candidates.get(address, []), reverse=True)
            if rows:
                ranked[address] = rows[0][1]
        for address, pool in sorted(ranked.items()):
            gecko_attempted += 1
            rate_limited = False
            try:
                points = fetch_gecko(pool, address, targets)
                if not args.dry_run:
                    write_prices(price_db, address, points, "geckoterminal_pool_1h")
                if not args.dry_run:
                    write_provenance(price_db, address, None, "geckoterminal_pool_1h", None, pool, "representative_pool", "priced" if points else "missing", None)
                gecko_done += bool(points)
            except Exception as error:
                error_text = str(error)[:500]
                if not args.dry_run:
                    write_provenance(price_db, address, None, "geckoterminal_pool_1h", None, pool, "representative_pool", "error", error_text)
                rate_limited = "429" in error_text or "Too Many Requests" in error_text
            price_db.commit()
            if rate_limited:
                print(json.dumps({"gecko_rate_limited": True, "gecko_attempted": gecko_attempted, "pause_seconds": 60}, ensure_ascii=False), flush=True)
                time.sleep(60)
            if args.gecko_delay > 0:
                time.sleep(args.gecko_delay)
            if gecko_attempted % 25 == 0:
                print(json.dumps({"gecko_attempted": gecko_attempted, "gecko_priced": gecko_done, "gecko_total": len(ranked)}, ensure_ascii=False), flush=True)

    price_db.commit()
    price_db.close()
    catalog.close()
    print(json.dumps({"complete": True, "defillama_covered": len(defillama_covered), "defillama_fetched": defillama_fetched, "defillama_errors": defillama_errors, "market_covered": len(market_covered), "defillama_hourly_carry": defillama_carry, "binance_candidates": len(markets), "binance_unique_markets": len({(asset, market) for asset, market, _ in markets.values()}), "binance_priced": done, "gecko_candidates": len(gecko_pending), "gecko_attempted": gecko_attempted, "gecko_priced": gecko_done}, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
