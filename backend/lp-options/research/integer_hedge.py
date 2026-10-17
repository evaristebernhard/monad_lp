#!/usr/bin/env python3
"""Read-only integer SOL LP hedge; executable unit sizes, order-book ask ladder and exact error oracle."""
import argparse,json,math
from pathlib import Path
import numpy as np
from scipy.optimize import Bounds,LinearConstraint,milp
from quantized_hedge import model,exact_error

def pay(s,side,k):return max(k-s,0.) if side=="put" else max(s-k,0.)

def marginal_lots(asks,size,cap=3):
    """Whole 10-SOL increments may be filled across several ask price levels."""
    levels=[]
    for z in asks:
        try:p,q=float(z["price_usdc_per_sol"]),float(z["amount_sol"])
        except (KeyError,TypeError,ValueError):continue
        if math.isfinite(p) and math.isfinite(q) and p>0 and q>0:levels.append([p,q])
    levels.sort()
    out=[]
    for _ in range(cap):
        left=size;cost=0.
        for z in levels:
            q=min(left,z[1]);left-=q;z[1]-=q;cost+=q*z[0]
            if left<1e-8:break
        if left>1e-8:break
        out.append(cost)
    return out

def eligible(snap,low,high,max_lots=3):
    spot=float(snap["spot_usdc_per_sol"]);out=[]
    for z in snap["options"]:
        side=z["side"];k=float(z["strike"])
        if not low<=k<=high or (side=="put" and k>spot) or (side=="call" and k<spot):continue
        if z.get("book_status")!="ok" or int(z["expiry_ms"])!=int(snap["expiry_ms"]):continue
        size=float(z["contract_size_sol"])
        if abs(float(z.get("min_trade_amount") or size)-size)>1e-6:continue
        for j,premium in enumerate(marginal_lots(z.get("book_asks",[]),size,max_lots)):
            fee=min(0.0003*spot*size,0.125*premium) # public fee estimate, not personalized
            out.append(dict(instrument=z["instrument"],side=side,k=k,size=size,
                            marginal=j+1,premium=premium,fee=fee,cost=premium+fee))
    return out

def optimize(snap,kind,wealth,risk_lambda=1.,max_lots=3,max_error=None,
             low_pct=.8,high_pct=1.2,max_rounds=18):
    spot=float(snap["spot_usdc_per_sol"]);lo=spot*low_pct;hi=spot*high_pct
    if not (0<lo<spot<hi and wealth>0 and risk_lambda>0):raise ValueError("invalid inputs")
    L,x0,loss,_=model(kind,spot,wealth,lo,hi)
    lots=eligible(snap,lo,hi,max_lots);N=len(lots)
    if not lots:raise ValueError("no depth-backed lots")
    groups={}
    for i,z in enumerate(lots):groups.setdefault(z["instrument"],[]).append(i)
    cost=np.array([z["cost"] for z in lots]+[risk_lambda])
    bounds=Bounds(np.zeros(N+1),np.r_[np.ones(N),np.inf])
    constraints=[]
    def cut(s):
        h=np.array([z["size"]*pay(s,z["side"],z["k"]) for z in lots])
        d=loss(s)
        constraints.extend(((np.r_[-h,-1.],-np.inf,-d),
                            (np.r_[h,-1.],-np.inf,d)))
    for s in sorted(set([lo,spot,hi]+[z["k"] for z in lots])):cut(s)
    for group in groups.values():
        for before,after in zip(group,group[1:]):
            a=np.zeros(N+1);a[after]=1;a[before]=-1
            constraints.append((a,-np.inf,0.))
    if max_error is not None:
        a=np.zeros(N+1);a[-1]=1
        constraints.append((a,-np.inf,max_error))
    rounds=[];verified=False;last=None
    for iteration in range(max_rounds):
        lc=LinearConstraint(np.stack([z[0] for z in constraints]),
                            [z[1] for z in constraints],[z[2] for z in constraints])
        result=milp(cost,integrality=[1]*N+[0],bounds=bounds,constraints=lc,
                    options={"time_limit":50,"mip_rel_gap":.002})
        if result.x is None:return dict(status="no_solution_or_timeout",kind=kind,wealth_usdc=wealth,
                                        message=str(result.message),depth_lots=N)
        bits=np.rint(result.x[:N]).astype(int)
        positions=[]
        for ids in groups.values():
            n=int(sum(bits[j] for j in ids))
            if n:
                z=lots[ids[0]]
                positions.append(dict(instrument=z["instrument"],side=z["side"],strike=z["k"],
                                      contracts=n,quantity_sol=n*z["size"]))
        oracle=exact_error(kind,lo,spot,hi,L,x0,loss,positions)
        actual=oracle["max_abs_residual_usd"];claimed=float(result.x[-1])
        rounds.append(dict(round=iteration+1,restricted_E=claimed,exact_E=actual,solver_status=result.status))
        last=(result,bits,positions,oracle)
        if actual<=claimed+1e-5:
            verified=True
            break
        cut(oracle["price_at_max"])
    result,bits,positions,oracle=last
    premium=sum(z["premium"] for z,b in zip(lots,bits) if b)
    fees=sum(z["fee"] for z,b in zip(lots,bits) if b)
    baseline=exact_error(kind,lo,spot,hi,L,x0,loss,[])["max_abs_residual_usd"]
    return dict(status="risk_certified" if verified else "cut_limit_reached",
                solver_status=int(result.status),solver_message=str(result.message),
                solver_gap=getattr(result,"mip_gap",None),
                kind=kind,wealth_usdc=wealth,spot=spot,expiry_ms=snap["expiry_ms"],
                risk_interval_usdc=[lo,hi],risk_lambda=risk_lambda,
                eligible_instruments=len(groups),available_whole_lots=N,
                no_hedge_worst_usdc=baseline,
                exact_worst_residual_usdc=oracle["max_abs_residual_usd"],
                exact_worst_price_usdc=oracle["price_at_max"],
                observed_premium_usdc=premium,estimated_fee_usdc=fees,
                indicative_total_cost_usdc=premium+fees,
                objective_usdc=premium+fees+risk_lambda*oracle["max_abs_residual_usd"],
                contracts=sum(p["contracts"] for p in positions),legs=positions,rounds=rounds,
                note="Indicative sequential public books, no fill guarantee, LP fee model, or production trade.")

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument("--snapshot",type=Path,required=True)
    ap.add_argument("--output",type=Path,required=True)
    ap.add_argument("--kind",choices=["cpmm","clmm"],required=True)
    ap.add_argument("--wealth",type=float,default=10000.)
    ap.add_argument("--risk-lambda",type=float,default=1.)
    ap.add_argument("--max-error",type=float)
    a=ap.parse_args()
    r=optimize(json.loads(a.snapshot.read_text()),a.kind,a.wealth,a.risk_lambda,max_error=a.max_error)
    a.output.parent.mkdir(parents=True,exist_ok=True)
    a.output.write_text(json.dumps(r,indent=2)+"\n")
    print(json.dumps({k:v for k,v in r.items() if k not in ("legs","rounds")}))
    return 0 if r["status"]=="risk_certified" else 1
if __name__=="__main__":raise SystemExit(main())
