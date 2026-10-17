#!/usr/bin/env python3
"""Low-maintenance Solana full-range CPMM screen using Raydium verified 7d/30d fees.

READ-ONLY Git LFS pool catalog to discover; real Raydium official public API
to verify canonical mint, AMM V4 program, feeRate, 24h/7d/30d volumes AND
volumeFee. No RPC.db/secrets, no orders, no wallets.

30d/7d trailing fees are actual POOL fee aggregates as reported by provider;
post-deposit fee capture and future expected fees are NOT historical user PnL.
"""
import argparse
import datetime as dt
import json
import math
import sqlite3
import time
from pathlib import Path
from urllib.parse import urlencode
from urllib.request import Request,urlopen

WSOL="So11111111111111111111111111111111111111112"
STABLES={
    "USDC":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    "USDT":"Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"}
RAYDIUM_AMM_V4_PROGRAM="675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"
CONTROLS=[
    "7XawhbbxtsRcQA8KTkHT9f9nc6d69UwqCDh6U5EEbEmX",
    "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2"]
FEE_LP_SHARE=.88
MAX_CANDIDATES=72

def now():return dt.datetime.now(dt.timezone.utc).isoformat()
def finite(x):
    try:
        v=float(x)
        return v if math.isfinite(v) else None
    except (ValueError,TypeError):return None

def cpmm_divergence(W,r):
    """Loss vs same initial basket, NOT USD account mark-to-market loss."""
    if W<=0 or r<=0:raise ValueError("W and terminal price ratio must be positive")
    return .5*W*(math.sqrt(r)-1)**2

def cpmm_rn_loss(W,annual_vol,horizon_days):
    """Risk-neutral GBM short-convexity price, not physical loss forecast."""
    if min(W,annual_vol,horizon_days)<0:raise ValueError("non-negative inputs")
    return W*(-math.expm1(-annual_vol**2*horizon_days/(365*8)))

def cpmm_break_even_range(W,fee_budget):
    if W<=0 or fee_budget<0:raise ValueError("invalid input")
    a=math.sqrt(2*fee_budget/W)
    return [max(0.,1-a)**2,(1+a)**2] if a<=1 else [0.,(1+a)**2]

def candidate_catalog(path,limit=MAX_CANDIDATES):
    p=Path(path)
    if p.open("rb").read(16)!=b"SQLite format 3\x00":raise ValueError("real Git LFS SQLite required")
    db=sqlite3.connect("file:"+str(p.resolve())+"?mode=ro",uri=True)
    db.row_factory=sqlite3.Row
    rows=db.execute("""SELECT pool_address,protocol,pool_type,chain,pair,token_a_symbol,
          token_b_symbol,liquidity_usd,volume_24h_usd,updated_at
        FROM pools WHERE lower(chain)='solana' AND pool_type='solana_standard'
          AND liquidity_usd BETWEEN 25000 AND 20000000
          AND volume_24h_usd>1000
          AND ((upper(token_a_symbol) IN ('SOL','WSOL') AND upper(token_b_symbol) IN ('USDC','USDT'))
            OR (upper(token_b_symbol) IN ('SOL','WSOL') AND upper(token_a_symbol) IN ('USDC','USDT')))
        ORDER BY (volume_24h_usd/liquidity_usd) DESC LIMIT 240""").fetchall()
    db.close()
    # Blend high volume, larger/older credible reserves, and prior verified pool IDs.
    selected=list(CONTROLS);seen=set(selected)
    candidates=[dict(r) for r in rows]
    high_by_turnover=sorted(candidates,key=lambda r:r["volume_24h_usd"]/max(1,r["liquidity_usd"]),reverse=True)
    high_by_liquidity=sorted(candidates,key=lambda r:r["liquidity_usd"],reverse=True)
    high_by_volume=sorted(candidates,key=lambda r:r["volume_24h_usd"],reverse=True)
    for i in range(90):
        for group in (high_by_turnover,high_by_liquidity,high_by_volume):
            if i>=len(group):continue
            addr=group[i]["pool_address"]
            if addr not in seen:
                selected.append(addr);seen.add(addr)
            if len(selected)>=limit:break
        if len(selected)>=limit:break
    return selected[:limit],len(rows)

