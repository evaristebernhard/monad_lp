#!/usr/bin/env python3
"""Backfill three days of Meteora-pool candles and Deribit option trades."""
import argparse
import datetime as dt
import json
import sqlite3
import time
import urllib.parse
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GECKO = "https://api.geckoterminal.com/api/v2/networks/solana/pools/{pool}/ohlcv/hour"
DERIBIT = "https://www.deribit.com/api/v2/"
SOL_MINT = "So11111111111111111111111111111111111111112"


def utc_now():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def get_json(url, timeout=30):
    for attempt in range(4):
        request = urllib.request.Request(
            url, headers={"accept": "application/json", "user-agent": "lp-history-backfill/1.0"}
        )
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return json.loads(response.read(32 * 1024 * 1024 + 1))
        except urllib.error.HTTPError as exc:
            if exc.code != 429 or attempt == 3:
                raise
            time.sleep(max(float(exc.headers.get("Retry-After", 20 * (attempt + 1))), 1.0))


def deribit_call(method, params):
    body = json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": method, "params": params},
        separators=(",", ":"),
    ).encode()
    for attempt in range(5):
        request = urllib.request.Request(
            DERIBIT + method,
            data=body,
            headers={"content-type": "application/json", "user-agent": "lp-history-backfill/1.0"},
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                payload = json.loads(response.read(16 * 1024 * 1024 + 1))
            break
        except urllib.error.HTTPError as exc:
            if exc.code != 429 or attempt == 4:
                raise
            time.sleep(max(float(exc.headers.get("Retry-After", 2 * (attempt + 1))), 1.0))
    if payload.get("error"):
        raise RuntimeError("deribit_error_" + str(payload["error"].get("code", "unknown")))
    return payload.get("result") or {}


def ensure_schema(db):
    db.executescript(
        """
        CREATE TABLE IF NOT EXISTS pool_market_candles (
          pool_address TEXT NOT NULL,
          timestamp_utc INTEGER NOT NULL,
          open_usd REAL NOT NULL,
          high_usd REAL NOT NULL,
          low_usd REAL NOT NULL,
          close_usd REAL NOT NULL,
          volume_usd REAL,
          source TEXT NOT NULL,
          fetched_at_utc TEXT NOT NULL,
          raw_json TEXT NOT NULL,
          PRIMARY KEY(pool_address,timestamp_utc)
        );
        CREATE TABLE IF NOT EXISTS deribit_historical_trades (
          instrument_name TEXT NOT NULL,
          trade_id TEXT NOT NULL,
          timestamp_ms INTEGER NOT NULL,
          direction TEXT,
          price_usdc_per_sol REAL NOT NULL,
          amount_sol REAL,
          contracts REAL,
          index_price_usdc_per_sol REAL,
          mark_price_usdc_per_sol REAL,
          iv_percent REAL,
          raw_json TEXT NOT NULL,
          PRIMARY KEY(instrument_name,trade_id)
        );
        CREATE INDEX IF NOT EXISTS idx_deribit_hist_trades_time
          ON deribit_historical_trades(timestamp_ms);
        CREATE TABLE IF NOT EXISTS deribit_history_fetches (
          instrument_name TEXT NOT NULL,
          start_timestamp_ms INTEGER NOT NULL,
          end_timestamp_ms INTEGER NOT NULL,
          status TEXT NOT NULL,
          trade_count INTEGER NOT NULL,
          truncated INTEGER NOT NULL,
          error_type TEXT,
          fetched_at_utc TEXT NOT NULL,
          PRIMARY KEY(instrument_name,start_timestamp_ms,end_timestamp_ms)
        );
        """
    )
    columns = {row[1] for row in db.execute("PRAGMA table_info(pool_market_candles)")}
    if "volume_token" in columns and "volume_usd" not in columns:
        db.execute("ALTER TABLE pool_market_candles RENAME COLUMN volume_token TO volume_usd")
    db.commit()


def backfill_pool_candles(db, pool_addresses, start_s, end_s, pause_s):
    completed = 0
    errors = 0
    fetched_at = utc_now()
    for index, pool in enumerate(pool_addresses):
        query = urllib.parse.urlencode(
            # Request extra leading bars so the current, still-open hour does
            # not displace one of the requested 72 completed hourly candles.
            {"aggregate": 1, "limit": 100, "currency": "usd", "token": SOL_MINT}
        )
        try:
            payload = get_json(GECKO.format(pool=pool) + "?" + query, timeout=15)
            rows = (((payload.get("data") or {}).get("attributes") or {}).get("ohlcv_list") or [])
            stored = 0
            for row in rows:
                if len(row) < 6:
                    continue
                timestamp = int(row[0])
                if not start_s <= timestamp < end_s:
                    continue
                db.execute(
                    """INSERT OR REPLACE INTO pool_market_candles
                       (pool_address,timestamp_utc,open_usd,high_usd,low_usd,
                        close_usd,volume_usd,source,fetched_at_utc,raw_json)
                       VALUES(?,?,?,?,?,?,?,?,?,?)""",
                    (
                        pool, timestamp, float(row[1]), float(row[2]), float(row[3]),
                        float(row[4]), float(row[5]), "geckoterminal_pool_1h",
                        fetched_at, json.dumps(row, separators=(",", ":")),
                    ),
                )
                stored += 1
            db.commit()
            completed += int(stored > 0)
            print(json.dumps({"pool": pool, "candles": stored, "status": "ok" if stored else "empty"}), flush=True)
        except Exception as exc:
            db.rollback()
            errors += 1
            print(json.dumps({"pool": pool, "candles": 0, "status": "error", "error_type": type(exc).__name__}), flush=True)
        if index + 1 < len(pool_addresses):
            time.sleep(pause_s)
    return completed, errors


def store_trade(db, instrument, trade):
    trade_id = str(trade.get("trade_id") or f"{trade.get('timestamp')}:{trade.get('trade_seq')}")
    db.execute(
        """INSERT OR IGNORE INTO deribit_historical_trades
           (instrument_name,trade_id,timestamp_ms,direction,price_usdc_per_sol,
            amount_sol,contracts,index_price_usdc_per_sol,mark_price_usdc_per_sol,
            iv_percent,raw_json) VALUES(?,?,?,?,?,?,?,?,?,?,?)""",
        (
            instrument, trade_id, int(trade["timestamp"]), trade.get("direction"),
            float(trade["price"]), trade.get("amount"), trade.get("contracts"),
            trade.get("index_price"), trade.get("mark_price"), trade.get("iv"),
            json.dumps(trade, separators=(",", ":")),
        ),
    )


def fetch_trade_window(db, instrument, start_ms, end_ms, depth=0):
    result = deribit_call(
        "public/get_last_trades_by_instrument_and_time",
        {
            "instrument_name": instrument,
            "start_timestamp": start_ms,
            "end_timestamp": end_ms,
            "count": 1000,
            "sorting": "asc",
        },
    )
    trades = result.get("trades") or []
    for trade in trades:
        if start_ms <= int(trade.get("timestamp", 0)) <= end_ms:
            store_trade(db, instrument, trade)
    db.commit()
    if not result.get("has_more"):
        return len(trades), False
    # The API caps a page at 1,000; subdivide overloaded intervals so that
    # dense instruments do not silently lose the middle of the time window.
    if depth >= 8 or end_ms - start_ms <= 60_000:
        return len(trades), True
    midpoint = (start_ms + end_ms) // 2
    left_count, left_truncated = fetch_trade_window(db, instrument, start_ms, midpoint, depth + 1)
    right_count, right_truncated = fetch_trade_window(db, instrument, midpoint + 1, end_ms, depth + 1)
    return left_count + right_count, left_truncated or right_truncated


def backfill_deribit(db, instruments, start_ms, end_ms, pause_s):
    complete = 0
    errors = 0
    for index, instrument in enumerate(instruments):
        try:
            count, truncated = fetch_trade_window(db, instrument, start_ms, end_ms)
            db.execute(
                """INSERT OR REPLACE INTO deribit_history_fetches
                   VALUES(?,?,?,?,?,?,?,?)""",
                (instrument, start_ms, end_ms, "truncated" if truncated else "ok",
                 count, int(truncated), None, utc_now()),
            )
            db.commit()
            complete += int(not truncated)
        except Exception as exc:
            db.rollback()
            errors += 1
            error_name = (
                f"http_{exc.code}"
                if isinstance(exc, urllib.error.HTTPError)
                else type(exc).__name__
            )
            db.execute(
                """INSERT OR REPLACE INTO deribit_history_fetches
                   VALUES(?,?,?,?,?,?,?,?)""",
                (instrument, start_ms, end_ms, "error", 0, 0, error_name, utc_now()),
            )
            db.commit()
        if index + 1 < len(instruments):
            time.sleep(pause_s)
    return complete, errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, default=ROOT / "data/sol_dlmm_deribit.db")
    parser.add_argument("--hours", type=int, default=72)
    parser.add_argument("--pool", action="append", default=[], help="limit GeckoTerminal backfill to these pool addresses")
    parser.add_argument("--missing-pools", action="store_true", help="select pools without a complete hourly candle window")
    parser.add_argument("--instrument", action="append", default=[], help="limit Deribit backfill to these option instruments")
    parser.add_argument("--only-deribit", action="store_true", help="skip pool OHLCV calls")
    parser.add_argument("--only-pools", action="store_true", help="skip Deribit trade calls")
    parser.add_argument("--pool-pause", type=float, default=2.1)
    parser.add_argument("--deribit-pause", type=float, default=0.25)
    args = parser.parse_args()
    if args.hours <= 0:
        parser.error("--hours must be positive")

    db = sqlite3.connect(args.db)
    ensure_schema(db)
    end_ms = int(time.time() * 1000)
    start_ms = end_ms - args.hours * 3_600_000
    end_s_aligned = end_ms // 3_600_000 * 3_600
    start_s_aligned = end_s_aligned - args.hours * 3600
    pools = [] if args.only_deribit else [
        row[0]
        for row in db.execute(
            """SELECT p.pool_address FROM pool_snapshots p
               JOIN (SELECT pool_address,max(snapshot_id) sid FROM pool_snapshots GROUP BY pool_address) x
                 ON p.snapshot_id=x.sid
               WHERE p.status='ok' ORDER BY p.pool_address"""
        )
    ]
    if args.pool:
        pools = [pool for pool in pools if pool in set(args.pool)]
    if args.missing_pools and pools:
        complete = {
            row[0]
            for row in db.execute(
                """SELECT pool_address FROM pool_market_candles
                   WHERE timestamp_utc>=? AND timestamp_utc<?
                   GROUP BY pool_address HAVING count(*)>=?""",
                (start_s_aligned, end_s_aligned, args.hours),
            )
        }
        pools = [pool for pool in pools if pool not in complete]
    latest_option_snapshot = db.execute(
        "SELECT max(snapshot_id) FROM deribit_snapshots"
    ).fetchone()[0]
    instruments = [] if args.only_pools else [
        row[0]
        for row in db.execute(
            "SELECT instrument_name FROM deribit_option_quotes WHERE snapshot_id=? ORDER BY instrument_name",
            (latest_option_snapshot,),
        )
    ]
    if args.instrument:
        instruments = [instrument for instrument in instruments if instrument in set(args.instrument)]
    if (not pools and not args.only_deribit) or (not instruments and not args.only_pools):
        raise SystemExit("expected existing pool and Deribit option snapshots in the database")

    print(json.dumps({"event": "backfill_start", "start_utc": dt.datetime.fromtimestamp(start_ms / 1000, dt.timezone.utc).isoformat(),
                      "end_utc": dt.datetime.fromtimestamp(end_ms / 1000, dt.timezone.utc).isoformat(),
                      "pool_count": len(pools), "instrument_count": len(instruments)}, separators=(",", ":")), flush=True)
    candle_pools, candle_errors = (0, 0)
    trade_instruments, trade_errors = (0, 0)
    if not args.only_deribit:
        candle_pools, candle_errors = backfill_pool_candles(
            db, pools, start_s_aligned, end_s_aligned, args.pool_pause
        )
    if not args.only_pools:
        trade_instruments, trade_errors = backfill_deribit(
            db, instruments, start_ms, end_ms, args.deribit_pause
        )
    summary = {
        "event": "backfill_finish",
        "pool_candles_successful_pools": candle_pools,
        "pool_errors": candle_errors,
        "deribit_instruments_complete": trade_instruments,
        "deribit_errors": trade_errors,
        "database": str(args.db),
    }
    print(json.dumps(summary, separators=(",", ":")), flush=True)
    db.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
