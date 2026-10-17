#!/usr/bin/env python3
"""Live Solana LP pool markets versus a SAME-expiry Deribit SOL integer hedge.

Research only: catalog pool identities/fee tiers may be stale; current market h24 and
books are non-atomic; CLMM position range is hypothetical; no user fee PnL or trades.
"""
import argparse, datetime as dt, json, math, sqlite3, sys, time
from pathlib import Path
from urllib.request import Request, urlopen
import numpy as np
from scipy.optimize import milp, Bounds, LinearConstraint

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/"backend/lp-options/research"))
from quantized_hedge import model
from integer_hedge import eligible, pay

WSOL="So11111111111111111111111111111111111111112"
USDC="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
WETH="0xC02aaa39b223FE8D0a0e5C4F27eAD9083C756Cc2".lower()
ETH_USDC="0xA0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".lower()
NATIVE_ETH="0x0000000000000000000000000000000000000000"
# Explicit pool IDs are carried from existing Git LFS catalogue; do not infer validity
# from pool symbols alone. A failing mint/venue check means "not verified".
SOL_POOLS={
    "raydium_amm_v4":{"pool":"58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2",
                      "family":"solana_standard","expected_dex":"raydium","share_assumption":.88,
                      "model":"cpmm","share_status":"documented AMM v4 split; subtype needs on-chain verification"},
    "raydium_clmm":{"pool":"3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv",
                    "family":"solana_clmm","expected_dex":"raydium","share_assumption":.84,
                    "model":"clmm","share_status":"documented CLMM share; pool config fee still to verify"},
    "orca_whirlpool":{"pool":"Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE",
                      "family":"solana_whirlpool","expected_dex":"orca","share_assumption":None,
                      "model":"clmm","share_status":"protocol share unverified; present gross fee and 75%-100% sensitivity"},
    "meteora_dlmm_reference":{"pool":"5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6",
                      "family":"solana_dlmm","expected_dex":"meteora","share_assumption":.90,
                      "model":"dlmm_reference","share_status":"saved on-chain protocol share from Oct 9; dynamic fees not modeled"}
}
def now():return dt.datetime.now(dt.timezone.utc).isoformat()
def sqlite_ro(path):
    if Path(path).open("rb").read(16)!=b"SQLite format 3\x00":raise ValueError("LFS SQLite bytes missing")
    db=sqlite3.connect("file:"+str(Path(path).resolve())+"?mode=ro",uri=True)
    db.row_factory=sqlite3.Row;db.execute("PRAGMA query_only=ON")
    return db
def lookup_catalog(db,pool):
    z=db.execute("""SELECT pool_address,protocol,pool_type,chain,pair,token_a_symbol,token_b_symbol,
                           liquidity_usd,volume_24h_usd,fee_bps,lp_fee_apr_estimate,updated_at
                    FROM pools WHERE pool_address=? LIMIT 1""",(pool,)).fetchone()
    return dict(z) if z else None
def get_pair(chain,pool,tries=2):
    url="https://api.dexscreener.com/latest/dex/pairs/"+chain+"/"+pool
    err=None
    for i in range(tries):
        try:
            req=Request(url,headers={"accept":"application/json","user-agent":"arbitage-cross-protocol-research/0.2"})
            with urlopen(req,timeout=18) as response:payload=json.load(response)
            matches=[z for z in payload.get("pairs",[]) or [] if z.get("chainId")==chain
                     and z.get("pairAddress","").lower()==pool.lower()]
            if not matches:return {"status":"pair_not_returned"}
            p=matches[0]
            base=p.get("baseToken") or {};quote=p.get("quoteToken") or {}
            addresses={base.get("address","").lower(),quote.get("address","").lower()}
            required={WSOL.lower(),USDC.lower()} if chain=="solana" else {WETH,ETH_USDC}
            if addresses!=required and not (chain=="ethereum" and addresses=={NATIVE_ETH,ETH_USDC}):
                return {"status":"wrong_token_mints","base_symbol":base.get("symbol"),"quote_symbol":quote.get("symbol")}
            price=float(p.get("priceUsd") or 0) if base.get("address","").lower() in (WSOL.lower(),WETH,NATIVE_ETH) else 0.
            if not (math.isfinite(price) and price>0):
                native=float(p.get("priceNative") or 0)
                if native<=0:return {"status":"cannot_derive_volatile_asset_price"}
                price=1/native
            tvl=float((p.get("liquidity") or {}).get("usd") or 0)
            vol=float((p.get("volume") or {}).get("h24") or 0)
            if not (tvl>0 and vol>=0 and math.isfinite(tvl) and math.isfinite(vol)):
                return {"status":"bad_market_numbers"}
            return dict(status="ok",timestamp_utc=now(),dex_id=p.get("dexId"),
                       pair_address=pool,token_symbols=(base.get("symbol","?")+"/"+quote.get("symbol","?")),
                       volume_24h_usd=vol,tvl_usd=tvl,volatile_price_usd=price,
                       price_change_24h=(p.get("priceChange") or {}).get("h24"))
        except Exception as exc:
            err=type(exc).__name__
            if i+1<tries:time.sleep(1)
    return {"status":"fetch_failed","error_type":err}
