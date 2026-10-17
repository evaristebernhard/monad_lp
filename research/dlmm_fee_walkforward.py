#!/usr/bin/env python3
"""Tiny walk-forward *counterfactual* DLMM experiment, not a trading backtest.

At t1, allocate solely from t0->t1 public Bin fee-growth; evaluate fee
capture and ideal short-Gamma divergence in t1->t2. No future information is
used to choose position bins. The position was NOT minted; adding liquidity
would alter on-chain flow and fee-growth, therefore these are shadow payoffs.
"""
import argparse,datetime as dt,json,math
from pathlib import Path
from fee_gamma_lvr_theory import allocate_fee_budget,bs_option
from dlmm_q64_fee_capture_study import exact_new_fee_raw,spot_from_active,reserves_usdc,parse_time
W_LIST=(2000.,10000.)
HORIZON_SECONDS=240.
SIGMA=.538

def measured_fees(t0,t1):
    """Approx total fees awarded to all original holders, in quote USD."""
    old=t0["bin_fee_growth_raw"];new=t1["bin_fee_growth_raw"]
    spot=spot_from_active(t0)
    result={}
    for key,a in old.items():
        b=new.get(key)
        if not b:continue
        dx=int(b["fee_growth_x_raw"])-int(a["fee_growth_x_raw"])
        dy=int(b["fee_growth_y_raw"])-int(a["fee_growth_y_raw"])
        if dx<0 or dy<0:continue
        qa=int(a["liquidity_supply"]);qb=int(b["liquidity_supply"])
        x=(exact_new_fee_raw(qa,dx)+exact_new_fee_raw(qb,dx))/2
        y=(exact_new_fee_raw(qa,dy)+exact_new_fee_raw(qb,dy))/2
        result[int(key)]=dict(fee_usdc=x/1e9*spot+y/1e6,
            changed_supply=qa!=qb)
    return result
def choose(t0,t1,wealth):
    """Training only, prospective entry t1."""
    historical=measured_fees(t0,t1)
    interval=(parse_time(t1["observed_utc"])-parse_time(t0["observed_utc"])).total_seconds()
    if interval<=0:raise ValueError("non-forward")
    scale=HORIZON_SECONDS/interval
    S=spot_from_active(t1)
    active=int(t1["active_bin_id"]);step=int(t1["bin_step_bps"])
    current=t1["bin_fee_growth_raw"]
    bins=[]
    for bid in [*range(active-35,active),*range(active+1,active+36)]:
        row=current.get(str(bid))
        # Cannot faithfully reconstruct active position value in absent BinArray.
        if row is None:continue
        ci=reserves_usdc(row,S)
        if ci<=0:continue
        side="put" if bid<active else "call"
        strike=S*math.exp((bid-active)*math.log1p(step/10000))
        ell=bs_option(S,strike,SIGMA,HORIZON_SECONDS/(365*86400),side)
        ell=ell/(strike if side=="put" else S)
        bins.append(dict(id=bid,capital=ci,fee_budget=max(0.,historical.get(bid,{}).get("fee_usdc",0.)*scale),
                   side=side,strike=strike,risk_per_usd=max(ell,1e-12)))
    if not bins:return None
    weights=allocate_fee_budget([z["fee_budget"] for z in bins],
             [z["capital"] for z in bins],[z["risk_per_usd"] for z in bins],wealth)["allocations_usdc"]
    for z,w in zip(bins,weights):z["optimal_usdc"]=w
    for z in bins:z["equal_usdc"]=wealth/70
    return bins
