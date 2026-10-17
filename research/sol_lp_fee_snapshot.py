#!/usr/bin/env python3
"""Sanitized read-only inventory of Git-LFS RPC / catalog / event DBs.

No endpoint URLs, credentials, wallet IDs, or raw trade JSON leave the runner.
No application databases are written or created.
"""
import argparse,datetime,json,sqlite3
from urllib.parse import urlsplit,parse_qsl
from pathlib import Path

def utc(ms):
    if ms is None:return None
    try:
        v=float(ms)
        if v>1e12:v/=1000
        return datetime.datetime.fromtimestamp(v,datetime.timezone.utc).isoformat()
    except (ValueError,OverflowError,TypeError,OSError):return None

def connect(path):
    if not path.exists():return None,"not_present"
    if path.open("rb").read(16)!=b"SQLite format 3\x00":return None,"not_sqlite_or_lfs_pointer"
    try:
        db=sqlite3.connect("file:"+str(path.resolve())+"?mode=ro",uri=True)
        db.execute("PRAGMA query_only=ON")
        return db,"ok"
    except sqlite3.DatabaseError as e:return None,"cannot_open:"+type(e).__name__

def tables(db):
    return {x[0] for x in db.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")}

def cols(db,table):
    if table not in tables(db):return set()
    return {x[1] for x in db.execute('PRAGMA table_info("'+table+'")')}

def summary(db,name):
    t=tables(db)
    report={"status":"ok","tables":sorted(t)}
    for tb in ("chains","rpc_endpoints","pools","pool_rpc_state","pool_price_history",
               "token_price_history","solana_pool_trades","solana_pool_snapshots"):
        if tb in t:
            try:report[tb+"_rows"]=db.execute('SELECT COUNT(*) FROM "'+tb+'"').fetchone()[0]
            except sqlite3.DatabaseError:report[tb+"_rows"]="query_failed"
    if name=="rpc":
        if "rpc_endpoints" in t:
            # Count credential-shaped URLs; NEVER emit source URLs, key substrings or hashes.
            flagged=0
            for (raw,) in db.execute("SELECT url FROM rpc_endpoints"):
                try:
                    u=urlsplit(raw)
                    host=(u.hostname or "").lower()
                    route=u.path.strip("/")
                    names={k.lower() for k,v in parse_qsl(u.query)}
                    if u.username or (names & {"apikey","api_key","key","token","access_token","auth"}) or (
                        route and any(tag in host for tag in ("alchemy.com","infura.io","quiknode.pro","helius-rpc.com","quicknode.com"))
                    ):
                        flagged+=1
                except Exception:
                    pass
            report["possibly_credential_bearing_endpoint_urls"]=flagged
            report["credential_scan_note"]="Heuristic count only; raw URLs and tokens are never printed."
        if "chains" in t:
            report["chains_enabled"]=db.execute("SELECT COUNT(*) FROM chains WHERE enabled=1").fetchone()[0]
        if "rpc_endpoints" in t:
            # Never log URL, API keys, authorization, or endpoint titles
            report["endpoint_groups"]=[{"provider":p,"tier":tier,"count":n}
                for p,tier,n in db.execute("SELECT provider,tier,COUNT(*) FROM rpc_endpoints GROUP BY provider,tier ORDER BY COUNT(*) DESC LIMIT 25")]
    if name=="catalog" and "pools" in t:
        c=cols(db,"pools")
        report["pool_schema_columns"]=sorted(c)
        report["pool_columns_available"]=sorted(c & {"chain","protocol","pair","pool_address","pool_type","token_a_symbol","token_b_symbol","liquidity_usd","volume_24h_usd","lp_fee_apr_estimate","fee_tier"})
        if {"chain","protocol","pool_type"} <= c:
            report["solana_pool_types"]=[dict(protocol=p,pool_type=f,count=n) for p,f,n in db.execute(
                "SELECT protocol,pool_type,COUNT(*) AS n FROM pools WHERE lower(chain)='solana' GROUP BY protocol,pool_type ORDER BY n DESC LIMIT 25")]
        needs={"chain","token_a_symbol","token_b_symbol","liquidity_usd","volume_24h_usd","protocol","pool_address"}
        if needs<=c:
            fee_expr="COALESCE(lp_fee_apr_estimate,0)" if "lp_fee_apr_estimate" in c else "NULL"
            type_expr="pool_type" if "pool_type" in c else "NULL"
            extra_exprs=[col if col in c else "NULL" for col in ("fee_bps","updated_at","lp_fee_share_assumption","volume_24h_to_tvl")]
            q=f"""SELECT pool_address, protocol, {type_expr},token_a_symbol,token_b_symbol,
                     liquidity_usd,volume_24h_usd,{fee_expr}, {",".join(extra_exprs)}
                     FROM pools WHERE lower(chain)='solana'
                     AND ((upper(token_a_symbol) IN ('SOL','WSOL') AND upper(token_b_symbol) IN ('USDC','USDT','USDS'))
                     OR (upper(token_b_symbol) IN ('SOL','WSOL') AND upper(token_a_symbol) IN ('USDC','USDT','USDS')))
                     AND liquidity_usd>0 ORDER BY COALESCE(volume_24h_usd,0) DESC LIMIT 30"""
            report["top_sol_stable_pairs"]=[
                {"pool":addr,"protocol":p,"family":fam,"pair":f"{a}/{b}","tvl_usd":tv,
                 "volume_24h_usd":v,"catalog_fee_apr_raw":fee,"fee_bps":bps,"pool_updated_at":utc(updated) or updated,
                 "lp_fee_share_assumption":share,"volume_24h_to_tvl":ratio}
                 for addr,p,fam,a,b,tv,v,fee,bps,updated,share,ratio in db.execute(q)]
    if name=="events" and "solana_pool_trades" in t:
        c=cols(db,"solana_pool_trades")
        if {"watched_pool","observed_at_ms","notional_usd"}<=c:
            lo,hi=db.execute("SELECT MIN(observed_at_ms),MAX(observed_at_ms) FROM solana_pool_trades").fetchone()
            report["first_trade_utc"]=utc(lo)
            report["last_trade_utc"]=utc(hi)
            if hi:
                report["last_24h_from_latest_observation"]=[
                 {"pool":p,"protocol":proto,"trade_count":n,"observed_volume_usd":vol}
                 for p,proto,n,vol in db.execute("""SELECT watched_pool,MAX(watched_protocol),COUNT(*),
                   ROUND(SUM(CASE WHEN notional_usd > 0 THEN notional_usd ELSE 0 END),2)
                   FROM solana_pool_trades WHERE observed_at_ms>=? GROUP BY watched_pool
                   ORDER BY 4 DESC LIMIT 25""",(int(hi)-86400000,))]
    if name=="prices":
        for tb in ("pool_price_history","token_price_history"):
            if tb in t:
                c=cols(db,tb)
                report[tb+"_columns"]=sorted(c)
                time_col=next((q for q in ["observed_at","recorded_at","timestamp","timestamp_sec","updated_at"] if q in c),None)
                if time_col:
                    a,b=db.execute(f"SELECT MIN({time_col}),MAX({time_col}) FROM {tb}").fetchone()
                    report[tb+"_start_utc"]=utc(a);report[tb+"_end_utc"]=utc(b)
    return report

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument("--output",required=True,type=Path)
    ap.add_argument("--rpc",default="rpc/data/rpc.db")
    ap.add_argument("--catalog",default="research-data/lp_screener.db")
    ap.add_argument("--prices",default="data/lp_price_history.db")
    ap.add_argument("--events",default="rpc/data/solana_events.db")
    ap.add_argument("--market",default="rpc/data/solana_market.db")
    args=ap.parse_args()
    result={"sampled_at_utc":datetime.datetime.now(datetime.timezone.utc).isoformat(),"sources":{}}
    for name in ("rpc","catalog","prices","events","market"):
        path=Path(getattr(args,name))
        db,status=connect(path)
        if db is None:result["sources"][name]={"status":status};continue
        try:result["sources"][name]=summary(db,name)
        except Exception as e:result["sources"][name]={"status":"query_failed","error_type":type(e).__name__,"query_error":str(e)[:140],"tables":sorted(tables(db)),"pool_schema_columns":sorted(cols(db,"pools")) if "pools" in tables(db) else []}
        finally:db.close()
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(result,ensure_ascii=False,indent=2)+"\n")
    print(json.dumps({"status_by_database":{k:v["status"] for k,v in result["sources"].items()},
          "sol_stable_pairs":len(result["sources"].get("catalog",{}).get("top_sol_stable_pairs",[]))}))
    return 0
if __name__=="__main__":raise SystemExit(main())