def official_batch(ids):
    url="https://api-v3.raydium.io/pools/info/ids?"+urlencode({"ids":",".join(ids)})
    req=Request(url,headers={"accept":"application/json","user-agent":"arbitage-fullrange-7d30d-research/1.0"})
    with urlopen(req,timeout=22) as response:payload=json.load(response)
    if not payload.get("success"):raise RuntimeError("official_raydium_api_unsuccessful")
    result=payload.get("data")
    if isinstance(result,dict):result=result.get("data",result.get("list"))
    if not isinstance(result,list):raise ValueError("official_api_bad_shape")
    return result

def official_discover_pair(limit=100):
    """Supplement stale LFS catalog with official mint-pair list sorted by 30d fees.

    Pool-type=standard includes both AMM V4 and CPMM; downstream verifies
    the exact legacy program ID, so no false equivalence is introduced.
    """
    ids=[];errors=[];raw_counts={}
    for symbol,mint in STABLES.items():
        for ordering in ("fee30d","liquidity"):
            params={"mint1":WSOL,"mint2":mint,"poolType":"standard",
              "poolSortField":ordering,"sortType":"desc","pageSize":limit,"page":1}
            url="https://api-v3.raydium.io/pools/info/mint?"+urlencode(params)
            try:
                req=Request(url,headers={"accept":"application/json",
                    "user-agent":"arbitage-fullrange-7d30d-research/1.0"})
                with urlopen(req,timeout=22) as response:payload=json.load(response)
                if not payload.get("success"):raise RuntimeError("mint_pair_query_failed")
                raw=payload.get("data")
                if isinstance(raw,dict):raw=raw.get("data",raw.get("list"))
                if not isinstance(raw,list):raise ValueError("mint_pair_list_bad_shape")
                raw_counts[symbol+"_"+ordering]=len(raw)
                for item in raw:
                    if item.get("programId")==RAYDIUM_AMM_V4_PROGRAM:
                        ids.append(item["id"])
            except Exception as exc:
                errors.append({"pair":symbol,"sort":ordering,"error_type":type(exc).__name__})
            time.sleep(.15)
    return list(dict.fromkeys(ids)),raw_counts,errors

def read_stats(d):
    if not isinstance(d,dict):return None
    vol=finite(d.get("volume"));fee=finite(d.get("volumeFee"))
    if vol is None or fee is None or min(vol,fee)<0:return None
    return {"volume_usd":vol,"gross_swap_fees_usd":fee,
       "official_fee_apr_value":finite(d.get("feeApr")),
       "official_tvl_apr_value":finite(d.get("apr"))}

