#!/usr/bin/env python3
"""Read-only Deribit SOL_USDC public options snapshot; no auth, trading, or DB writes."""
import argparse, json, math, sys, time
from datetime import datetime,timezone
from pathlib import Path
from urllib.parse import urlencode
from urllib.request import urlopen,Request

BASE="https://www.deribit.com/api/v2/public"
def positive(x):
    try:
        n=float(x)
        return n if math.isfinite(n) and n>0 else None
    except (TypeError,ValueError): return None

def get(method,**params):
    url=BASE+"/"+method+"?"+urlencode(params)
    last=None
    for i in range(3):
        try:
            req=Request(url,headers={"accept":"application/json","user-agent":"arbitage-sol-options-research/0.1"})
            with urlopen(req,timeout=25) as f: p=json.load(f)
            if "error" in p: raise RuntimeError(str(p["error"]))
            return p["result"]
        except Exception as exc:
            last=exc
            if i<2:time.sleep(i+1)
    raise RuntimeError(method+" failed: "+str(last))

def fetch(days=7,per_side=20):
    now=time.time()
    index=get("get_index_price",index_name="sol_usdc")
    spot=positive(index.get("index_price"))
    if spot is None:raise RuntimeError("SOL_USDC index missing")
    instruments=get("get_instruments",currency="USDC",kind="option",expired="false")
    sol=[z for z in instruments if str(z.get("instrument_name","")).startswith("SOL_USDC-") and z.get("is_active")]
    if not sol:raise RuntimeError("No active SOL_USDC options")
    expiry_candidates=sorted({int(z["expiration_timestamp"]) for z in sol if int(z["expiration_timestamp"])>now*1000})
    if not expiry_candidates:raise RuntimeError("No future expiry")
    later=[t for t in expiry_candidates if t>=(now+days*86400)*1000]
    expiry=min(later) if later else max(expiry_candidates)
    picked=[z for z in sol if int(z["expiration_timestamp"])==expiry]
    summary=get("get_book_summary_by_currency",currency="USDC",kind="option")
    quotes={z.get("instrument_name"):z for z in summary}
    options=[]
    for z in picked:
        name=z["instrument_name"];q=quotes.get(name,{})
        side=z.get("option_type");strike=positive(z.get("strike"))
        if side not in ("put","call") or strike is None:continue
        bid=positive(q.get("bid_price"));ask=positive(q.get("ask_price"))
        options.append(dict(instrument=name,side=side,strike=strike,expiry_ms=expiry,
          contract_size_sol=z.get("contract_size"),min_trade_amount=z.get("min_trade_amount"),
          bid_usdc_per_sol=bid,ask_usdc_per_sol=ask,mark_iv=q.get("mark_iv"),
          mark_price_usdc_per_sol=q.get("mark_price"),open_interest=q.get("open_interest"),
          volume_usd=q.get("volume_usd"),two_sided=bool(bid and ask and bid<=ask)))
    options.sort(key=lambda z:(z["side"],abs(math.log(z["strike"]/spot))))
    selected=[z for side in ("put","call") for z in [x for x in options if x["side"]==side][:per_side]]
    return dict(status="ok",fetched_at=datetime.now(timezone.utc).isoformat(),source="Deribit public REST",
                spot_usdc_per_sol=spot,target_days=days,expiry_ms=expiry,
                expiry_utc=datetime.fromtimestamp(expiry/1000,timezone.utc).isoformat(),
                total_expiry_instruments=len(picked),selected_count=len(selected),
                two_sided_count=sum(z["two_sided"] for z in selected),options=selected,
                note="Quote data only; not depth or guaranteed execution; price is USDC per SOL, not per 10-SOL contract.")

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("--days",type=float,default=7.)
    p.add_argument("--per-side",type=int,default=120)
    p.add_argument("--output",type=Path,default=Path("deribit_sol_snapshot.json"))
    a=p.parse_args()
    if not (a.days>0 and 1<=a.per_side<=250):p.error("invalid target days or per-side")
    a.output.parent.mkdir(parents=True,exist_ok=True)
    try:
        result=fetch(a.days,a.per_side)
    except Exception as exc:
        result=dict(status="fetch_failed",fetched_at=datetime.now(timezone.utc).isoformat(),error=str(exc))
    a.output.write_text(json.dumps(result,indent=2)+"\n")
    print(json.dumps({k:v for k,v in result.items() if k!="options"}))
    if result["status"]!="ok":return 1
    listed=[dict(side=z["side"],strike=z["strike"],instrument=z["instrument"])
            for z in result["options"] if z["two_sided"]]
    a.output.with_name("deribit_sol_quoted_strikes.json").write_text(json.dumps(listed,indent=2)+"\n")
    return 0

if __name__=="__main__":sys.exit(main())