def eval_alloc(bins,weighted_type,t1,t2,capital):
    fee_next=measured_fees(t1,t2)
    s0=spot_from_active(t1);s2=spot_from_active(t2)
    received=0.;D=0.;unknown=0;bybin=[]
    for b in bins:
        w=b[weighted_type]
        if w<=1e-8:continue
        if b["id"] not in fee_next:
            unknown+=1;continue
        fraction=w/(w+b["capital"])
        earned=fraction*fee_next[b["id"]]["fee_usdc"]
        if b["side"]=="put":
            D+=w/b["strike"]*max(b["strike"]-s2,0.)
        else:
            D+=w/s0*max(s2-b["strike"],0.)
        received+=earned
        bybin.append((b["id"],w,earned,fee_next[b["id"]]["changed_supply"]))
    invested=sum(b[weighted_type] for b in bins)
    return {"fee_usdc":received,"divergence_vs_HODL_usdc":D,
          "shadow_fee_minus_divergence_usdc":received-D,
          "invested_usdc":invested,"unallocated_usdc":capital-invested,
          "unknown_end_fee_bin_count":unknown,"activated_bin_count":len(bybin),
          "bins_with_fee_observed_next":sum(bool(fee_next.get(b["id"],{}).get("fee_usdc")) for b in bins),
          "fee_counter_supply_changed_next":sum(flag for _,_,_,flag in bybin),
          "warning":"Unfilled counterfactual shadow position; not a realized fee or achievable fill"}
def backtest(samples,capital):
    out=[]
    for i in range(1,len(samples)-1):
        b=choose(samples[i-1],samples[i],capital)
        if not b:continue
        per={}
        for name,field in (("hindsight_derived_KKT","optimal_usdc"),
                           ("simple_equal_70_bins","equal_usdc")):
            per[name]=eval_alloc(b,field,samples[i],samples[i+1],capital)
        out.append({"training_sample_idx":i-1,"entry_sample_idx":i,
              "evaluation_sample_idx":i+1,
              "entry_utc":samples[i]["observed_utc"],
              "end_utc":samples[i+1]["observed_utc"],
              "entry_spot_bin_proxy":spot_from_active(samples[i]),
              "end_spot_bin_proxy":spot_from_active(samples[i+1]),
              "candidate_existing_nonzero_bins":len(b),
              "training_fee_hotspot_bin_count":sum(z["fee_budget"]>0 for z in b),
              "strategies":per})
    return out
def main():
    p=argparse.ArgumentParser()
    p.add_argument("--snap",action="append",type=Path,required=True)
    p.add_argument("--out",type=Path,default=Path("research-artifacts/dlmm_fee_walkforward.json"))
    a=p.parse_args()
    snapshots=[json.loads(path.read_text()) for path in a.snap]
    if len(snapshots)<3:raise ValueError("need 3 chronological snapshots")
    allp={"created_utc":dt.datetime.now(dt.timezone.utc).isoformat(),"model":"observed-fee shadow walk-forward",
        "sample_count":len(snapshots),
        "horizon_decision_seconds":HORIZON_SECONDS,
        "riskneutral_iv_assumption":SIGMA,"pools":{},
        "risks":["Very few out-of-sample windows; no robust market inference.",
        "No LP position was minted; changing liquidity changes trade routing and fees.",
        "Only selected near-active Bin arrays from each snapshot are available.",
        "Pool Bin fee growth × endpoint supply is approximate with intraperiod liquidity changes.",
        "Bin prices are coarse spot proxies; no external fair spot or actual execution.",
        "KKT may leave capital idle; profits are relative-to-HODL on deployed LP capital only.",
        "No annualized return or future profitability claimed."]}
    for name in ("meteora_1bp_SOL_USDC","meteora_4bp_SOL_USDC","meteora_20bp_SOL_USDC"):
        series=[z["pools"][name] for z in snapshots]
        allp["pools"][name]={}
        for W in W_LIST:
            result=backtest(series,W)
            allp["pools"][name][str(int(W))]=result
            print(json.dumps({"pool":name,"capital":W,"windows":len(result),
              "train_positive_bins":[z["training_fee_hotspot_bin_count"] for z in result],
              "net_shadow_KKT":[round(z["strategies"]["hindsight_derived_KKT"]["shadow_fee_minus_divergence_usdc"],5) for z in result],
              "net_shadow_equal":[round(z["strategies"]["simple_equal_70_bins"]["shadow_fee_minus_divergence_usdc"],5) for z in result],
              "KKT_invested":[round(z["strategies"]["hindsight_derived_KKT"]["invested_usdc"],2) for z in result]}),flush=True)
    a.out.parent.mkdir(parents=True,exist_ok=True);a.out.write_text(json.dumps(allp,indent=2)+"\n")
if __name__=="__main__":main()
