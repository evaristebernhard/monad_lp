#!/usr/bin/env python3
"""Append a public Deribit SOL_USDC option-chain/depth snapshot to the DLMM DB."""
import argparse
import datetime as dt
import importlib
import json
import sqlite3
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RESEARCH_DIR = ROOT / "rpc/option/research"
sys.path.insert(0, str(RESEARCH_DIR))
fetcher = importlib.import_module("fetch_deribit_sol")
depth_fetcher = importlib.import_module("fetch_sol_depth")


def ensure_schema(db):
    db.executescript(
        """
        CREATE TABLE IF NOT EXISTS deribit_snapshots (
            snapshot_id INTEGER PRIMARY KEY,
            fetched_at_utc TEXT NOT NULL,
            source TEXT NOT NULL,
            underlying TEXT NOT NULL,
            currency TEXT NOT NULL,
            spot_usdc_per_sol REAL NOT NULL,
            target_days REAL NOT NULL,
            expiry_timestamp_ms INTEGER NOT NULL,
            expiry_utc TEXT NOT NULL,
            total_expiry_instruments INTEGER NOT NULL,
            selected_count INTEGER NOT NULL,
            two_sided_count INTEGER NOT NULL,
            depth_attempted INTEGER NOT NULL,
            depth_with_asks INTEGER NOT NULL,
            depth_errors INTEGER NOT NULL,
            depth_level_limit INTEGER NOT NULL,
            nearest_pool_snapshot_id INTEGER,
            nearest_pool_delta_seconds REAL,
            status TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_deribit_snapshots_time
            ON deribit_snapshots(fetched_at_utc);
        CREATE TABLE IF NOT EXISTS deribit_option_quotes (
            snapshot_id INTEGER NOT NULL REFERENCES deribit_snapshots(snapshot_id),
            instrument_name TEXT NOT NULL,
            option_type TEXT NOT NULL,
            strike REAL NOT NULL,
            expiry_timestamp_ms INTEGER NOT NULL,
            contract_size_sol REAL,
            min_trade_amount REAL,
            bid_usdc_per_sol REAL,
            ask_usdc_per_sol REAL,
            mark_price_usdc_per_sol REAL,
            mark_iv REAL,
            open_interest REAL,
            volume_usd REAL,
            two_sided INTEGER NOT NULL,
            book_status TEXT NOT NULL,
            book_timestamp_ms INTEGER,
            raw_json TEXT NOT NULL,
            PRIMARY KEY(snapshot_id, instrument_name)
        );
        CREATE INDEX IF NOT EXISTS idx_deribit_quotes_strike
            ON deribit_option_quotes(snapshot_id, option_type, strike);
        CREATE TABLE IF NOT EXISTS deribit_ask_levels (
            snapshot_id INTEGER NOT NULL,
            instrument_name TEXT NOT NULL,
            level INTEGER NOT NULL,
            price_usdc_per_sol REAL NOT NULL,
            amount_sol REAL NOT NULL,
            PRIMARY KEY(snapshot_id, instrument_name, level),
            FOREIGN KEY(snapshot_id, instrument_name)
              REFERENCES deribit_option_quotes(snapshot_id, instrument_name)
        );
        """
    )


def closest_pool_snapshot(db, observed):
    row = db.execute(
        """SELECT snapshot_id,observed_at_utc FROM pool_snapshots
           WHERE status IN ('ok','pool_only') ORDER BY snapshot_id DESC LIMIT 1"""
    ).fetchone()
    if not row:
        return None, None
    try:
        pool_time = dt.datetime.fromisoformat(row[1])
        option_time = dt.datetime.fromisoformat(observed)
        return row[0], abs((option_time - pool_time).total_seconds())
    except (TypeError, ValueError):
        return row[0], None


def fetch_snapshot(days, per_side, risk_low, risk_high, depth, pause):
    # Keep urllib's environment proxy support for public Deribit access.
    opener = urllib.request.build_opener()
    fetcher.urlopen = opener.open
    # Existing helper uses three public endpoints for index, instruments and
    # the complete currency book summary; 250 per side retains the full weekly
    # SOL_USDC expiry on the current listing scale.
    snapshot = fetcher.fetch(days=days, per_side=per_side)
    enriched = depth_fetcher.enrich(
        snapshot,
        lower=risk_low,
        upper=risk_high,
        depth=depth,
        pause=pause,
        request=fetcher.get,
    )
    return enriched


