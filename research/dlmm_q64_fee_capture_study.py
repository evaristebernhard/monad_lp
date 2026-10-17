#!/usr/bin/env python3
"""Observed Meteora Bin fee growth -> *conditional* fee-capture and Gamma cost.

Inputs are two PUBLIC GitHub Actions JSON snapshots of same SOL/USDC pool.
No private RPC DB, credentials, positions, orders or expected-return claims.

Meteora Q64 fee accounting: raw_fee = ((liquidity_shares >> 64)
                                        * delta_fee_growth_per_token) >> 64.
For complete-bin fee illustrations, use start/end liquidity supply as competing
SENSITIVITY proxies. They are NOT mathematical bounds if liquidity changed
between snapshots. A hypothetical deposit is NOT included in actual fee growth.
"""
import argparse,datetime as dt,json,math
from pathlib import Path
from fee_gamma_lvr_theory import allocate_fee_budget,bs_option,dlmm_loss,dlmm_bins
Q64=1<<64
WSOL="So11111111111111111111111111111111111111112"
USDC="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
SIGMA_ASSUMPTION=.538

def parse_time(s):
    return dt.datetime.fromisoformat(s.replace("Z","+00:00"))
def exact_new_fee_raw(share_raw,delta_growth_raw):
    """Same operation order as Meteora fee accrual; round down twice."""
    s=int(share_raw);g=int(delta_growth_raw)
    if s<0 or g<0:raise ValueError("negative on-chain fee accumulator")
    return ((s>>64)*g)>>64
def spot_from_active(pool):
    if {pool.get("token_x_mint"),pool.get("token_y_mint")}!={WSOL,USDC}:
        raise ValueError("token mints not canonical SOL/USDC")
    if pool["token_x_mint"]!=WSOL or pool["token_y_mint"]!=USDC:
        raise ValueError("this calculator requires X=SOL Y=USDC")
    step=int(pool["bin_step_bps"])
    return math.exp(int(pool["active_bin_id"])*math.log1p(step/10000))*1000.
def reserves_usdc(binrow,spot):
    return int(binrow["amount_x_raw"])/1e9*spot+int(binrow["amount_y_raw"])/1e6
def divergence_at_end(wealth,first,last,lower=35,upper=35):
    s0=spot_from_active(first);s1=spot_from_active(last)
    basket=dlmm_bins(wealth,s0,int(first["bin_step_bps"]),lower,upper)
    return dlmm_loss(basket,s1)
