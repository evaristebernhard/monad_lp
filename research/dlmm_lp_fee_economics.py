#!/usr/bin/env python3
"""Inspect SAVED DLMM pool snapshot and fee-economics (read-only).

Public pool aggregates only. No RPC URLs, wallet addresses, order requests or trades.
"""
import datetime,json,sqlite3,math,urllib.request
from pathlib import Path
POOL="5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6"
SOL="So11111111111111111111111111111111111111112"
DB=Path("data/sol_dlmm_deribit.db")
OUT=Path("research-artifacts/dlmm_lp_fee_economics.json")
def pull_public_market():
    url="https://api.dexscreener.com/latest/dex/pairs/solana/"+POOL
    req=urllib.request.Request(url,headers={"accept":"application/json","user-agent":"arbitage-public-pool-fee-audit/1.0"})
    try:
        with urllib.request.urlopen(req,timeout=20) as resp: payload=json.load(resp)
        p=next(x for x in payload.get("pairs",[]) if x.get("pairAddress")==POOL and x.get("chainId")=="solana")
        return {"status":"ok","sampled_at_utc":datetime.datetime.now(datetime.timezone.utc).isoformat(),
                "tvl_usd":float(p["liquidity"]["usd"]),"volume_24h_usd":float(p["volume"]["h24"]),
                "dex_id":p.get("dexId")}
    except Exception as exc:return {"status":"unavailable","reason_type":type(exc).__name__}
