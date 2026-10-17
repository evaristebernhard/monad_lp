#!/usr/bin/env python3
"""Time-integrated shadow fee ledger for ONE fixed DLMM Bin cohort.

Same 70 Bin IDs funded at first slot; no redistributing, no looking ahead.
Uses Q64 actual observed fee-growth differences and endpoint-average *existing*
liquidity to approximate the dilution a new fixed liquidity-share q would
induce. Real deposits change swap routing, so no realized or guaranteed profit.
"""
import argparse,datetime as dt,json,math
from pathlib import Path
from dlmm_q64_fee_capture_study import (
    exact_new_fee_raw,spot_from_active,reserves_usdc,parse_time)
from fee_gamma_lvr_theory import dlmm_bins,dlmm_loss
NAMES=("meteora_1bp_SOL_USDC","meteora_4bp_SOL_USDC","meteora_20bp_SOL_USDC")
def fee_ledger(samples,name,capital):
    first=samples[0]["pools"][name];last=samples[-1]["pools"][name]
    spot0=spot_from_active(first);spot1=spot_from_active(last)
    aid=int(first["active_bin_id"]);step=int(first["bin_step_bps"])
    included_ids=list(range(aid-35,aid))+list(range(aid+1,aid+36))
    init=first["bin_fee_growth_raw"]
    funds=capital/70
    q_by_id={}
    for bin_id in included_ids:
        record=init.get(str(bin_id))
        if record is None:raise ValueError("initial bin not measured")
        C=reserves_usdc(record,spot0)
        L=int(record["liquidity_supply"])
        if not(C>0 and L>0):raise ValueError("cannot mint hypothetical liquidity into empty/zero-supply bin")
        q_by_id[bin_id]=int(L*(funds/C))
    flows=[]
    for before,after in zip(samples,samples[1:]):
        a=before["pools"][name];b=after["pools"][name]
        A=a["bin_fee_growth_raw"];B=b["bin_fee_growth_raw"]
        if int(b["slot"])<=int(a["slot"]):raise ValueError("non-forward pool slot")
        sol=spot_from_active(a)
        fee=0.;gross_x=0.;gross_y=0.;changed=0;missing=0;contributing=0
        for bid,q in q_by_id.items():
            x=A.get(str(bid));y=B.get(str(bid))
            if not(x and y):
                missing+=1;continue
            dfx=int(y["fee_growth_x_raw"])-int(x["fee_growth_x_raw"])
            dfy=int(y["fee_growth_y_raw"])-int(x["fee_growth_y_raw"])
            if min(dfx,dfy)<0:raise ValueError("counter reset unsupported")
            L0=int(x["liquidity_supply"]);L1=int(y["liquidity_supply"])
            if L0!=L1:changed+=1
            # Scenario: preserve observed old LP fee volume and include hypothetical
            # q new shares; dilution at the midpoint SUPPLY, not constant ownership.
            observed_mid=(L0+L1)//2
            hypot_fraction=q/(observed_mid+q) if observed_mid+q else 0.
            old_x=exact_new_fee_raw(observed_mid,dfx)
            old_y=exact_new_fee_raw(observed_mid,dfy)
            newx=old_x*hypot_fraction;newy=old_y*hypot_fraction
            fee+=newx/1e9*sol+newy/1e6
            gross_x+=newx;gross_y+=newy
            if newx or newy:contributing+=1
        flows.append(dict(
            start_slot=a["slot"],end_slot=b["slot"],
            elapsed_seconds=(parse_time(b["observed_utc"])-parse_time(a["observed_utc"])).total_seconds(),
            fee_usdc_shadow=fee,
            x_sol_shadow=gross_x/1e9,
            y_usdc_shadow=gross_y/1e6,
            contributing_bins=contributing,
            changed_supply_bins=changed,missing_position_bins=missing))
    if any(z["missing_position_bins"] for z in flows):
        valid=False
    else:valid=True
    divergence=dlmm_loss(dlmm_bins(capital,spot0,step),spot1)
    return dict(pool=first["pool"],capital_usdc=capital,step_bps=step,
      initial_spot_bin_proxy=spot0,final_spot_bin_proxy=spot1,
      initial_active_bin=aid,final_active_bin=last["active_bin_id"],
      observation_seconds=sum(z["elapsed_seconds"] for z in flows),
      position_bin_count=70,all_bin_intervals_observed=valid,
      fee_usdc_shadow=sum(z["fee_usdc_shadow"] for z in flows),
      ideal_terminal_divergence_vs_HODL=divergence,
      conditional_fee_minus_divergence_usdc=sum(z["fee_usdc_shadow"] for z in flows)-divergence,
      windows=flows,
      warnings=["Unrealized synthetic 70-bin deposit, not live PositionV2 shares",
        "Fixed-share estimate uses initial reserve-to-supply proportional mint rule",
        "Fees estimated using midpoint observed old liquidity and diluted hypothetical q",
        "We assume adding liquidity does not change swap volumes, flow mix or on-chain fee growth",
        "Endpoint-active-bin-derived spot proxy does not capture within-Bin price changes",
        "Same active ID does NOT imply zero market price movement (particularly 20bps Bin Step)",
        "Only 704 seconds of market behavior, never annualize or use as expected fee yield",
        "No slippage, composition fee, withdrawal gas, external hedge or principal/USD-delta risk"])
def main():
    p=argparse.ArgumentParser()
    p.add_argument("--snap",type=Path,action="append",required=True)
    p.add_argument("--out",type=Path,default=Path("research-artifacts/dlmm_fixed_bin_cohort.json"))
    a=p.parse_args()
    snapshots=[json.loads(x.read_text()) for x in a.snap]
    if len(snapshots)<3:raise ValueError("Need at least three real snapshots")
    output={"status":"ok","created_utc":dt.datetime.now(dt.timezone.utc).isoformat(),
      "source":"exact Q64 real fee counters; estimated diluted hypothetical static LP",
      "pools":{}}
    for name in NAMES:
        output["pools"][name]={}
        for capital in (2000.,10000.):
            result=fee_ledger(snapshots,name,capital)
            output["pools"][name][str(int(capital))]=result
            print(json.dumps({"pool":name,"capital":capital,
              "seconds":round(result["observation_seconds"]),
              "fee_usdc":round(result["fee_usdc_shadow"],6),
              "ideal_divergence_usdc":round(result["ideal_terminal_divergence_vs_HODL"],6),
              "conditional_fee_minus_divergence":round(result["conditional_fee_minus_divergence_usdc"],6),
              "all_measured":result["all_bin_intervals_observed"]}),flush=True)
    a.out.parent.mkdir(parents=True,exist_ok=True)
    a.out.write_text(json.dumps(output,indent=2)+"\n")
if __name__=="__main__":main()