def parse_pool(p):
    pool=p.get("id","")
    base=(p.get("mintA") or {}).get("address")
    quote=(p.get("mintB") or {}).get("address")
    pairset={base,quote}
    token=next((k for k,v in STABLES.items() if pairset=={WSOL,v}),None)
    typ=p.get("type");program=p.get("programId")
    out={"pool":pool,"status":"excluded",
         "official_type":typ,"program_id":program,
         "canonical_underlying_pair":token and "SOL/"+token,
         "identity_checked_mints":bool(token)}
    if typ!="Standard" or program!=RAYDIUM_AMM_V4_PROGRAM or token is None:
        out["exclude_reason"]="not_amm_v4_canonical_SOL_stable"
        return out
    tvl=finite(p.get("tvl"));rate=finite(p.get("feeRate"))
    if tvl is None or tvl<=0 or rate is None or not 0<rate<.01:
        out["exclude_reason"]="missing_tvl_or_fee_rate";return out
    periods={}
    for name in ("day","week","month"):
        stats=read_stats(p.get(name))
        if stats is None:
            out["exclude_reason"]="missing_"+name+"_volume_or_fee"
            return out
        # Explicit consistency check: do not silently infer which API value is LP revenue.
        implied=stats["volume_usd"]*rate
        if stats["volume_usd"]>0 and abs(stats["gross_swap_fees_usd"]-implied)>max(2.,implied*.15):
            out["exclude_reason"]="volumeFee_vs_volume_times_pool_fee_mismatch"
            out["fee_consistency_failed"]=name;return out
        periods[name]=stats
    opened=finite(p.get("openTime"));age=None
    if opened is not None and opened>0:
        if opened>1e12:opened=opened/1000
        age=(dt.datetime.now(dt.timezone.utc).timestamp()-opened)/86400
    price=finite(p.get("price"))
    liquidity_amnt=finite(p.get("lpAmount"))
    out.update(status="ok",current_tvl_usd=tvl,current_pool_price_quote_raw=price,
       open_age_days=round(age,2) if age is not None else None,
       fee_rate_fraction_verified_by_provider=rate,lp_share_assumed_from_official_docs=FEE_LP_SHARE,
       official_fee_totals=periods,
       lp_supply_reported=liquidity_amnt,
       note="Official gross swap fee sum is NOT fees earned by a specific LP position")
    return out

def economics(pool,wealth=10000.,horizon=14.,costs=0.):
    tvl=pool["current_tvl_usd"];f=FEE_LP_SHARE
    # Fee fraction assumes instantaneous proportion, same as LP minted proportional
    # to pool TVL. Actual LP minting, changing TVL, fee compounding not modeled.
    share=wealth/(tvl+wealth)
    fees=pool["official_fee_totals"]
    day_1=fees["day"]["gross_swap_fees_usd"]
    week_7=fees["week"]["gross_swap_fees_usd"]
    month_30=fees["month"]["gross_swap_fees_usd"]
    rate_daily={"last24h":day_1,"trailing7d_daily_mean":week_7/7,
       "trailing30d_daily_mean":month_30/30}
    fee_budgets={k:rate*share*f*horizon for k,rate in rate_daily.items()}
    # The time-series windows overlap; they are NOT 30 independent observations.
    week_over_month=(week_7/7)/(month_30/30) if month_30>0 else None
    day_over_month=day_1/(month_30/30) if month_30>0 else None
    provider_30d_fee_apr_gross=month_30/tvl*365/30 if tvl>0 else None
    snapshots={"fullrange_initial_LP_fraction_after_deposit":share,
      "source":"provider 24h/7d/30d ACTUAL pool gross fees, hypothetical LP share fixed with current TVL",
      "fee_usdc_horizon_scenarios":fee_budgets,
      "last_day_to_trailing30d_ratio":day_over_month,
      "trailing7d_to_trailing30d_ratio":week_over_month,
      "annualized_30d_current_tvl_gross_fee_run_rate":provider_30d_fee_apr_gross,
      "annualized_30d_current_tvl_after_lp_share_run_rate":provider_30d_fee_apr_gross*f,
      "vol_sensitivity":{"30pct":cpmm_rn_loss(wealth,.30,horizon),
                         "55pct":cpmm_rn_loss(wealth,.55,horizon),
                         "90pct":cpmm_rn_loss(wealth,.90,horizon)},
      "terminal_stress":{
         label:dict(terminal_price_ratio=r,divergence_vs_HODL=cpmm_divergence(wealth,r),
             cpmm_terminal_wealth_no_fees=wealth*math.sqrt(r),
             buy_hold_terminal_wealth=wealth*(1+r)/2,
             directional_cash_drawdown_no_fees=wealth*(1-math.sqrt(r)))
         for label,r in (("down10pct",.90),("down25pct",.75),("down50pct",.50),
                         ("up10pct",1.10),("up25pct",1.25),("up50pct",1.50))},
      "fee_minus_gamma_cost_sensitivity":{
         window:{vol_label:fee_value-loss-costs
           for vol_label,loss in (("30pct",cpmm_rn_loss(wealth,.30,horizon)),
                                  ("55pct",cpmm_rn_loss(wealth,.55,horizon)),
                                  ("90pct",cpmm_rn_loss(wealth,.90,horizon)))}
         for window,fee_value in fee_budgets.items()},
      "down_up_terminal_ratio_where_assumed_30d_mean_fees_cover_divergence":
          cpmm_break_even_range(wealth,fee_budgets["trailing30d_daily_mean"]),
      "excluded_costs":"gas rent account creation liquidity imbalance slippage swaps compounding token basis volatility credit",
      "warning":"Scenario fees are NOT realized or forecast LP profits; volatility risk-neutral prices NOT physical expected loss"}
    return snapshots

