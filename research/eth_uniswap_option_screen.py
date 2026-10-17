#!/usr/bin/env python3
"""Live Ethereum LP V2/V3 and Deribit ETH_USDC min-0.1-contract hedge screen.

No private RPC, no orders. Estimates are NOT realized LP fee income or fillable quotes.
"""
import argparse,datetime as dt,json,math,sys,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path[:0]=[str(ROOT/"rpc/option/research"),str(ROOT/"research")]
from fetch_deribit_sol import get,positive
from fetch_sol_depth import parse_asks
from sol_cross_protocol_option_screen import get_pair,optimize_hedge

PAIRS={
 "uniswap_v2":("0xb4e16d0168e52d35cacd2c6185b44281ec28c9dc",30.,"cpmm"),
 "uniswap_v3":("0x88e6a0c2ddd26feeb64f039a2c41296fcb3f5640",5.,"clmm"),
 "uniswap_v4":("0xdce6394339af00981949f5f3baf27e3610c76326a700af57e4b3e3ae4977f78d",None,"hooks")
}
def utc():return dt.datetime.now(dt.timezone.utc).isoformat()
def capture(days=12,per_side=6):
    spot=positive(get("get_index_price",index_name="eth_usdc").get("index_price"))
    if spot is None:raise ValueError("ETH index unavailable")
    instr=[z for z in get("get_instruments",currency="USDC",kind="option",expired="false")
           if z.get("is_active") and str(z.get("instrument_name","")).startswith("ETH_USDC-")]
    nowms=time.time()*1000
    expiration=sorted({int(z["expiration_timestamp"]) for z in instr
                       if int(z["expiration_timestamp"])>nowms+days*86400000})
    if not expiration:raise ValueError("no future ETH option expiry")
    expiry=expiration[0]
    summaries={z.get("instrument_name"):z for z in get("get_book_summary_by_currency",currency="USDC",kind="option")}
    candidates=[]
    all_active=[z for z in instr if int(z["expiration_timestamp"])==expiry]
    for z in all_active:
        k=positive(z.get("strike"));side=z.get("option_type");name=z["instrument_name"]
        quote=summaries.get(name,{})
        bid=positive(quote.get("bid_price"));ask=positive(quote.get("ask_price"))
        if not(k and bid and ask and bid<=ask and side in ("put","call")):continue
        if not(.85*spot<k<1.15*spot):continue
        if (side=="put" and k>spot) or (side=="call" and k<spot):continue
        unit=positive(z.get("min_trade_amount"));mult=positive(z.get("contract_size"))
        if not(unit and mult):continue
        candidates.append(dict(instrument=name,side=side,strike=k,expiry_ms=expiry,
          minimum_underlying_eth=unit,contract_multiplier_eth=mult,mark_iv=quote.get("mark_iv"),
          bid=bid,ask=ask,book_status="not_requested"))
    selected=[z for side in ("put","call") for z in
        sorted([o for o in candidates if o["side"]==side],key=lambda x:abs(x["strike"]/spot-1))[:per_side]]
    for z in selected:
        try:
            b=get("get_order_book",instrument_name=z["instrument"],depth=20)
            z["book_asks"]=parse_asks(b);z["book_status"]="ok" if z["book_asks"] else "empty"
            z["book_timestamp_ms"]=b.get("timestamp")
        except Exception as exc:
            z["book_asks"]=[];z["book_status"]="fetch_failed";z["error_type"]=type(exc).__name__
        time.sleep(.06)
    return dict(status="ok",sampled_at_utc=utc(),spot_usdc_per_eth=spot,
        expiry_ms=expiry,expiry_utc=dt.datetime.fromtimestamp(expiry/1000,dt.timezone.utc).isoformat(),
        expiry_instruments=len(all_active),two_sided_otm=len(candidates),
        depth_attempted=len(selected),depth_with_asks=sum(x["book_status"]=="ok" for x in selected),
        options=selected)
def price_whole_lots(asks,step_eth,max_lots=16):
    levels=sorted([[float(z["price_usdc_per_sol"]),float(z["amount_sol"])] for z in asks
        if float(z["price_usdc_per_sol"])>0 and float(z["amount_sol"])>0],key=lambda x:x[0])
    prices=[]
    for _ in range(max_lots):
        remaining=step_eth;total=0.
        for r in levels:
            qty=min(r[1],remaining);remaining-=qty;r[1]-=qty;total+=r[0]*qty
            if remaining<=1e-9:break
        if remaining>1e-9:break
        prices.append(total)
    return prices