def store(db_path, snapshot):
    db = sqlite3.connect(db_path)
    db.execute("PRAGMA foreign_keys=ON")
    db.execute("PRAGMA journal_mode=WAL")
    ensure_schema(db)
    fetched_at = snapshot["fetched_at"]
    pool_id, pool_delta = closest_pool_snapshot(db, fetched_at)
    transaction = db
    transaction.execute("BEGIN IMMEDIATE")
    cur = transaction.execute(
        """INSERT INTO deribit_snapshots(
           fetched_at_utc,source,underlying,currency,spot_usdc_per_sol,
           target_days,expiry_timestamp_ms,expiry_utc,total_expiry_instruments,
           selected_count,two_sided_count,depth_attempted,depth_with_asks,
           depth_errors,depth_level_limit,nearest_pool_snapshot_id,
           nearest_pool_delta_seconds,status)
           VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,'ok')""",
        (
            fetched_at,
            snapshot["source"],
            "SOL",
            "USDC",
            snapshot["spot_usdc_per_sol"],
            snapshot["target_days"],
            snapshot["expiry_ms"],
            snapshot["expiry_utc"],
            snapshot["total_expiry_instruments"],
            snapshot["selected_count"],
            snapshot["two_sided_count"],
            snapshot["depth_attempted"],
            snapshot["depth_with_asks"],
            snapshot["depth_errors"],
            snapshot["depth_level_limit"],
            pool_id,
            pool_delta,
        ),
    )
    snapshot_id = cur.lastrowid
    for option in snapshot["options"]:
        instrument = option["instrument"]
        transaction.execute(
            """INSERT INTO deribit_option_quotes(
               snapshot_id,instrument_name,option_type,strike,expiry_timestamp_ms,
               contract_size_sol,min_trade_amount,bid_usdc_per_sol,ask_usdc_per_sol,
               mark_price_usdc_per_sol,mark_iv,open_interest,volume_usd,two_sided,
               book_status,book_timestamp_ms,raw_json)
               VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)""",
            (
                snapshot_id,
                instrument,
                option["side"],
                option["strike"],
                option["expiry_ms"],
                option.get("contract_size_sol"),
                option.get("min_trade_amount"),
                option.get("bid_usdc_per_sol"),
                option.get("ask_usdc_per_sol"),
                option.get("mark_price_usdc_per_sol"),
                option.get("mark_iv"),
                option.get("open_interest"),
                option.get("volume_usd"),
                int(bool(option.get("two_sided"))),
                option.get("book_status", "not_requested"),
                option.get("book_timestamp_ms"),
                json.dumps(option, separators=(",", ":")),
            ),
        )
        transaction.executemany(
            """INSERT INTO deribit_ask_levels(
               snapshot_id,instrument_name,level,price_usdc_per_sol,amount_sol)
               VALUES(?,?,?,?,?)""",
            [
                (
                    snapshot_id,
                    instrument,
                    level,
                    ask["price_usdc_per_sol"],
                    ask["amount_sol"],
                )
                for level, ask in enumerate(option.get("book_asks", []), start=1)
            ],
        )
    transaction.commit()
    db.close()
    return snapshot_id, pool_id, pool_delta


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--db", type=Path, default=ROOT / "data/sol_dlmm_deribit.db")
    parser.add_argument("--days", type=float, default=7)
    parser.add_argument("--per-side", type=int, default=250)
    parser.add_argument("--risk-low", type=float, default=0.8)
    parser.add_argument("--risk-high", type=float, default=1.2)
    parser.add_argument("--depth", type=int, default=20)
    parser.add_argument("--pause", type=float, default=0.05)
    args = parser.parse_args()
    if not args.db.exists():
        parser.error(f"pool database does not exist: {args.db}")
    schema_db = sqlite3.connect(args.db)
    ensure_schema(schema_db)
    schema_db.commit()
    schema_db.close()
    snapshot = fetch_snapshot(
        args.days,
        args.per_side,
        args.risk_low,
        args.risk_high,
        args.depth,
        args.pause,
    )
    snapshot_id, pool_id, pool_delta = store(args.db, snapshot)
    print(
        json.dumps(
            {
                "status": snapshot["status"],
                "snapshot_id": snapshot_id,
                "fetched_at_utc": snapshot["fetched_at"],
                "spot_usdc_per_sol": snapshot["spot_usdc_per_sol"],
                "expiry_utc": snapshot["expiry_utc"],
                "selected_options": snapshot["selected_count"],
                "two_sided_quotes": snapshot["two_sided_count"],
                "books_attempted": snapshot["depth_attempted"],
                "books_with_asks": snapshot["depth_with_asks"],
                "book_errors": snapshot["depth_errors"],
                "pool_snapshot_id": pool_id,
                "pool_snapshot_delta_seconds": pool_delta,
                "database": str(args.db),
            },
            separators=(",", ":"),
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