def evaluate(pool,wealth,horizon):
    pool["scenario"]=economics(pool,wealth,horizon)
    pool["capital_sensitivity_usdc"]={}
    for capital in (2000.,10000.):
        capital_econ=economics(pool,capital,horizon)
        pool["capital_sensitivity_usdc"][str(int(capital))]={
          "initial_postdeposit_pool_share":capital_econ["fullrange_initial_LP_fraction_after_deposit"],
          "14d_fee_if_7d_avg_persists":capital_econ["fee_usdc_horizon_scenarios"]["trailing7d_daily_mean"],
          "14d_fee_if_30d_avg_persists":capital_econ["fee_usdc_horizon_scenarios"]["trailing30d_daily_mean"],
          "riskneutral_55pct_vol_short_gamma_14d":cpmm_rn_loss(capital,.55,horizon),
          "terminal_down25pct_HODL_relative_divergence":cpmm_divergence(capital,.75),
          "terminal_down25pct_USD_wealth_no_fees":capital*math.sqrt(.75),
          "note":"All modelled investments; no real minted LP shares or future fee guarantee."}
    fees=pool["scenario"]["fee_usdc_horizon_scenarios"]
    stable=pool["scenario"]["trailing7d_to_trailing30d_ratio"]
    pool["quality_filters"]={
       "pool_age_30d_confirmed":pool["open_age_days"] is not None and pool["open_age_days"]>=30,
       "current_tvl_250k_to_5m":250000<=pool["current_tvl_usd"]<=5000000,
       "deposit_less_than_2pct_tvl":wealth/pool["current_tvl_usd"]<=.02,
       "seven_day_fees_at_least_half_monthly_daily_mean":stable is not None and stable>=.5,
       "14d_30d_fee_proxy_more_than_55pct_rn_short_gamma":fees["trailing30d_daily_mean"]>
          cpmm_rn_loss(wealth,.55,horizon)}
    pool["age_must_be_verified_separately"]=pool["open_age_days"] is None
    pool["eligible_for_further_onchain_audit"]=all(pool["quality_filters"].values())
    pool["priority_for_onchain_age_audit"]=all(v for k,v in pool["quality_filters"].items()
        if k!="pool_age_30d_confirmed")
    # Not a claim of good return or safety: only prioritization for more on-chain auditing.
    return pool