def exact_oracle(kind,lo,hi,spot,poslo,poshi,L,x0,loss,positions):
    hinges=sorted(set([lo,hi,spot]+
        ([poslo,poshi] if kind=="clmm" else [])+
        [z["strike"] for z in positions if lo<=z["strike"]<=hi]))
    points=list(hinges)
    for a,b in zip(hinges,hinges[1:]):
        mid=(a+b)/2
        slope=sum((z["quantity_sol"]*(1 if z["side"]=="call" else -1))
                  for z in positions if (mid>z["strike"] if z["side"]=="call" else mid<z["strike"]))
        denom=x0-slope if kind=="cpmm" else x0+L/math.sqrt(poshi)-slope
        if denom>0:
            root=(L/denom)**2
            if a+1e-12<root<b-1e-12 and (kind=="cpmm" or poslo<root<poshi):points.append(root)
    vals=[(p,loss(p)-sum(z["quantity_sol"]*pay(p,z["side"],z["strike"]) for z in positions)) for p in points]
    spotworst,val=max(vals,key=lambda z:abs(z[1]))
    return {"max_abs_error_usdc":abs(val),"worst_price":spotworst,
            "signed_residual_usdc":val,"checked_points":len(points)}
def optimize_hedge(snap,kind,wealth,pos_width,stress=.1,lam=1.,max_lots=2,seconds=30,max_rounds=12,lots_override=None):
    spot=float(snap["spot_usdc_per_sol"]);plo=spot*(1-pos_width);phi=spot*(1+pos_width)
    lo=spot*(1-stress);hi=spot*(1+stress)
    L,x0,loss,_=model(kind,spot,wealth,plo,phi)
    lots=lots_override if lots_override is not None else eligible(snap,lo,hi,max_lots=max_lots)
    zero=exact_oracle(kind,lo,hi,spot,plo,phi,L,x0,loss,[])
    if not lots:return {"status":"no_quoted_whole_contract_lots","no_hedge":zero}
    n=len(lots)
    groups={}
    for i,z in enumerate(lots):groups.setdefault(z["instrument"],[]).append(i)
    cuts=sorted(set([lo,hi,spot]+([plo,phi] if kind=="clmm" else [])+[z["k"] for z in lots]))
    costs=np.array([z["cost"] for z in lots]+[lam])
    fixed=[]
    for group in groups.values():
        for prev,nxt in zip(group,group[1:]):
            row=np.zeros(n+1);row[nxt]=1;row[prev]=-1
            fixed.append((row,0.))
    chosen=None;valid=False;history=[]
    for iteration in range(max_rounds):
        rows=[];bounds_upper=[]
        for p in cuts:
            signed=np.array([z["size"]*pay(p,z["side"],z["k"]) for z in lots])
            D=loss(p)
            rows.extend([np.r_[signed,-1],np.r_[-signed,-1]])
            bounds_upper.extend([D,-D])
        for row,b in fixed:rows.append(row);bounds_upper.append(b)
        constraints=LinearConstraint(np.stack(rows),-np.inf,np.asarray(bounds_upper))
        res=milp(costs,integrality=np.r_[np.ones(n),0],
              bounds=Bounds(np.zeros(n+1),np.r_[np.ones(n),np.inf]),
              constraints=constraints,options={"time_limit":seconds,"mip_rel_gap":.004})
        if res.x is None:return {"status":"no_feasible_milp_result","solver_message":str(res.message),"no_hedge":zero}
        binary=np.rint(res.x[:n]).astype(int)
        legs=[]
        for ids in groups.values():
            nc=sum(int(binary[i]) for i in ids)
            if nc:
                z=lots[ids[0]]
                legs.append({"instrument":z["instrument"],"side":z["side"],"strike":z["k"],
                             "quantity_sol":z["size"]*nc,"contracts":nc})
        oracle=exact_oracle(kind,lo,hi,spot,plo,phi,L,x0,loss,legs)
        gap=oracle["max_abs_error_usdc"]-float(res.x[-1])
        history.append({"restricted_bound":float(res.x[-1]),"verified_bound":oracle["max_abs_error_usdc"],
                        "solver_status":int(res.status)})
        chosen=(binary,legs,oracle,res)
        if gap<=1e-5:
            valid=True;break
        cuts.append(oracle["worst_price"])
        cuts=sorted(set(cuts))
    binary,legs,oracle,res=chosen
    premium=sum(z["premium"] for z,b in zip(lots,binary) if b)
    fee=sum(z["fee"] for z,b in zip(lots,binary) if b)
    return {"status":"risk_certified" if valid else "cut_limit_reached",
            "model":kind,"lp_position_half_width_pct":pos_width*100,
            "stress_half_width_pct":stress*100,"wealth_usdc":wealth,
            "option_spot_usdc":spot,"option_expiry_ms":snap["expiry_ms"],
            "option_eligible_instruments":len(groups),"option_eligible_lots":n,
            "contracts":sum(z["contracts"] for z in legs),"legs":legs,
            "premium_usdc":premium,"estimated_exchange_fees_usdc":fee,
            "indicative_option_cost_usdc":premium+fee,
            "no_hedge":zero,"hedged":oracle,
            "premium_plus_worst_residual_usdc":premium+fee+oracle["max_abs_error_usdc"],
            "solver_status":int(res.status),"solver_gap":getattr(res,"mip_gap",None),
            "cut_iterations":len(history),"final_cut":history[-1],
            "note":"Illustrative initial position range; exact terminal HODL-relative principal loss, no LP fees."}