def per_pool(before,after,name,capital=10000.0,bins_each_side=35,sigma=SIGMA_ASSUMPTION):
    a,b=before["pools"][name],after["pools"][name]
    if a.get("status")!="ok" or b.get("status")!="ok":return {"status":"unavailable"}
    if a["pool"]!=b["pool"] or int(a["bin_step_bps"])!=int(b["bin_step_bps"]):
        raise ValueError("pool identity/step changed")
    if int(b["slot"])<=int(a["slot"]):raise ValueError("non-forward Solana slots")
    duration=(parse_time(b["observed_utc"])-parse_time(a["observed_utc"])).total_seconds()
    if duration<=0:raise ValueError("non-forward observation UTC")
    s0=spot_from_active(a);s1=spot_from_active(b)
    b0=a["bin_fee_growth_raw"];b1=b["bin_fee_growth_raw"]
    active0=int(a["active_bin_id"])
    lo=active0-bins_each_side;hi=active0+bins_each_side
    years=duration/(365*86400)
    records=[]
    missing=0;resets=0
    for key,row0 in b0.items():
        row1=b1.get(key)
        if row1 is None:missing+=1;continue
        bini=int(key)
        dfx=int(row1["fee_growth_x_raw"])-int(row0["fee_growth_x_raw"])
        dfy=int(row1["fee_growth_y_raw"])-int(row0["fee_growth_y_raw"])
        if min(dfx,dfy)<0:resets+=1;continue
        L0=int(row0["liquidity_supply"]);L1=int(row1["liquidity_supply"])
        C0=reserves_usdc(row0,s0)
        C1=reserves_usdc(row1,s1)
        fx0=exact_new_fee_raw(L0,dfx);fx1=exact_new_fee_raw(L1,dfx)
        fy0=exact_new_fee_raw(L0,dfy);fy1=exact_new_fee_raw(L1,dfy)
        usd0=fx0/1e9*s0+fy0/1e6
        usd1=fx1/1e9*s0+fy1/1e6
        side="put" if bini<active0 else ("call" if bini>active0 else "active")
        k=s0*math.exp((bini-active0)*math.log1p(a["bin_step_bps"]/10000))
        if side=="put":cost_per_usd=bs_option(s0,k,sigma,years,"put")/k
        elif side=="call":cost_per_usd=bs_option(s0,k,sigma,years,"call")/s0
        else:cost_per_usd=None
        records.append(dict(id=bini,side=side,initial_reserves_usdc=C0,
           final_reserves_usdc=C1,initial_supply_raw=str(L0),
           final_supply_raw=str(L1),initial_supply_is_zero=L0==0,
           supply_changed=L0!=L1,delta_fee_x_raw=str(dfx),
           delta_fee_y_raw=str(dfy),
           inferred_pool_fee_usdc_from_start_supply=usd0,
           inferred_pool_fee_usdc_from_end_supply=usd1,
           midpoint_fee_usdc=(usd0+usd1)/2,
           riskneutral_short_gamma_cost_per_dollar=cost_per_usd,
           in_initial_70_funded_bins=(lo<=bini<=hi and bini!=active0)))
    share_rows=[r for r in records if r["in_initial_70_funded_bins"]]
    # At t0, unlike an actual PositionV2, no new user funds were deposited.
    # Therefore this is a retrospective, market-impact-free approximation ONLY.
    selected=[]
    equal_dollar=capital/(2*bins_each_side)
    for r in share_rows:
        old=r["initial_reserves_usdc"]
        if old<=0:
            if r["midpoint_fee_usdc"]>0:
                raise ValueError("positive fee growth with empty marked reserves")
            continue
        fraction=equal_dollar/(old+equal_dollar)
        selected.append((r,fraction))
    fee_scenario=sum(r["midpoint_fee_usdc"]*f for r,f in selected)
    low_scenario=sum(min(r["inferred_pool_fee_usdc_from_start_supply"],
                         r["inferred_pool_fee_usdc_from_end_supply"])*f for r,f in selected)
    high_scenario=sum(max(r["inferred_pool_fee_usdc_from_start_supply"],
                          r["inferred_pool_fee_usdc_from_end_supply"])*f for r,f in selected)
    D_end=divergence_at_end(capital,a,b,bins_each_side,bins_each_side)
    # KKT is a retrospective ILLUSTRATION: expected Bin fees are unknown.
    # Choosing w_i after observing ΔG is hindsight (and changes fee market).
    valid=[r for r in share_rows if r["side"] in ("put","call")
           and r["initial_reserves_usdc"]>0
           and r["riskneutral_short_gamma_cost_per_dollar"] is not None]
    A=[r["midpoint_fee_usdc"] for r in valid]
    C=[r["initial_reserves_usdc"] for r in valid]
    ell=[max(r["riskneutral_short_gamma_cost_per_dollar"],1e-12) for r in valid]
    example=allocate_fee_budget(A,C,ell,capital) if valid else None
    ranked=[dict(id=r["id"],observed_fee_pool_usdc=round(r["midpoint_fee_usdc"],7),
         initial_incumbent_usdc=round(r["initial_reserves_usdc"],2),
         fee_per_initial_capital=r["midpoint_fee_usdc"]/r["initial_reserves_usdc"],
         supply_changed=r["supply_changed"]) for r in records
        if r["midpoint_fee_usdc"]>0 and r["initial_reserves_usdc"]>0]
    ranked.sort(key=lambda r:r["observed_fee_pool_usdc"],reverse=True)
    rv=[r for r in records if r["midpoint_fee_usdc"]>0]
    stable=[r for r in records if not r["supply_changed"]]
    return {"status":"measured_counter_interval",
      "pool":a["pool"],"base_fee_bps":a["base_fee_bps"],"bin_step_bps":a["bin_step_bps"],
      "start_slot":a["slot"],"end_slot":b["slot"],"start_utc":a["observed_utc"],
      "end_utc":b["observed_utc"],"seconds":duration,
      "start_active_bin":active0,"end_active_bin":b["active_bin_id"],
      "start_sol_bin_price_proxy":s0,"end_sol_bin_price_proxy":s1,
      "token_x":a["token_x_mint"],"token_y":a["token_y_mint"],
      "initial_observed_bin_ids":len(b0),"common_bins":len(records),
      "initial_bins_not_shared":missing,"negative_counter_bins":resets,
      "bins_with_positive_fee_growth":len(rv),
      "bins_with_changed_supply":sum(r["supply_changed"] for r in records),
      "endpoints_supply_proxy_all_bins_usdc":{
       "using_start_supply":sum(r["inferred_pool_fee_usdc_from_start_supply"] for r in records),
       "using_end_supply":sum(r["inferred_pool_fee_usdc_from_end_supply"] for r in records),
       "midpoint":sum(r["midpoint_fee_usdc"] for r in records)},
      "unchanged_endpoint_supply_bins":{
       "bin_count":len(stable),
       "fee_usdc_proxy":sum(r["midpoint_fee_usdc"] for r in stable)},
      "hypothetical_70_bin_equal_dollar_deposit":{
        "capital_usdc":capital,"funded_bins":bins_each_side*2,
        "observed_bins_in_range":len(share_rows),"bins_with_nonempty_start_reserves":len(selected),
        "fee_midpoint_counterfactual_usdc":fee_scenario,
        "fee_endpoint_supply_sensitivity_usdc":[low_scenario,high_scenario],
        "terminal_fee_free_divergence_vs_HODL_usdc":D_end,
        "conditional_net_fee_minus_divergence_usdc":fee_scenario-D_end,
        "caveat":"Post-facto hypothetical share; DOES NOT REPRESENT a real wallet, actual fee APY or a replayable position. Adding funds would dilute fee growth and could change swaps."},
      "postfacto_KKT":{
        "status":"hindsight_not_tradeable",
        "only_funded_bin_candidates":len(valid),
        "assumed_sigma_ann":sigma,"risk_horizon_same_as_counter_interval_seconds":duration,
        "capital_not_required_to_be_fully_allocated":True,
        "nonzero_bins":sum(w>1e-8 for w in example["allocations_usdc"]) if example else 0,
        "idle_cash_usdc":example["idle_cash_usdc"] if example else capital,
        "allocations":sorted([dict(bin_id=r["id"],usdc=round(w,4)) for r,w in zip(valid,example["allocations_usdc"])
          if w>=0.01],key=lambda z:z["usdc"],reverse=True)[:12] if example else [],
        "warning":"This uses realized fees AFTER the interval as inputs; look-ahead bias. Not a forward strategy."},
      "largest_bins_by_new_fee":ranked[:10],
      "warnings":["Q64 full-bin gross LP fee proxy, with start/end liquidity supply scenario; not rigorous bounds between observations.",
       "Value token X at initial active Bin price proxy; exact spot/transfer-fee dynamics not observed.",
       "Only five BinArrays near first snapshot active ID; swaps outside are not measured.",
       "Fees are actual public counter growth, not individual PositionV2 owner earnings.",
       "The reserve-fraction LP fee capture is a counterfactual market-impact-free assumption.",
       "Risk-neutral Gamma cost is not an expected realized short-window price loss and not directly interchangeable with physical fees.",
       "No annualized or fourteen-day APR is extrapolated from this short interval."]}