def run(a):
    ids,old_ct=candidate_catalog(a.catalog,a.limit)
    discovered,discovery_counts,discovery_errors=official_discover_pair()
    ids=list(dict.fromkeys(CONTROLS+discovered+ids))[:a.limit]
    successes={};failed_batches=[]
    for index in range(0,len(ids),a.batch_size):
        group=ids[index:index+a.batch_size]
        try:
            fetched=official_batch(group)
            for item in fetched:
                if item.get("id") in group:successes[item["id"]]=item
        except Exception as exc:
            failed_batches.append({"batch_index":index//a.batch_size,
                "error_type":type(exc).__name__})
        time.sleep(.25)
    results=[]
    for id in ids:
        if id not in successes:
            results.append({"pool":id,"status":"official_pool_not_returned"});continue
        row=parse_pool(successes[id])
        if row["status"]=="ok":evaluate(row,a.wealth,a.horizon)
        results.append(row)
    valid=[v for v in results if v["status"]=="ok"]
    valid.sort(key=lambda r:r["scenario"]["fee_usdc_horizon_scenarios"]["trailing30d_daily_mean"],reverse=True)
    shortlisted=[x for x in valid if x["eligible_for_further_onchain_audit"]]
    pending_age=[x for x in valid if x["priority_for_onchain_age_audit"]
                 and not x["eligible_for_further_onchain_audit"]]
    report={"status":"ok" if len(valid)>=2 else "insufficient_verified_pools",
        "sampled_utc":now(),"source":"Raydium official API v3 /pools/info/ids; Git LFS discovery",
        "official_lookbacks":"day/week/month are rolling aggregate metrics; 7d and 30d overlap",
        "age_and_tvl_are_current_only":True,
        "catalog_candidate_count":old_ct,"official_mint_pair_list_counts":discovery_counts,
        "official_mint_pair_discovery_errors":discovery_errors,"official_queries":len(ids),
        "api_failed_batches":failed_batches,"verified_amm_v4_pools":len(valid),
        "strategy_considered":"unhedged full-range Raydium AMM V4 SOL/USDC or SOL/USDT",
        "wealth":a.wealth,"horizon_days":a.horizon,
        "screened_pools":results,
        "ranked_verified":valid,
        "further_onchain_audit_shortlist":[r["pool"] for r in shortlisted],
        "passes_economic_tests_but_needs_age_audit":[r["pool"] for r in pending_age],
        "caveats":[
          "This is a forward scenario, NOT a 7d/30d LP backtest: pool TVL/supply over prior periods is absent.",
          "LP fees assumed 88% AMM V4 share; verify specific on-chain AmmInfo fee config.",
          "Forward fees estimated by replaying trailing fee-per-day *present entry share*; no volume persistence guaranteed.",
          "No compounding, IL due to fee reinvestment, liquidity minting, account exit costs or timing execution.",
          "Past gross fees could contain incentivized or wash flows; verified canonical mints reduce but do not eliminate risk.",
          "A 55% annualized risk-neutral GBM convexity price is not a physical risk forecast or tail bound.",
          "Relative-to-HODL profitability can coexist with large USD capital loss if SOL crashes.",
          "A 30d aggregate fee is not 30 daily observations; it cannot establish day-by-day consistency."]}
    p=Path(a.out);p.parent.mkdir(parents=True,exist_ok=True)
    p.write_text(json.dumps(report,indent=2)+"\n")
    print(json.dumps({"status":report["status"],"checked":len(ids),
      "verified":len(valid),"eligible":len(shortlisted),
      "pending_age_audit":len(pending_age),
      "pair_list_counts":discovery_counts,"pair_discovery_errors":discovery_errors,
      "api_failed_batches":failed_batches,
      "shortlist":shortlisted and [dict(pool=r["pool"],pair=r["canonical_underlying_pair"],
        tvl=round(r["current_tvl_usd"]),month_fee=round(r["official_fee_totals"]["month"]["gross_swap_fees_usd"],2),
        week_to_month=round(r["scenario"]["trailing7d_to_trailing30d_ratio"],3),
        projected_14d_fees_usdc=round(r["scenario"]["fee_usdc_horizon_scenarios"]["trailing30d_daily_mean"],2),
        proxy_fee_minus_RN_Gamma_55pct=round(r["scenario"]["fee_minus_gamma_cost_sensitivity"]["trailing30d_daily_mean"]["55pct"],2))
        for r in shortlisted[:12]],
      "top_current_verified":[dict(pool=v["pool"],pair=v["canonical_underlying_pair"],
         tvl=round(v["current_tvl_usd"]),age=v["open_age_days"],
         m30=round(v["official_fee_totals"]["month"]["gross_swap_fees_usd"]),
         seven_thirty=round(v["scenario"]["trailing7d_to_trailing30d_ratio"],2),
         fees14d10k=round(v["scenario"]["fee_usdc_horizon_scenarios"]["trailing30d_daily_mean"],2),
         fees14d10k_recent7d=round(v["scenario"]["fee_usdc_horizon_scenarios"]["trailing7d_daily_mean"],2),
         fees14d2k_recent7d=round(v["capital_sensitivity_usdc"]["2000"]["14d_fee_if_7d_avg_persists"],2),
         pending_age=v.get("age_must_be_verified_separately"),
         filter_pass_excl_age=v.get("priority_for_onchain_age_audit"))
         for v in valid[:21]],
      "controls":[dict(pool=v["pool"],status=v["status"],
        reasons=v.get("quality_filters"),age_days=v.get("open_age_days"),
        tvl=v.get("current_tvl_usd"),day_fee=v.get("official_fee_totals",{}).get("day",{}).get("gross_swap_fees_usd"),month_fee=v.get("official_fee_totals",{}).get("month",{}).get("gross_swap_fees_usd"),
        fee14=v.get("scenario",{}).get("fee_usdc_horizon_scenarios",{}).get("trailing30d_daily_mean"),
        ratio=v.get("scenario",{}).get("trailing7d_to_trailing30d_ratio"))
        for v in results if v["pool"] in CONTROLS]},ensure_ascii=False),flush=True)
    return 0 if len(valid)>=2 else 2

