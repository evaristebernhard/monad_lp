#!/usr/bin/env python3
"""Conditional joint DLMM Bin placement / integer Deribit hedge frontier (saved snapshots).
Risk is exact for passive ideal full-conversion Bin basket, not historical or future PnL.
No revenue estimate without per-bin swap volume / fee growth.
"""
import argparse,datetime,sqlite3,math,json,sys
from pathlib import Path
from dlmm_option_window_strategy import pool_from_sql, extract_lots, solve, evaluate

POOL="5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6"
SOL="So11111111111111111111111111111111111111112"
PLANS=[
 ("narrow_10_plus_10",10,10,.5),
 ("centered_35_plus_35",35,35,.5),
 ("wide_125_plus_125",125,125,.5),
 ("puts_only_70",70,0,1.),
 ("calls_only_70",0,70,0.),
 ("asymmetric_10_plus_60",10,60,.35)]
def derive_weights(active,spot,step,wealth,nlower,nupper,below_fraction):
    """Fixed USD weights in each bin; active bin intentionally omitted."""
    if not 0<=below_fraction<=1 or min(nlower,nupper)<0:raise ValueError("invalid allocation")
    if (not nlower and below_fraction) or (not nupper and below_fraction<1):
        raise ValueError("need at least one bin on every funded side")
    out=[]
    for i in range(1,nlower+1):
        k=spot*(1+step/10000)**(-i)
        if below_fraction>0:out.append(("put",k,wealth*below_fraction/nlower/k,active-i))
    for i in range(1,nupper+1):
        k=spot*(1+step/10000)**i
        if below_fraction<1:out.append(("call",k,wealth*(1-below_fraction)/nupper/spot,active+i))
    return out
def pool_bin_competition(db,snapshot,weights,spot,wealth):
    row=db.execute("SELECT token_x_mint,token_y_mint,token_x_decimals,token_y_decimals,price_y_per_x FROM pool_snapshots WHERE snapshot_id=?",(snapshot,)).fetchone()
    flip=row["token_y_mint"]==SOL
    xdec=int(row["token_x_decimals"]);ydec=int(row["token_y_decimals"])
    reserves={}
    for b in db.execute("SELECT bin_id,amount_x_raw,amount_y_raw FROM bin_snapshots WHERE snapshot_id=?",(snapshot,)):
        if flip:
            sol=float(b["amount_y_raw"])/10**ydec;stable=float(b["amount_x_raw"])/10**xdec
        else:
            sol=float(b["amount_x_raw"])/10**xdec;stable=float(b["amount_y_raw"])/10**ydec
        reserves[int(b["bin_id"])]=sol*spot+stable
    total=0.;weighted_own_share=0.;empty=0;maxshare=0.
    for side,k,q,idx in weights:
        funded=q*k if side=="put" else q*spot
        total+=funded
        competing=reserves.get(idx,0.)
        frac=funded/(competing+funded) if competing+funded else 0
        weighted_own_share+=funded*frac
        maxshare=max(maxshare,frac)
        if competing<=0:empty+=1
    return {"actual_snapshot_competing_reserves_usdc":round(sum(reserves.get(z[3],0) for z in weights),2),
            "hypothetical_new_capital_usdc":round(total,2),
            "capital_weighted_local_ownership_after_deposit":round(weighted_own_share/total,4) if total else None,
            "maximum_local_ownership_fraction":round(maxshare,4),
            "selected_empty_bins":empty,
            "note":"Current marked bin reserves only, NOT fee share over future executed swaps."}