def main():
    p=argparse.ArgumentParser()
    p.add_argument("--before",type=Path)
    p.add_argument("--after",type=Path)
    p.add_argument("--out",type=Path,default=Path("research-artifacts/dlmm_bin_fee_capture_study.json"))
    p.add_argument("--self-test",action="store_true")
    args=p.parse_args()
    if args.self_test:
        assert exact_new_fee_raw(1<<64,1<<64)==1
        assert exact_new_fee_raw(3<<64,1<<64)==3
        assert exact_new_fee_raw(3<<64,(1<<64)//2)==1
        assert exact_new_fee_raw((3<<64)-1,1<<64)==2
        try:exact_new_fee_raw(1,-1)
        except ValueError:pass
        else:raise AssertionError("negative counter accepted")
        spot0=110.25;active=-5483
        pool=dict(token_x_mint=WSOL,token_y_mint=USDC,bin_step_bps=4,active_bin_id=active)
        assert 50<spot_from_active(pool)<200
        assert dlmm_loss(dlmm_bins(10000,spot0,4),spot0)==0
        print("PASS: Meteora exact double-shift Q64 accrual, integer rounding, token/price controls")
        return
    if not (args.before and args.after):raise SystemExit("provide two real saved Actions JSON")
    before=json.loads(args.before.read_text());after=json.loads(args.after.read_text())
    if before.get("schema_version")!=1 or after.get("schema_version")!=1:
        raise ValueError("untrusted fixture schema")
    out={"status":"ok","created_utc":dt.datetime.now(dt.timezone.utc).isoformat(),
         "provenance":{"before_run_artifact":str(args.before),"after_run_artifact":str(args.after)},
         "initial_capital_usdc":10000,"sigma_annual_assumption":SIGMA_ASSUMPTION,
         "intervals":[]}
    for name in ("meteora_1bp_SOL_USDC","meteora_4bp_SOL_USDC","meteora_20bp_SOL_USDC"):
        interval=per_pool(before,after,name)
        out["intervals"].append(interval)
        print(json.dumps({"pool":name,"seconds":interval["seconds"],
          "bin_activity":interval["bins_with_positive_fee_growth"],
          "whole_pool_subset_gross_fee_usdc":round(interval["endpoints_supply_proxy_all_bins_usdc"]["midpoint"],4),
          "hypothetical_user_fee_usdc":round(interval["hypothetical_70_bin_equal_dollar_deposit"]["fee_midpoint_counterfactual_usdc"],5),
          "hypothetical_divergence_usdc":round(interval["hypothetical_70_bin_equal_dollar_deposit"]["terminal_fee_free_divergence_vs_HODL_usdc"],5),
          "hindsight_KKT_bins":interval["postfacto_KKT"]["nonzero_bins"],
          "changed_supply_bins":interval["bins_with_changed_supply"]}),flush=True)
    args.out.parent.mkdir(parents=True,exist_ok=True)
    args.out.write_text(json.dumps(out,indent=2)+"\n")
if __name__=="__main__":main()
