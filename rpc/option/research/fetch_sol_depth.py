#!/usr/bin/env python3
"""Read-only Deribit SOL_USDC order-book depth enrichment, preserving quote source and timestamps."""
import argparse,json,math,time
from datetime import datetime,timezone
from pathlib import Path
from fetch_deribit_sol import get

def parse_asks(payload):
    out=[]
    for row in payload.get("asks",[]):
        try:p,q=float(row[0]),float(row[1])
        except (TypeError,ValueError,IndexError):continue
        if math.isfinite(p) and math.isfinite(q) and p>0 and q>0:
            out.append(dict(price_usdc_per_sol=p,amount_sol=q))
    return sorted(out,key=lambda z:z["price_usdc_per_sol"])

def enrich(snapshot,lower=.8,upper=1.2,depth=20,pause=.04,request=get):
    if snapshot.get("status")!="ok":raise ValueError("source snapshot not OK")
    spot=float(snapshot["spot_usdc_per_sol"]);lo=spot*lower;hi=spot*upper
    if not (0<lo<spot<hi and 1<=depth<=100):raise ValueError("bad interval or depth")
    r=dict(snapshot);r["options"]=[dict(z) for z in snapshot["options"]]
    r["depth_started_utc"]=datetime.now(timezone.utc).isoformat()
    tried=filled=errors=0
    for o in r["options"]:
        o["book_asks"]=[];o["book_status"]="not_requested"
        k=float(o["strike"]);side=o["side"]
        if not (lo<=k<=hi) or (side=="put" and k>spot) or (side=="call" and k<spot):continue
        if not o.get("two_sided"):continue
        tried+=1
        try:
            b=request("get_order_book",instrument_name=o["instrument"],depth=depth)
            asks=parse_asks(b)
            o["book_asks"]=asks
            o["book_timestamp_ms"]=b.get("timestamp")
            o["book_status"]="ok" if asks else "empty"
            if asks:filled+=1
        except Exception as exc:
            o["book_status"]="fetch_failed";o["book_error"]=str(exc)[:160]
            errors+=1
        if pause:time.sleep(pause)
    r["depth_finished_utc"]=datetime.now(timezone.utc).isoformat()
    r["depth_attempted"]=tried;r["depth_with_asks"]=filled
    r["depth_errors"]=errors;r["depth_level_limit"]=depth
    r["depth_note"]="Non-atomic order books, bids and asks may change between calls; ask amount is underlying SOL."
    return r

def main():
    p=argparse.ArgumentParser()
    p.add_argument("--input",type=Path,required=True)
    p.add_argument("--output",type=Path,required=True)
    p.add_argument("--depth",type=int,default=20)
    a=p.parse_args()
    r=enrich(json.loads(a.input.read_text()),depth=a.depth)
    a.output.parent.mkdir(parents=True,exist_ok=True)
    a.output.write_text(json.dumps(r,indent=2)+"\n")
    print(json.dumps({k:r[k] for k in ("depth_attempted","depth_with_asks","depth_errors")}))
    return 0 if r["depth_with_asks"]>0 else 1
if __name__=="__main__":raise SystemExit(main())