def eligible(s):
    spot=s["spot_usdc_per_eth"];lots=[]
    for z in s["options"]:
        if z["book_status"]!="ok":continue
        unit=float(z["minimum_underlying_eth"])
        mult=float(z["contract_multiplier_eth"])
        # Fail closed if Deribit changes the lot contract specification.
        if abs(mult-1)>1e-9 or abs(unit-.1)>1e-9:
            raise ValueError("ETH_USDC contract units changed; require human audit")
        for i,premium in enumerate(price_whole_lots(z["book_asks"],unit)):
            fee=min(.0003*spot*unit,.125*premium)
            lots.append(dict(instrument=z["instrument"],side=z["side"],k=float(z["strike"]),
                             size=unit,marginal=i+1,premium=premium,fee=fee,cost=premium+fee))
    return lots
def checks():
    q=price_whole_lots([dict(price_usdc_per_sol=10,amount_sol=.06),
                        dict(price_usdc_per_sol=20,amount_sol=.15)],.1,max_lots=3)
    assert len(q)==2 and abs(q[0]-1.4)<1e-8 and abs(q[1]-2.)<1e-8
    assert price_whole_lots([dict(price_usdc_per_sol=20,amount_sol=.09)],.1)==[]
    print("PASS: ETH fractional-contract ask-ladder pricing and missing depth")
def main():
    p=argparse.ArgumentParser()
    p.add_argument("--output",type=Path,default=Path("research-artifacts/eth_uniswap_option_screen.json"))
    p.add_argument("--self-test",action="store_true")
    a=p.parse_args()
    if a.self_test:return checks()
    snap=capture()
    if snap["depth_with_asks"]<4:raise ValueError("insufficient confirmed ETH ask depth")
    lots=eligible(snap)
    compatible=dict(status="ok",spot_usdc_per_sol=snap["spot_usdc_per_eth"],expiry_ms=snap["expiry_ms"],options=[])
    models={}
    for name,kind,width in [("v2_cpmm","cpmm",.1),("v3_clmm_halfwidth_5pct","clmm",.05),
                             ("v3_clmm_halfwidth_10pct","clmm",.1)]:
        opt=optimize_hedge(compatible,kind,10000,width,stress=.1,seconds=35,max_rounds=12,
                           lots_override=lots)
        opt["minimum_0p1_contract_lots"]=opt.get("contracts",0)
        opt["deribit_contracts_fractional_total"]=opt.get("contracts",0)*.1
        opt["option_quantity_warning"]="contracts is counted as 0.1-ETH lots, not whole 1-ETH contracts"
        models[name]=opt
        print(json.dumps({"hedge":name,"status":opt["status"],"lots":opt.get("contracts"),
            "cost":opt.get("indicative_option_cost_usdc"),
            "worst":opt.get("hedged",{}).get("max_abs_error_usdc")}),flush=True)
    horizon=(snap["expiry_ms"]/1000-
        dt.datetime.fromisoformat(snap["sampled_at_utc"]).timestamp())/86400
    pools={}
    for name,(addr,fee,kind) in PAIRS.items():
        live=get_pair("ethereum",addr)
        row={"pool":addr,"kind":kind,"current_public_market":live,
             "catalog_fee_bps_unverified":fee}
        if live["status"]=="ok" and abs(live["volatile_price_usd"]/snap["spot_usdc_per_eth"]-1)<.05:
            row["status"]="ok"
            if fee is not None:
                gross=365*live["volume_24h_usd"]/live["tvl_usd"]*fee/10000
                row["pool_gross_turnover_apr_proxy"]=gross
                row["gross_horizon_fee_proxy_10000_usdc"]=10000*gross*horizon/365
        else:row["status"]=live["status"] if live["status"]!="ok" else "spot_basis_too_large"
        pools[name]=row
        print(json.dumps({"market":name,"status":row["status"],
                          "tvl":live.get("tvl_usd"),"volume":live.get("volume_24h_usd")}),flush=True)
    report={"status":"ok","created_utc":utc(),"deribit_eth":snap,
        "horizon_days":horizon,"initial_wealth_usdc":10000,
        "unit":"each integer optimizer lot is 0.1 ETH = 0.1 Deribit ETH_USDC contract",
        "option_hedge_models":models,"ethereum_pools":pools,
        "warnings":["Uniswap V3 ranges hypothetical, no NFT fee growth.",
                    "V4 Hook/fee unverified, market discovery only.",
                    "No pooled USDC fee switch, gas costs, option fills or actual PnL."]}
    a.output.parent.mkdir(parents=True,exist_ok=True)
    a.output.write_text(json.dumps(report,indent=2)+"\n")
    print(json.dumps({"status":"ok","books":snap["depth_with_asks"],
          "models":{k:v["status"] for k,v in models.items()}}),flush=True)
if __name__=="__main__":main()
