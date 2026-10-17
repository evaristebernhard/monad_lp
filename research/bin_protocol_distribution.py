#!/usr/bin/env python3
"""Read-only snapshot BIN distribution and cross-protocol SOL-stable catalog audit.
No user positions, RPC connection URLs, keys or raw trade logs are emitted.
"""
import argparse, sqlite3, json, math, datetime
from collections import defaultdict
from pathlib import Path

SOL="So11111111111111111111111111111111111111112"
FOCUS="5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6"
BANDS=[.0025,.005,.01,.014,.025,.05,.10,.20]
def open_ro(p):
    p=Path(p)
    if not p.is_file() or p.open("rb").read(16)!=b"SQLite format 3\x00":
        raise RuntimeError("missing actual SQLite LFS bytes: "+p.name)
    db=sqlite3.connect("file:"+str(p.resolve())+"?mode=ro",uri=True)
    db.row_factory=sqlite3.Row; db.execute("PRAGMA query_only=ON")
    return db
def columns(db,table):
    return {r["name"] for r in db.execute('PRAGMA table_info("'+table+'")')}
def norm(v):return float(v) if v is not None else 0.
def summarize_pool(db,row):
    sid=row["snapshot_id"]
    bins=list(db.execute("SELECT * FROM bin_snapshots WHERE snapshot_id=? ORDER BY bin_id",(sid,)))
    if not bins:return None
    step=int(row["bin_step_bps"])
    active=int(row["active_id"]); raw_spot=float(row["price_y_per_x"])
    active_data=next((b for b in bins if int(b["bin_id"])==active),None)
    if active_data is None:return {"pool":row["pool_address"],"status":"active_bin_missing","snapshot_id":sid}
    raw=float(active_data["price_y_per_x"])
    decimals_factor=10**(int(row["token_x_decimals"])-int(row["token_y_decimals"]))
    if abs(raw/raw_spot-1)<.03:fix=1.0
    elif abs(raw/decimals_factor/raw_spot-1)<.03:fix=float(decimals_factor)
    else:return {"pool":row["pool_address"],"status":"price_inconsistent","active_raw_ratio":raw/raw_spot}
    reversed_assets=row["token_y_mint"]==SOL
    if SOL not in [row["token_x_mint"],row["token_y_mint"]]:
        return {"pool":row["pool_address"],"status":"not_sol_pair"}
    spot=float(row["price_usdc_per_sol"])
    xdec=int(row["token_x_decimals"]); ydec=int(row["token_y_decimals"])
    result=[]
    for b in bins:
        k=float(b["price_y_per_x"])/fix
        price=(1/k if reversed_assets else k)
        x=norm(b["amount_y_raw"])/10**ydec if reversed_assets else norm(b["amount_x_raw"])/10**xdec
        y=norm(b["amount_x_raw"])/10**xdec if reversed_assets else norm(b["amount_y_raw"])/10**ydec
        val=x*spot+y
        result.append(dict(id=int(b["bin_id"]),k=price,sol=x,usd=y,value=val,
                           liquidity=int(b["liquidity_supply"])))
    total=sum(z["value"] for z in result)
    active_bin=next(z for z in result if z["id"]==active)
    occupied=[z for z in result if z["value"]>0]
    bands={}
    for bw in BANDS:
        inband=[z for z in result if spot*(1-bw)<=z["k"]<=spot*(1+bw)]
        value=sum(z["value"] for z in inband)
        bands[str(bw)]={"bins":len(inband),"reserves_usdc":round(value,2),
              "share_pool_reserves":round(value/total,5) if total else 0,
              "aggregate_sol":round(sum(z["sol"] for z in inband),5),
              "aggregate_usdc":round(sum(z["usd"] for z in inband),2)}
    left=[z for z in result if z["id"]<active]
    right=[z for z in result if z["id"]>active]
    def side(a):return {"bins":len(a),"reserves_usdc":round(sum(z["value"] for z in a),2),
                        "sol_units":round(sum(z["sol"] for z in a),5),
                        "stable_usdc":round(sum(z["usd"] for z in a),2)}
    # rank by marked *reserves*, which are observable, not by unobserved fees/volume
    highest=sorted(occupied,key=lambda z:z["value"],reverse=True)[:10]
    near=sorted(result,key=lambda z:abs(z["id"]-active))[:15]
    return {"pool":row["pool_address"],"status":"ok","snapshot_id":sid,
      "snapshot_time":row["observed_at_utc"],"active_id":active,
      "spot_usdc_per_sol":spot,"bin_step_bps":step,"price_fix":fix,
      "nonempty_bin_count":len(occupied),"records":len(result),
      "total_marked_reserves_usdc":round(total,2),
      "active_bin":dict(id=active,price=active_bin["k"],sol=active_bin["sol"],
                        stable_usdc=active_bin["usd"],reserves_usdc=round(active_bin["value"],2),
                        reserve_share=round(active_bin["value"]/total,7)),
      "below":side(left),"above":side(right),
      "bin_bands":bands,
      "top_10_bins_by_reserve": [dict(id=z["id"],price=round(z["k"],5),
                             marked_usdc=round(z["value"],2),
                             relative_price_pct=round(100*(z["k"]/spot-1),3)) for z in highest],
      "nearest_15_bin_reserves": [dict(id=z["id"],price=round(z["k"],5),
                               marked_usdc=round(z["value"],2),
                               sol=round(z["sol"],5),stable=round(z["usd"],2)) for z in near],
      "note":"Pool reserves, not user's position shares or fees."}
