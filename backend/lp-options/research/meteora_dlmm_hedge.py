#!/usr/bin/env python3
"""Static DLMM curvature hedge: exact finite-breakpoint residual, no live trading."""
import argparse,json
from pathlib import Path

def validate(bins,spot):
    if spot<=0:raise ValueError("positive spot required")
    strikes=[float(b["strike"]) for b in bins]
    if any(k<=0 for k in strikes) or strikes!=sorted(set(strikes)):raise ValueError("strictly increasing bin strikes required")
    if any(float(b["delta_sol"])<0 for b in bins):raise ValueError("negative conversion weight")
def loss(spot,price,bins):
    validate(bins,spot)
    return sum(float(b["delta_sol"])*(
        max(float(b["strike"])-price,0) if float(b["strike"])<spot else
        max(price-float(b["strike"]),0)) for b in bins)
def payout(price,legs):
    return sum(float(z["quantity_sol"])*(
        max(float(z["strike"])-price,0) if z["side"]=="put" else
        max(price-float(z["strike"]),0)) for z in legs)
def exact_residual(spot,bins,legs,low,high):
    if not 0<low<spot<high:raise ValueError("invalid risk range")
    validate(bins,spot)
    for z in legs:
        if z["side"] not in ("put","call") or z["quantity_sol"]<0:raise ValueError("invalid option leg")
    critical=sorted(set([low,high,spot]+[float(z["strike"]) for z in bins+legs if low<=float(z["strike"])<=high]))
    results=[{"price":s,"signed_residual":loss(spot,s,bins)-payout(s,legs)} for s in critical]
    maximum=max(results,key=lambda z:abs(z["signed_residual"]))
    return {"max_abs_residual_usdc":abs(maximum["signed_residual"]),"worst_price":maximum["price"],
            "signed_worst_residual_usdc":maximum["signed_residual"],"critical_prices_checked":len(critical)}
def ideal_legs(spot,bins):
    validate(bins,spot)
    return [{"side":"put" if float(b["strike"])<spot else "call",
             "strike":float(b["strike"]),"quantity_sol":float(b["delta_sol"])}
            for b in bins if float(b["delta_sol"])>0]
def main():
    p=argparse.ArgumentParser()
    p.add_argument("--input",type=Path,required=True,help="JSON with spot, low, high and bins")
    p.add_argument("--output",type=Path,required=True)
    a=p.parse_args()
    x=json.loads(a.input.read_text())
    ideal=ideal_legs(x["spot"],x["bins"])
    result=exact_residual(x["spot"],x["bins"],x.get("legs",ideal),x["low"],x["high"])
    result["interpretation"]="ideal discrete Gamma only; requires verified actual bin conversion weights"
    a.output.parent.mkdir(parents=True,exist_ok=True)
    a.output.write_text(json.dumps(result,indent=2)+"\n")
    print(json.dumps(result))
if __name__=="__main__":main()