def main():
    p=argparse.ArgumentParser()
    p.add_argument("--catalog",type=Path,default=Path("data/lp_screener.db"))
    p.add_argument("--option-depth",type=Path,required=True)
    p.add_argument("--output",type=Path,default=Path("research-artifacts/cross_protocol_sol_options.json"))
    p.add_argument("--wealth",type=float,default=10000.)
    p.add_argument("--skip-live",action="store_true",help="for offline mocked tests, catalog metrics are NOT fresh")
    args=p.parse_args()
    db=sqlite_ro(args.catalog)
    snap=json.loads(args.option_depth.read_text())
    if snap["status"]!="ok" or snap.get("depth_with_asks",0)<=0:raise ValueError("Deribit depth missing")
    fetch_dt=dt.datetime.fromisoformat(snap["fetched_at"].replace("Z","+00:00"))
    days=(int(snap["expiry_ms"])/1000-fetch_dt.timestamp())/86400
    if not 1<=days<=35:raise ValueError("option remaining expiry outside [1,35] days")
    base={"sampled_at_utc":now(),"status":"ok","wealth_usdc":args.wealth,
          "deribit_fetched_at_utc":snap["fetched_at"],"expiry_utc":snap["expiry_utc"],
          "option_index_spot":snap["spot_usdc_per_sol"],"option_days_left":days,
          "depth_with_asks":snap["depth_with_asks"],"depth_attempted":snap["depth_attempted"],
          "option_books_non_atomic":True,"lp_position_actual":False,"markets":{},"hedges":{},
          "ethereum_catalog":{},
          "safety":"Research only; no actual positions, no profits, no orders or DB writes."}
    # One identical SOL option market and maturity for every Solana LP curve.
    hedge_scenarios=[("cpmm",None),("clmm",.025),("clmm",.05),("clmm",.10)]
    for model_type,width in hedge_scenarios:
        key="cpmm" if model_type=="cpmm" else "clmm_halfwidth_"+str(round(width*100,2))+"pct"
        result=optimize_hedge(snap,model_type,args.wealth,width or .10)
        base["hedges"][key]=result
        print(json.dumps({"hedge":key,"status":result["status"],
            "cost":result.get("indicative_option_cost_usdc"),
            "error":result.get("hedged",{}).get("max_abs_error_usdc"),
            "contracts":result.get("contracts")}),flush=True)
    for name,conf in SOL_POOLS.items():
        market={"protocol":name,"family":conf["family"],"pool_address":conf["pool"],
                "model_class":conf["model"],"catalog":lookup_catalog(db,conf["pool"]),
                "official_lp_fee_share_assumption":conf["share_assumption"],
                "fee_share_status":conf["share_status"]}
        if market["catalog"] is not None and market["catalog"]["pool_type"]!=conf["family"]:
            market["status"]="catalog_type_mismatch";base["markets"][name]=market;continue
        fresh=get_pair("solana",conf["pool"]) if not args.skip_live else {"status":"not_fetched_offline"}
        market["live"]=fresh;market["status"]=fresh["status"]
        if fresh["status"]=="ok":
            venue=str(fresh["dex_id"] or "").lower()
            if not (conf["expected_dex"] in venue or (name=="orca_whirlpool" and "whirlpool" in venue)):
                market["status"]="dex_mismatch"
            if abs(fresh["volatile_price_usd"]/float(snap["spot_usdc_per_sol"])-1)>.07:
                market["status"]="spot_basis_too_large"
            fee_bps=market["catalog"].get("fee_bps") if market["catalog"] else None
            if fee_bps is not None and float(fee_bps)>0 and market["status"]=="ok":
                gross=365*fresh["volume_24h_usd"]/fresh["tvl_usd"]*float(fee_bps)/10000
                f14=args.wealth*gross*days/365
                share=conf["share_assumption"]
                fee_model={"catalog_fee_bps_STALE_unverified":fee_bps,
                  "turnover_based_gross_apr_proxy":gross,
                  "gross_fee_reference_usdc_over_option_term":f14,
                  "not_position_realized_fee":True}
                if share is not None:
                    fee_model["assumed_lp_share"]=share
                    fee_model["pool_prorata_lp_apr_proxy"]=gross*share
                    fee_model["pool_prorata_fee_over_option_term_usdc"]=f14*share
                else:
                    fee_model["lp_share_sensitivity_75pct_to_100pct"]= [f14*.75,f14]
                market["fee_model"]=fee_model
                if conf["model"]!="dlmm_reference":
                    hedge_keys=["cpmm"] if conf["model"]=="cpmm" else [
                          "clmm_halfwidth_2.5pct","clmm_halfwidth_5.0pct","clmm_halfwidth_10.0pct"]
                    market["hedge_comparison"]={}
                    for hedge_key in hedge_keys:
                        h=base["hedges"][hedge_key]
                        if h["status"]!="risk_certified":continue
                        need=h["premium_plus_worst_residual_usdc"]
                        market["hedge_comparison"][hedge_key]={
                          "indicative_cost_plus_worst_residual_usdc":need,
                          "premium_only_hurdle_apr":h["indicative_option_cost_usdc"]/args.wealth*365/days,
                          "gross_fee_capture_fraction_hurdle":need/f14 if f14 else None,
                          "assumed_lp_fee_capture_fraction_hurdle":
                              (need/(f14*share) if f14 and share is not None else None),
                          "optimistic_pool_prorata_minus_premium_minus_worst_usdc":
                              (f14*share-need if share is not None else None),
                          "warning":"Fee proxy is pool-wide and hypothetical; NOT actual position cashflow."}
        base["markets"][name]=market
        print(json.dumps({"market":name,"status":market["status"],
             "tvl":fresh.get("tvl_usd"),"volume24h":fresh.get("volume_24h_usd"),
             "fee_bps":market.get("fee_model",{}).get("catalog_fee_bps_STALE_unverified")}),flush=True)
        time.sleep(.15)
    # EVM V2/V3 pools are researched as market-only: SOL options cannot hedge ETH.
    for family in ("constant_product_v2","concentrated_v3","uniswap_v4"):
        rows=db.execute("""SELECT pool_address,pair,protocol,pool_type,chain,
                                  liquidity_usd,volume_24h_usd,fee_bps,updated_at
                            FROM pools WHERE pool_type=? AND lower(chain)='ethereum'
                             AND lower(protocol) LIKE '%uniswap%'
                             AND ((upper(token_a_symbol) IN ('ETH','WETH') AND upper(token_b_symbol)='USDC')
                               OR (upper(token_b_symbol) IN ('ETH','WETH') AND upper(token_a_symbol)='USDC'))
                             AND liquidity_usd>100000 ORDER BY liquidity_usd DESC LIMIT 1""",(family,)).fetchall()
        base["ethereum_catalog"][family]=[]
        for row in rows:
            market={"catalog":dict(row),"model_category":"CPMM" if family=="constant_product_v2" else "concentrated_or_hooks",
                    "option_hedge":"NOT RUN: requires ETH options, actual pool fee decode / position range"}
            market["live"]=get_pair("ethereum",row["pool_address"]) if not args.skip_live else {"status":"not_fetched_offline"}
            base["ethereum_catalog"][family].append(market)
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(base,indent=2)+"\n")
    db.close()
    print(json.dumps({"status":"ok","available_markets":{k:v["status"] for k,v in base["markets"].items()},
      "hedge_results":{k:v["status"] for k,v in base["hedges"].items()},
      "eth_discovery":{k:len(v) for k,v in base["ethereum_catalog"].items()}}),flush=True)
if __name__=="__main__":main()