def selftest():
    assert abs(cpmm_divergence(10000,.5)-428.9321881345)<.00001
    assert abs(cpmm_divergence(10000,1)-0)<1e-10
    assert cpmm_rn_loss(10000,.55,14)>0
    assert math.isclose(cpmm_rn_loss(10000,.55,14),10000*(1-math.exp(-.55**2*14/365/8)))
    f=cpmm_break_even_range(10000,95)
    assert cpmm_divergence(10000,f[0])<=95+1e-8
    assert cpmm_divergence(10000,f[1])<=95+1e-8
    def mock():
        return dict(id="example",type="Standard",programId=RAYDIUM_AMM_V4_PROGRAM,
          mintA=dict(address=WSOL),mintB=dict(address=STABLES["USDC"]),tvl=1000000,
          feeRate=.0025,openTime=int(time.time()-86400*100),
          day=dict(volume=100000,volumeFee=250),
          week=dict(volume=700000,volumeFee=1750),
          month=dict(volume=3000000,volumeFee=7500))
    p=parse_pool(mock());assert p["status"]=="ok"
    e=economics(p,10000,14);assert e["trailing7d_to_trailing30d_ratio"]==1
    assert e["fee_usdc_horizon_scenarios"]["trailing30d_daily_mean"]>0
    bad=mock();bad["programId"]="untrusted";assert parse_pool(bad)["status"]=="excluded"
    bad=mock();bad["month"]["volumeFee"]=999999;assert parse_pool(bad)["status"]=="excluded"
    bad=mock();bad["mintB"]["address"]="fakeusdc";assert parse_pool(bad)["status"]=="excluded"
    print("PASS CPMM convexity, no-hedge stress, verified mints/program, 7d/30d fee units and no fake totals")

def main():
    p=argparse.ArgumentParser()
    p.add_argument("--catalog",type=Path,default=Path("data/lp_screener.db"))
    p.add_argument("--out",type=Path,default=Path("research-artifacts/cpmm_7d30d_fullrange.json"))
    p.add_argument("--limit",type=int,default=MAX_CANDIDATES)
    p.add_argument("--batch-size",type=int,default=8)
    p.add_argument("--wealth",type=float,default=10000.)
    p.add_argument("--horizon",type=float,default=14.)
    p.add_argument("--self-test",action="store_true")
    a=p.parse_args()
    if a.self_test:return selftest()
    if not 2<=a.limit<=100 or not 1<=a.batch_size<=10 or a.wealth<=0 or a.horizon<=0:
        raise ValueError("invalid configuration")
    return run(a)
if __name__=="__main__":raise SystemExit(main() or 0)
