#!/usr/bin/env python3
"""Cross-check chosen Raydium small pools against public official API v3.

No authenticated RPC URLs, private keys or wallet state. Only explicitly named IDs.
"""
import datetime as dt,json,urllib.request
from pathlib import Path
from urllib.parse import urlencode
POOL_IDS=[
 "7XawhbbxtsRcQA8KTkHT9f9nc6d69UwqCDh6U5EEbEmX",
 "CYbD9RaToYMtWKA7QZyoLahnHdWq553Vm62Lh6qWtuxq",
]
def main():
    url="https://api-v3.raydium.io/pools/info/ids?"+urlencode({"ids":",".join(POOL_IDS)})
    result={"source":"Raydium API V3 /pools/info/ids","queried_utc":dt.datetime.now(dt.timezone.utc).isoformat(),
            "status":"unavailable","pools":[]}
    try:
        req=urllib.request.Request(url,headers={"accept":"application/json","user-agent":"arbitage-small-pool-fee-research/0.3"})
        with urllib.request.urlopen(req,timeout=22) as resp:res=json.load(resp)
        if not res.get("success"):raise RuntimeError("official API success false")
        raw=res.get("data") or []
        if isinstance(raw,dict):raw=raw.get("data",raw.get("list",[]))
        if not isinstance(raw,list):raise RuntimeError("unrecognized official response shape")
        for p in raw:
            if str(p.get("id")) not in POOL_IDS:continue
            minta=p.get("mintA") or {};mintb=p.get("mintB") or {}
            config=p.get("config") or {}
            observation={"pool":p.get("id"),"pool_type":p.get("type"),"program_id":p.get("programId"),
                         "pool_tvl_usd":p.get("tvl"),"fee_rate_field":p.get("feeRate"),
                         "config_id":config.get("id"),
                         "base_mint":minta.get("address"),"quote_mint":mintb.get("address"),
                         "day_volume_usd":(p.get("day") or {}).get("volume"),
                         "day_fee_usd":(p.get("day") or {}).get("volumeFee")}
            result["pools"].append(observation)
        result["status"]="ok" if result["pools"] else "pool_ids_not_found"
    except Exception as exc:result["error_type"]=type(exc).__name__
    Path("research-artifacts").mkdir(exist_ok=True)
    Path("research-artifacts/raydium_small_pool_verification.json").write_text(json.dumps(result,indent=2)+"\n")
    print(json.dumps(result))
if __name__=="__main__":main()
