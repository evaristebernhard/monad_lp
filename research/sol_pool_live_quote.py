#!/usr/bin/env python3
"""Compare archived Solana pools with latest read-only DexScreener pool observations.

Outputs public pool-level aggregates only. Never send RPC endpoint URLs or wallet data.
"""
import argparse,datetime,json,time
from pathlib import Path
from urllib.request import Request,urlopen

FAMILIES=("solana_standard","solana_clmm","solana_dlmm","solana_whirlpool")

def fetch_pair(address):
    url="https://api.dexscreener.com/latest/dex/pairs/solana/"+address
    req=Request(url,headers={"accept":"application/json","user-agent":"arbitage-sol-lp-coverage-research/0.1"})
    with urlopen(req,timeout=18) as resp: r=json.load(resp)
    pairs=[p for p in r.get("pairs") or [] if p.get("chainId")=="solana" and p.get("pairAddress")==address]
    if not pairs:return {"status":"pair_not_returned"}
    p=pairs[0]
    return {"status":"ok","dex_id":p.get("dexId"),"pair_address":address,
            "pair":str(p.get("baseToken",{}).get("symbol","?"))+"/"+str(p.get("quoteToken",{}).get("symbol","?")),
            "liquidity_usd":(p.get("liquidity") or {}).get("usd"),
            "volume_24h_usd":(p.get("volume") or {}).get("h24"),
            "price_usd":p.get("priceUsd"),"price_change_24h_pct":(p.get("priceChange") or {}).get("h24")}

def main():
    a=argparse.ArgumentParser()
    a.add_argument("--coverage",required=True,type=Path)
    a.add_argument("--output",required=True,type=Path)
    v=a.parse_args()
    coverage=json.loads(v.coverage.read_text())
    pools=coverage.get("sources",{}).get("catalog",{}).get("top_sol_stable_pairs",[])
    out={"fetched_at_utc":datetime.datetime.now(datetime.timezone.utc).isoformat(),
         "source":"DexScreener public /latest/dex/pairs/solana/{pool}",
         "results":[]}
    for family in FAMILIES:
        matches=[p for p in pools if p.get("family")==family and (p.get("tvl_usd") or 0)>=1000000]
        if not matches:continue
        old=matches[0]
        try:latest=fetch_pair(old["pool"])
        except Exception as e:latest={"status":"fetch_failed","error_type":type(e).__name__}
        out["results"].append({"family":family,"catalog_snapshot":old,"live":latest})
        time.sleep(.15)
    v.output.write_text(json.dumps(out,indent=2)+"\n")
    print(json.dumps({"results":[{"family":x["family"],"status":x["live"]["status"]} for x in out["results"]]}))
if __name__=="__main__":main()