def main():
    if DB.open("rb").read(16)!=b"SQLite format 3\x00":raise ValueError("Database is Git LFS pointer")
    db=sqlite3.connect(f"file:{DB.absolute()}?mode=ro",uri=True)
    db.row_factory=sqlite3.Row
    pool=db.execute("SELECT * FROM pool_snapshots WHERE pool_address=? AND status='ok' ORDER BY snapshot_id DESC LIMIT 1",(POOL,)).fetchone()
    if not pool:raise ValueError("no pool")
    static=json.loads(pool["static_parameters_json"]);var=json.loads(pool["variable_parameters_json"])
    step=float(pool["bin_step_bps"]);power=int(static["base_fee_power_factor"]);factor=int(static["base_factor"])
    fee_fraction=factor*step*10*(10**power)/1e9
    protocol_bps=int(static["protocol_share"])
    lp_share=1-protocol_bps/10000
    spot=float(pool["price_usdc_per_sol"])
    flip=pool["token_y_mint"]==SOL
    dx=int(pool["token_x_decimals"]);dy=int(pool["token_y_decimals"])
    raw=db.execute("SELECT * FROM bin_snapshots WHERE snapshot_id=?",(pool["snapshot_id"],)).fetchall()
    asset_bins=[]
    for b in raw:
        kval=float(b["price_y_per_x"])
        px_pool=float(pool["price_y_per_x"])
        if abs(kval/px_pool-1)<.03 and int(b["bin_id"])==int(pool["active_id"]):correction=1
        elif int(b["bin_id"])==int(pool["active_id"]):
            correction=10**(dx-dy)
        else:continue
    for b in raw:
        k=float(b["price_y_per_x"])/correction
        price=1/k if flip else k
        sol=float(b["amount_y_raw"])/10**dy if flip else float(b["amount_x_raw"])/10**dx
        stable=float(b["amount_x_raw"])/10**dx if flip else float(b["amount_y_raw"])/10**dy
        asset_bins.append((int(b["bin_id"]),price,sol,stable,sol*spot+stable))
    total=sum(x[4] for x in asset_bins)
    active_id=int(pool["active_id"])
    active=next((z for z in asset_bins if z[0]==active_id),None)
    ranges={}
    for band in (0.0025,.01,.03,.05,.1,.2):
        selected=[z for z in asset_bins if abs(z[1]/spot-1)<=band]
        ranges[str(band)]={"bin_count":len(selected),"reserves_mark_usdc":round(sum(x[4] for x in selected),2),
                           "reserve_fraction":round(sum(x[4] for x in selected)/total,6)}
    rows= db.execute("SELECT snapshot_id,fetched_at_utc,expiry_utc,spot_usdc_per_sol FROM deribit_snapshots WHERE status='ok' ORDER BY snapshot_id DESC LIMIT 1").fetchone()
    market=pull_public_market()
    if market["status"]!="ok":
        market={"status":"fallback","sampled_at_utc":"2026-10-09T06:56:00Z",
                "tvl_usd":4110000,"volume_24h_usd":44010000,
                "note":"Previous public pool observation, NOT a new quote"}
    apr_proxy=365*market["volume_24h_usd"]/market["tvl_usd"]*fee_fraction*lp_share
    horizon=(datetime.datetime.fromisoformat(rows["expiry_utc"].replace("Z","+00:00"))-
             datetime.datetime.fromisoformat(rows["fetched_at_utc"].replace("Z","+00:00"))).total_seconds()/86400
    wealth=10000
    fees=wealth*apr_proxy*horizon/365
    inputs=[("10pct",122.99249381295454,29.455533520167393),("20pct",177.31581889689392,15.8299696854545)]
    scenarios={}
    for key,cost,residual in inputs:
        hurdle=cost+residual
        scenarios[key]={"contract_cost_including_fee_usdc":round(cost,2),
                        "worst_static_residual_usdc":round(residual,2),
                        "premium_only_apr_hurdle":cost/wealth*365/horizon,
                        "fee_plus_residual_apr_hurdle":hurdle/wealth*365/horizon,
                        "fee_capture_fraction_needed":hurdle/fees,
                        "excess_if_full_pool_pro_rata_fee_usdc":fees-hurdle,
                        "fee_income_if_capture_25pct_usdc":fees*.25,
                        "excess_if_capture_25pct_usdc":fees*.25-hurdle}
    r={"status":"ok","sampled_at_utc":datetime.datetime.now(datetime.timezone.utc).isoformat(),
       "pool":POOL,"pool_snapshot_id":pool["snapshot_id"],"pool_snapshot_observed":pool["observed_at_utc"],
       "active_id":active_id,"bin_step_bps":step,"raw_static_fee":static,"raw_dynamic_fee_state":var,
       "base_swap_fee_fraction_exact":fee_fraction,"base_swap_fee_bps_exact":fee_fraction*1e4,
       "protocol_share_bps":protocol_bps,"assumed_lp_fee_share_from_config":lp_share,
       "active_bin":{"price_usdc_per_sol":active[1],"sol_amount":active[2],"stable_amount":active[3],
                     "reserves_mark_usdc":active[4]} if active else None,
       "pool_reserves_mark_usdc":total,"num_bins":len(asset_bins),"reserve_distribution":ranges,
       "deribit_snapshot_time":rows["fetched_at_utc"],"option_expiry":rows["expiry_utc"],
       "horizon_days":horizon,"market_source":market,
       "turnover_fee_apr_proxy":apr_proxy,"fee_14d_pro_rata_proxy_usdc":fees,
       "option_hurdles":scenarios,
       "warning":"No user PositionV2, per-bin fee_growth data or future fee guarantee. Displayed base fee can differ from realized dynamic fee; this is not backtested PnL."}
    OUT.parent.mkdir(exist_ok=True,parents=True);OUT.write_text(json.dumps(r,indent=2)+"\n")
    summary={k:r[k] for k in ("pool_snapshot_id","base_swap_fee_bps_exact","protocol_share_bps",
        "assumed_lp_fee_share_from_config","active_bin","pool_reserves_mark_usdc",
        "market_source","turnover_fee_apr_proxy","fee_14d_pro_rata_proxy_usdc","horizon_days","option_hurdles")}
    print(json.dumps(summary,indent=2))
if __name__=="__main__":main()