def main():
    ap=argparse.ArgumentParser()
    ap.add_argument("--db",type=Path,default=Path("data/sol_dlmm_deribit.db"))
    ap.add_argument("--out",type=Path,default=Path("research-artifacts/dlmm_bin_allocation_frontier.json"))
    ap.add_argument("--self-test",action="store_true")
    a=ap.parse_args()
    if a.self_test:
        w=derive_weights(-20,100,4,10000,35,35,.5)
        assert len(w)==70 and abs(sum(q*k if side=="put" else q*100 for side,k,q,i in w)-10000)<1e-6
        assert abs(evaluate(w,[dict(side=s,strike=k,quantity_sol=q) for s,k,q,i in w],90,110,100)["max_abs_error_usdc"])<1e-7
        try:derive_weights(0,100,4,100,0,10,1)
        except ValueError:pass
        else:raise AssertionError("invalid split accepted")
        print("PASS ideal Bin payoff, USD budget, one-sided validation");return
    if a.db.open("rb").read(16)!=b"SQLite format 3\x00":raise ValueError("needs LFS SQLite")
    db=sqlite3.connect("file:"+str(a.db.resolve())+"?mode=ro",uri=True);db.row_factory=sqlite3.Row
    opt=db.execute("SELECT * FROM deribit_snapshots WHERE status='ok' ORDER BY snapshot_id DESC LIMIT 1").fetchone()
    state=pool_from_sql(db,POOL,opt["fetched_at_utc"])
    spot=state["price"]
    if state["pool_option_time_delta_seconds"]>1800:raise ValueError("inconsistent pool/option times")
    if abs(float(opt["spot_usdc_per_sol"])/spot-1)>.03:raise ValueError("index/pool price mismatch")
    expiry=datetime.datetime.fromisoformat(opt["expiry_utc"].replace("Z","+00:00"))
    t0=datetime.datetime.fromisoformat(opt["fetched_at_utc"].replace("Z","+00:00"))
    days=(expiry-t0).total_seconds()/86400
    out={"source":"Saved Oct 9 LFS Bin state and Deribit ask-depth; theoretical new user Bin allocations",
      "created_at_utc":datetime.datetime.now(datetime.timezone.utc).isoformat(),
      "pool":POOL,"pool_snapshot_id":state["snapshot_id"],"pool_timestamp":state["observed_at_utc"],
      "option_snapshot_id":opt["snapshot_id"],"option_timestamp":opt["fetched_at_utc"],
      "option_expiry_utc":opt["expiry_utc"],"maturity_days":days,
      "reference_spot":spot,"active_id":state["active_id"],"bin_step_bps":state["bin_step_bps"],
      "fee_data_status":"Not enough data to predict realized per-Bin fee income; no profit maximizer claimed.",
      "results":[]}
    lo=spot*.9;hi=spot*1.1
    lots=extract_lots(db,opt["snapshot_id"],spot,lo,hi)
    for wealth,plans in [(10000.,PLANS),(2000.,[PLANS[1]]),(5000.,[PLANS[1]])]:
        for name,nlower,nupper,frac in plans:
            w=derive_weights(state["active_id"],spot,state["bin_step_bps"],wealth,nlower,nupper,frac)
            contest=pool_bin_competition(db,state["snapshot_id"],w,spot,wealth)
            solved=solve(w,lots,lo,hi,spot,lamb=1.,seconds=30)
            entry={"capital_usdc":wealth,"strategy":name,"lower_bins":nlower,"upper_bins":nupper,
               "put_dollar_fraction":frac,"competition":contest,
               "underlying_sol_risk_put":round(sum(q for side,k,q,i in w if side=="put"),3),
               "underlying_sol_risk_call":round(sum(q for side,k,q,i in w if side=="call"),3),
               "solver":solved}
            if solved.get("status")=="risk_certified":
                cost=solved["indicative_cost_incl_fees_usdc"]
                E=solved["hedged"]["max_abs_error_usdc"]
                entry["premium_only_annualized_fee_hurdle"]=cost/wealth*365/days
                entry["premium_plus_worst_residual_annualized_hurdle"]=(cost+E)/wealth*365/days
                entry["relative_risk_reduction"]=1-E/solved["no_hedge"]["max_abs_error_usdc"] if solved["no_hedge"]["max_abs_error_usdc"] else 0
            out["results"].append(entry)
            print(json.dumps({"strategy":name,"capital":wealth,
              "competition":contest,"contracts":solved.get("contract_count"),
              "cost_usd":solved.get("indicative_cost_incl_fees_usdc"),
              "max_error_usd":solved.get("hedged",{}).get("max_abs_error_usdc"),
              "nohedge_usd":solved.get("no_hedge",{}).get("max_abs_error_usdc"),
              "required_fee_apr":entry.get("premium_plus_worst_residual_annualized_hurdle"),
              "solver_status":solved["status"]}))
    a.out.parent.mkdir(parents=True,exist_ok=True);a.out.write_text(json.dumps(out,indent=2)+"\n")
    db.close()
if __name__=="__main__":sys.exit(main() or 0)
