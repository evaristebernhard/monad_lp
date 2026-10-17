#!/usr/bin/env python3
"""Read-only existing LFS Solana event/realtime/market history COVERAGE audit.

Reports table counts, pool-specific row counts and available timestamp spans.
Never prints user wallets, transactions, RPC URLs, trade raw_json, API keys.
Does not claim fee revenue from swap-event volume alone.
"""
import argparse,datetime as dt,json,sqlite3
from pathlib import Path
POOLS={
"meteora_1bp_SOL_USDC":"HTvjzsfX3yU6BUodCjZ5vZkUrAxMDTrBs3CJaq43ashR",
"meteora_4bp_SOL_USDC":"5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6",
"meteora_20bp_SOL_USDC":"BVRbyLjjfSBcoyiYFuxbgKYnWuiFaF9CSXEa5vdSZ9Hh"}
WHITELIST={"solana_pool_trades","solana_pool_events","solana_pool_snapshots",
           "solana_pool_latest","solana_ohlcv_candles","solana_queried_pools",
           "solana_swap_events","solana_raw_events"}
POOL_COLS=("watched_pool","pool_address","pool","pair_address","pool_id","lb_pair","address")
TIME_COLS=("observed_at_ms","timestamp_utc","block_time","block_time_utc","observed_at_utc",
 "timestamp","block_timestamp","recorded_at_utc","recorded_at","created_at_utc",
 "candle_start","candle_timestamp","time_bucket_utc")
def sqlite_db(p):
    p=Path(p)
    if not p.is_file() or p.open("rb").read(16)!=b"SQLite format 3\x00":return None
    db=sqlite3.connect("file:"+str(p.resolve())+"?mode=ro",uri=True)
    db.row_factory=sqlite3.Row;db.execute("PRAGMA query_only=ON")
    return db
def audit(path):
    db=sqlite_db(path)
    if db is None:return {"status":"no_sqlite_object"}
    try:
        found=[x[0] for x in db.execute("SELECT name FROM sqlite_master WHERE type='table'")]
        result={"status":"ok","matching_table_count":len(set(found)&WHITELIST),"tables":{}}
        for tab in sorted(set(found)&WHITELIST):
            # All table identifiers come from hardcoded allowlist, not DB text.
            cols={r[1] for r in db.execute('PRAGMA table_info("'+tab+'")')}
            pc=next((x for x in POOL_COLS if x in cols),None)
            tc=next((x for x in TIME_COLS if x in cols),None)
            rows=db.execute('SELECT count(*) FROM "'+tab+'"').fetchone()[0]
            section={"rows":rows,"has_pool_column":bool(pc),"timestamp_column":tc,"by_target_pool":{}}
            if pc:
                for k,pool in POOLS.items():
                    c=db.execute('SELECT count(*) FROM "'+tab+'" WHERE "'+pc+'"=?',(pool,)).fetchone()[0]
                    item={"rows":c}
                    if tc and c>0:
                        x=db.execute('SELECT min("'+tc+'"),max("'+tc+'") FROM "'+tab+'" WHERE "'+pc+'"=?',(pool,)).fetchone()
                        item["first_time_raw"]=str(x[0])[:36]
                        item["last_time_raw"]=str(x[1])[:36]
                        item["time_data_type"]=type(x[1]).__name__
                    section["by_target_pool"][k]=item
            result["tables"][tab]=section
        return result
    finally:db.close()
def main():
    a=argparse.ArgumentParser()
    a.add_argument("--events",default="rpc/data/solana_events.db")
    a.add_argument("--realtime",default="rpc/data/solana_realtime.db")
    a.add_argument("--market",default="rpc/data/solana_market.db")
    a.add_argument("--out",default="research-artifacts/solana_database_coverage.json")
    args=a.parse_args()
    reports={k:audit(p) for k,p in (("events",args.events),("realtime",args.realtime),
                                   ("market",args.market))}
    result={"collected_at_utc":dt.datetime.now(dt.timezone.utc).isoformat(),
        "source":"existing mounted Git LFS SQLite databases (read-only)",
        "coverage":reports,"limitations":["Pool event history is not a complete on-chain history.",
            "A pool trade record does not attribute trade fees among individual bins.",
            "No wallet addresses, transaction signatures or raw records exported."]}
    p=Path(args.out);p.parent.mkdir(parents=True,exist_ok=True)
    p.write_text(json.dumps(result,indent=2)+"\n")
    print(json.dumps({"status":"ok","tables":{k:{z:{ "rows":v["rows"],
       "target_pool_counts":{name:item["rows"] for name,item in v["by_target_pool"].items()}}
       for z,v in data.get("tables",{}).items()} for k,data in reports.items()}},indent=2))
if __name__=="__main__":main()