def market_catalog(db):
    c=columns(db,"pools")
    families=["solana_standard","solana_clmm","solana_dlmm","solana_whirlpool",
              "constant_product_v2","concentrated_v3","uniswap_v4"]
    out={}
    need={"pool_type","chain","token_a_symbol","token_b_symbol","liquidity_usd","volume_24h_usd"}
    if not need<=c:
        return {"status":"unsupported_schema","columns_present":sorted(c & need)}
    for family in families:
        query="""SELECT pool_address,protocol,pair,chain,pool_type,token_a_symbol,token_b_symbol,
                   liquidity_usd,volume_24h_usd,fee_bps,lp_fee_apr_estimate,updated_at
                  FROM pools WHERE pool_type=? AND
                   ((upper(token_a_symbol) IN ('SOL','WSOL','ETH','WETH','BTC','WBTC') AND upper(token_b_symbol) IN ('USDC','USDT'))
                    OR (upper(token_b_symbol) IN ('SOL','WSOL','ETH','WETH','BTC','WBTC') AND upper(token_a_symbol) IN ('USDC','USDT')))
                   AND liquidity_usd>100000 ORDER BY liquidity_usd DESC LIMIT 5"""
        try:rows=db.execute(query,(family,)).fetchall()
        except sqlite3.Error:rows=[]
        count=db.execute("SELECT COUNT(*) FROM pools WHERE pool_type=?",(family,)).fetchone()[0]
        out[family]={"catalog_count":count,"top_liquid_hedgeable_pairs":[dict(
                pool=r["pool_address"],pair=r["pair"],protocol=r["protocol"],chain=r["chain"],
                catalog_tvl_usd=r["liquidity_usd"],catalog_volume_24h_usd=r["volume_24h_usd"],
                fee_bps=r["fee_bps"],catalog_apr_raw=r["lp_fee_apr_estimate"],
                updated_at=r["updated_at"]) for r in rows]}
    return out
def main():
    p=argparse.ArgumentParser()
    p.add_argument("--db",default="data/sol_dlmm_deribit.db")
    p.add_argument("--catalog",default="data/lp_screener.db")
    p.add_argument("--out",default="research-artifacts/bin_protocol_distribution.json")
    a=p.parse_args()
    db=open_ro(a.db);cat=open_ro(a.catalog)
    rows=db.execute("""SELECT * FROM pool_snapshots WHERE status='ok' AND active_id IS NOT NULL
                 ORDER BY snapshot_id DESC""").fetchall()
    latest={}
    for row in rows:latest.setdefault(row["pool_address"],row)
    summaries=[summarize_pool(db,row) for row in latest.values()]
    summaries=sorted([x for x in summaries if x],key=lambda x:(x["status"]!="ok",-x.get("total_marked_reserves_usdc",0)))
    focus=next((x for x in summaries if x.get("pool")==FOCUS),None)
    if not focus or focus["status"]!="ok":raise RuntimeError("focus pool invalid/missing")
    output={"created_at_utc":datetime.datetime.now(datetime.timezone.utc).isoformat(),
     "scope":"Saved latest per-pool Git LFS Bin snapshots, not fresh blockchain read; no user positions.",
     "pool_snapshot_count":len(rows),"distinct_pools":len(latest),
     "focus":focus,"dlmm_all_pools":[{k:v for k,v in x.items() if k not in
          ("top_10_bins_by_reserve","nearest_15_bin_reserves")} for x in summaries],
     "catalog_protocol_comparison":market_catalog(cat),
     "limits":["Pool bin reserves are not owned PositionV2 shares.",
               "Pool-level trade volume does not give per-bin fee-growth.",
               "Catalog h24 volume and APR may be stale and may not share snapshot timestamps."]}
    Path(a.out).parent.mkdir(parents=True,exist_ok=True)
    Path(a.out).write_text(json.dumps(output,indent=2)+"\n")
    print(json.dumps({"distinct_pools":len(latest),"focus":{
        "snapshot":focus["snapshot_id"],"time":focus["snapshot_time"],
        "total_reserves":focus["total_marked_reserves_usdc"],
        "active_bin":focus["active_bin"],"below":focus["below"],"above":focus["above"],
        "bin_bands":focus["bin_bands"],"largest_bins":focus["top_10_bins_by_reserve"][:5]},
        "protocol_count":{k:v["catalog_count"] for k,v in output["catalog_protocol_comparison"].items()},
        "other_pools":[{"pool":x.get("pool"),"reserves":x.get("total_marked_reserves_usdc"),
                       "step":x.get("bin_step_bps"),"active":x.get("active_bin",{}).get("reserve_share")} for x in summaries[:8]]}))
    db.close();cat.close()
if __name__=="__main__":main()
