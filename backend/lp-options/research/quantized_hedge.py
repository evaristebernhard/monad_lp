#!/usr/bin/env python3
"""Research-only quantized SOL option hedge certificate (no orders or DB writes)."""
import argparse, json, math
from pathlib import Path

def model(kind, spot, wealth, low, high):
    if not (0 < low < spot < high and wealth > 0):
        raise ValueError("require 0 < low < spot < high; wealth > 0")
    if kind == "cpmm":
        L=wealth/(2*math.sqrt(spot))
        x0=wealth/(2*spot)
        def value(s): return 2*L*math.sqrt(s)
        def delta(s): return L/math.sqrt(s)
    elif kind == "clmm":
        d=2*math.sqrt(spot)-spot/math.sqrt(high)-math.sqrt(low)
        L=wealth/d
        x0=L*(1/math.sqrt(spot)-1/math.sqrt(high))
        def value(s):
            if s<=low: return L*s*(1/math.sqrt(low)-1/math.sqrt(high))
            if s>=high: return L*(math.sqrt(high)-math.sqrt(low))
            return L*(2*math.sqrt(s)-s/math.sqrt(high)-math.sqrt(low))
        def delta(s):
            if s<=low: return L*(1/math.sqrt(low)-1/math.sqrt(high))
            if s>=high: return 0.
            return L*(1/math.sqrt(s)-1/math.sqrt(high))
    else: raise ValueError("unknown AMM")
    return L,x0,lambda s: wealth+x0*(s-spot)-value(s),lambda s:x0-delta(s)

def construct(L,low,spot,high,nput,ncall):
    if min(nput,ncall)<1: raise ValueError("each side needs a positive interval count")
    legs=[]
    for side,a,b,n in (("put",low,spot,nput),("call",spot,high,ncall)):
        step=(b**.25-a**.25)/n
        for i in range(n):
            lo=(a**.25+i*step)**4
            hi=(a**.25+(i+1)*step)**4
            legs.append(dict(side=side,strike=math.sqrt(lo*hi),quantity_sol=L*(lo**(-.5)-hi**(-.5))))
    err=L*max(((spot**.25-low**.25)/nput)**2,((high**.25-spot**.25)/ncall)**2)
    return legs,err

def quantize(ideal,contract_size,listed=None):
    if contract_size<=0: raise ValueError("bad contract size")
    result=[]
    for z in ideal:
        possible=[float(r["strike"]) for r in (listed or []) if r["side"]==z["side"]]
        if listed is not None and not possible: raise ValueError("missing listed strike on "+z["side"])
        k=min(possible,key=lambda v:abs(v-z["strike"])) if listed is not None else z["strike"]
        contracts=math.floor(z["quantity_sol"]/contract_size+.5)
        result.append(dict(side=z["side"],strike=k,quantity_sol=contracts*contract_size,contracts=contracts,
                           ideal_strike=z["strike"],ideal_quantity_sol=z["quantity_sol"]))
    return result

def payoff(s,legs):
    return sum(z["quantity_sol"]*(max(z["strike"]-s,0) if z["side"]=="put" else max(s-z["strike"],0)) for z in legs)

def slope(s,legs):
    return sum(z["quantity_sol"]*(-1 if z["side"]=="put" else 1) for z in legs
               if (s<z["strike"] if z["side"]=="put" else s>z["strike"]))

def exact_error(kind,low,spot,high,L,x0,loss,legs):
    """Certified exact sup |D-H| on [low,high], checking stationary minima too."""
    cuts=sorted(set([low,spot,high]+[z["strike"] for z in legs if low<z["strike"]<high]))
    candidates=list(cuts)
    for a,b in zip(cuts,cuts[1:]):
        c=slope((a+b)/2,legs)
        denom=(x0-c) if kind=="cpmm" else (x0+L/math.sqrt(high)-c)
        if denom>0:
            stationary=(L/denom)**2
            if a<stationary<b: candidates.append(stationary)
    signed=[(s,loss(s)-payoff(s,legs)) for s in candidates]
    worst=max(signed,key=lambda z:abs(z[1]))
    return dict(max_abs_residual_usd=abs(worst[1]),price_at_max=worst[0],
                signed_residual_usd=worst[1],candidates=len(candidates))

def certificate(ideal,actual,ideal_error,low,high):
    strike=sum(a["quantity_sol"]*abs(a["strike"]-b["strike"]) for a,b in zip(ideal,actual))
    rounding=0.
    for a,b in zip(ideal,actual):
        cap=max(0,b["strike"]-low) if b["side"]=="put" else max(0,high-b["strike"])
        rounding+=abs(a["quantity_sol"]-b["quantity_sol"])*cap
    return dict(ideal_error_usd=ideal_error,strike_error_bound_usd=strike,
                rounding_error_bound_usd=rounding,total_bound_usd=ideal_error+strike+rounding)

def main():
    p=argparse.ArgumentParser(description=__doc__)
    for name,default in [("spot",112.45),("wealth",10000.),("low",89.96),("high",134.94),
                         ("contract-size",10.)]:
        p.add_argument("--"+name,type=float,default=default)
    p.add_argument("--kind",choices=["cpmm","clmm"],default="cpmm")
    p.add_argument("--n-put",type=int,default=2)
    p.add_argument("--n-call",type=int,default=2)
    p.add_argument("--strikes-json",type=Path)
    args=p.parse_args()
    L,x0,loss,_=model(args.kind,args.spot,args.wealth,args.low,args.high)
    ideal,err=construct(L,args.low,args.spot,args.high,args.n_put,args.n_call)
    listed=json.loads(args.strikes_json.read_text()) if args.strikes_json else None
    actual=quantize(ideal,args.contract_size,listed)
    print(json.dumps(dict(kind=args.kind,spot=args.spot,interval=[args.low,args.high],
        ideal_legs=ideal,actual_legs=actual,
        certificate=certificate(ideal,actual,err,args.low,args.high),
        exact=exact_error(args.kind,args.low,args.spot,args.high,L,x0,loss,actual),
        warning="Research only; no bid/ask, execution, or PnL guarantee"),indent=2))

if __name__=="__main__": main()
