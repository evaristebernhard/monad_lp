#!/usr/bin/env python3
"""Collect SOL/stable Meteora DLMM pool and bin snapshots into a dedicated DB."""
import argparse
import base64
import datetime as dt
import importlib.util
import json
import sqlite3
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROBE_PATH = Path(__file__).with_name("meteora-dlmm-live-probe.py")
SPEC = importlib.util.spec_from_file_location("meteora_probe", PROBE_PATH)
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)

SOL = "So11111111111111111111111111111111111111112"
STABLES = {"USDC", "USDT", "USDS"}


def utc_now():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def open_db(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(path)
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("PRAGMA synchronous=NORMAL")
    db.executescript(
        """
        CREATE TABLE IF NOT EXISTS pools (
            pool_address TEXT PRIMARY KEY,
            pair TEXT NOT NULL,
            token_x_mint TEXT,
            token_y_mint TEXT,
            token_x_decimals INTEGER,
            token_y_decimals INTEGER,
            catalog_tvl_usd REAL,
            catalog_volume_24h_usd REAL,
            catalog_updated_at TEXT,
            first_seen_utc TEXT NOT NULL,
            last_seen_utc TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS pool_snapshots (
            snapshot_id INTEGER PRIMARY KEY,
            pool_address TEXT NOT NULL,
            observed_at_utc TEXT NOT NULL,
            status TEXT NOT NULL,
            active_id INTEGER,
            bin_step_bps INTEGER,
            price_y_per_x REAL,
            price_usdc_per_sol REAL,
            token_x_mint TEXT,
            token_y_mint TEXT,
            token_x_decimals INTEGER,
            token_y_decimals INTEGER,
            static_parameters_json TEXT,
            variable_parameters_json TEXT,
            bin_array_count INTEGER,
            populated_bin_count INTEGER,
            error_stage TEXT,
            error_type TEXT,
            FOREIGN KEY(pool_address) REFERENCES pools(pool_address)
        );
        CREATE INDEX IF NOT EXISTS idx_pool_snapshots_pool_time
            ON pool_snapshots(pool_address, observed_at_utc);
        CREATE TABLE IF NOT EXISTS bin_snapshots (
            snapshot_id INTEGER NOT NULL,
            bin_id INTEGER NOT NULL,
            price_y_per_x REAL NOT NULL,
            amount_x_raw INTEGER NOT NULL,
            amount_y_raw INTEGER NOT NULL,
            liquidity_supply TEXT NOT NULL,
            PRIMARY KEY(snapshot_id, bin_id),
            FOREIGN KEY(snapshot_id) REFERENCES pool_snapshots(snapshot_id)
        );
        """
    )
    columns = {row[1] for row in db.execute("PRAGMA table_info(pool_snapshots)")}
    if "error_stage" not in columns:
        db.execute("ALTER TABLE pool_snapshots ADD COLUMN error_stage TEXT")
    return db


def candidates(path, limit):
    db = sqlite3.connect(f"file:{path.resolve()}?mode=ro", uri=True)
    db.row_factory = sqlite3.Row
    rows = db.execute(
        """
        SELECT pool_address, pair, token_a_symbol, token_b_symbol,
               token_a_address, token_b_address, liquidity_usd,
               volume_24h_usd, updated_at
        FROM pools
        WHERE lower(chain)='solana'
          AND lower(coalesce(pool_type,'')) LIKE '%dlmm%'
          AND ((upper(token_a_symbol) IN ('SOL','WSOL')
                AND upper(token_b_symbol) IN ('USDC','USDT','USDS'))
            OR (upper(token_b_symbol) IN ('SOL','WSOL')
                AND upper(token_a_symbol) IN ('USDC','USDT','USDS')))
        ORDER BY coalesce(volume_24h_usd,0) DESC,
                 coalesce(liquidity_usd,0) DESC
        LIMIT ?
        """,
        (limit,),
    ).fetchall()
    db.close()
    return rows


def read_pool_state(url, pool_address):
    owner, raw = PROBE.account(url, pool_address)
    if owner != PROBE.PROGRAM:
        raise ValueError("not_meteora_dlmm_pair")
    state = PROBE.parse_lb_pair(raw)
    _, mint_x_data = PROBE.account(url, state["token_x_mint"])
    _, mint_y_data = PROBE.account(url, state["token_y_mint"])
    state["token_x_decimals"] = PROBE.parse_mint(mint_x_data)["decimals"]
    state["token_y_decimals"] = PROBE.parse_mint(mint_y_data)["decimals"]
    return state


def read_bins(url, pool_address, state):
    filters = [
        {"dataSize": PROBE.BIN_ARRAY_SIZE},
        {"memcmp": {"offset": 24, "bytes": pool_address}},
    ]
    found = PROBE.request(
        url,
        "getProgramAccounts",
        [PROBE.PROGRAM, {"encoding": "base64", "commitment": "confirmed", "filters": filters}],
    )
    arrays = [
        PROBE.parse_bin_array(
            base64.b64decode(item["account"]["data"][0]),
            pool_address,
            state["token_x_decimals"],
            state["token_y_decimals"],
            state["bin_step_bps"],
        )
        for item in found
    ]
    bins = [entry for array in arrays for entry in array["bins"]]
    return len(arrays), bins


def record_success(db, row, state, array_count, bins, observed,
                   status="ok", error_stage=None, error_type=None):
    pool = row["pool_address"]
    pair = row["pair"]
    db.execute(
        """INSERT INTO pools VALUES(?,?,?,?,?,?,?,?,?,?,?)
           ON CONFLICT(pool_address) DO UPDATE SET pair=excluded.pair,
             token_x_mint=excluded.token_x_mint,token_y_mint=excluded.token_y_mint,
             token_x_decimals=excluded.token_x_decimals,
             token_y_decimals=excluded.token_y_decimals,
             catalog_tvl_usd=excluded.catalog_tvl_usd,
             catalog_volume_24h_usd=excluded.catalog_volume_24h_usd,
             catalog_updated_at=excluded.catalog_updated_at,
             last_seen_utc=excluded.last_seen_utc""",
        (
            pool,
            pair,
            state["token_x_mint"],
            state["token_y_mint"],
            state["token_x_decimals"],
            state["token_y_decimals"],
            row["liquidity_usd"],
            row["volume_24h_usd"],
            str(row["updated_at"]),
            observed,
            observed,
        ),
    )
    human_price = state["raw_price_y_per_x"] * 10 ** (
        state["token_x_decimals"] - state["token_y_decimals"]
    )
    price_usdc_per_sol = None
    if state["token_x_mint"] == SOL and row["token_b_symbol"].upper() in STABLES:
        price_usdc_per_sol = human_price
    elif state["token_y_mint"] == SOL and row["token_a_symbol"].upper() in STABLES:
        price_usdc_per_sol = 1 / human_price if human_price else None
    cursor = db.execute(
        """INSERT INTO pool_snapshots(
           pool_address,observed_at_utc,status,active_id,bin_step_bps,
           price_y_per_x,price_usdc_per_sol,token_x_mint,token_y_mint,
           token_x_decimals,token_y_decimals,static_parameters_json,
           variable_parameters_json,bin_array_count,populated_bin_count,
           error_stage,error_type)
           VALUES(?,?,?, ?,?,?,?,?,?,?,?,?,?,?,?,?,?)""",
        (
            pool,
            observed,
            status,
            state["active_id"],
            state["bin_step_bps"],
            human_price,
            price_usdc_per_sol,
            state["token_x_mint"],
            state["token_y_mint"],
            state["token_x_decimals"],
            state["token_y_decimals"],
            json.dumps(state["static_parameters"], separators=(",", ":")),
            json.dumps(state["variable_parameters"], separators=(",", ":")),
            array_count,
            len(bins),
            error_stage,
            error_type,
        ),
    )
    snapshot_id = cursor.lastrowid
    db.executemany(
        "INSERT INTO bin_snapshots VALUES(?,?,?,?,?,?)",
        [
            (
                snapshot_id,
                item["bin_id"],
                item["price_y_per_x"],  # parse_bin_array already normalized token decimals
                item["amount_x_raw"],
                item["amount_y_raw"],
                item["liquidity_supply"],
            )
            for item in bins
        ],
    )
    return snapshot_id, human_price, price_usdc_per_sol


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--catalog", type=Path, default=ROOT / "data/lp_screener.db")
    parser.add_argument("--db", type=Path, default=ROOT / "data/sol_dlmm_deribit.db")
    parser.add_argument("--local-rpc", default="http://127.0.0.1:8545/rpc/solana-mainnet")
    parser.add_argument("--limit", type=int, default=40)
    args = parser.parse_args()
    source_rows = candidates(args.catalog, args.limit)
    if not source_rows:
        print(json.dumps({"status": "no_candidates", "db": str(args.db)}))
        return 1

    # RPC URL is local and explicit. Ignore shell proxy environment for loopback.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    PROBE.urllib.request.urlopen = opener.open
    db = open_db(args.db)
    observed = utc_now()
    summary = []
    for row in source_rows:
        stage = "pool_account"
        state = None
        try:
            state = read_pool_state(args.local_rpc, row["pool_address"])
            stage = "bin_arrays"
            array_count, bins = read_bins(args.local_rpc, row["pool_address"], state)
            snapshot_id, human_price, price_usd = record_success(
                db, row, state, array_count, bins, observed
            )
            db.commit()
            summary.append(
                {
                    "pool": row["pool_address"],
                    "pair": row["pair"],
                    "snapshot_id": snapshot_id,
                    "active_id": state["active_id"],
                    "bin_step_bps": state["bin_step_bps"],
                    "price_y_per_x": human_price,
                    "price_usdc_per_sol": price_usd,
                    "bin_arrays": array_count,
                    "populated_bins": len(bins),
                    "status": "ok",
                }
            )
        except Exception as exc:
            db.rollback()
            if stage == "bin_arrays" and state is not None:
                snapshot_id, _, price_usd = record_success(
                    db, row, state, 0, [], observed,
                    status="pool_only", error_stage=stage,
                    error_type=type(exc).__name__,
                )
                db.commit()
                summary.append(
                    {
                        "pool": row["pool_address"],
                        "pair": row["pair"],
                        "snapshot_id": snapshot_id,
                        "active_id": state["active_id"],
                        "bin_step_bps": state["bin_step_bps"],
                        "price_usdc_per_sol": price_usd,
                        "status": "pool_only",
                        "error_stage": stage,
                        "error_type": type(exc).__name__,
                    }
                )
                print(json.dumps(summary[-1], separators=(",", ":")), flush=True)
                continue
            # A failure snapshot references the registry row, so ensure it is
            # present before inserting the observation.
            db.execute(
                """INSERT OR IGNORE INTO pools(
                   pool_address,pair,first_seen_utc,last_seen_utc)
                   VALUES(?,?,?,?)""",
                (row["pool_address"], row["pair"], observed, observed),
            )
            db.execute(
                """INSERT INTO pool_snapshots(
                   pool_address,observed_at_utc,status,error_stage,error_type)
                   VALUES(?,?,?,?,?)""",
                (row["pool_address"], observed, "unavailable", stage, type(exc).__name__),
            )
            db.commit()
            summary.append(
                {
                    "pool": row["pool_address"],
                    "pair": row["pair"],
                    "status": "unavailable",
                    "error_stage": stage,
                    "error_type": type(exc).__name__,
                }
            )
        print(json.dumps(summary[-1], separators=(",", ":")), flush=True)

    db.close()
    ok = sum(item["status"] in ("ok", "pool_only") for item in summary)
    print(
        json.dumps(
            {
                "status": "complete" if ok else "no_live_data",
                "observed_at_utc": observed,
                "candidate_count": len(source_rows),
                "successful_pools": ok,
                "pools_with_bins": sum(item["status"] == "ok" for item in summary),
                "failed_pools": len(summary) - ok,
                "database": str(args.db),
            },
            separators=(",", ":"),
        )
    )
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
