#!/usr/bin/env python3
"""Auditable small-LP-pool fee opportunity screen, not a net-APR recommendation.

Read-only Git LFS catalogue + live public DexScreener h24 snapshots. No wallets,
secrets, trades, RPC endpoint DB, historical data backfilling or on-chain mutations.
"""
import argparse,datetime as dt,json,math,sqlite3,time
from collections import Counter,defaultdict
from pathlib import Path
from urllib.request import Request,urlopen

WSOL="So11111111111111111111111111111111111111112".lower()
SUSDC="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".lower()
SUSDT="Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".lower()
WETH="0xC02aaa39b223FE8D0a0e5C4F27eAD9083C756Cc2".lower()
NATIVE_ETH="0x0000000000000000000000000000000000000000"
EUSDC="0xA0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".lower()
EUSDT="0xdAC17F958D2ee523a2206206994597C13D831ec7".lower()
DAI="0x6B175474E89094C44Da98b954EedeAC495271d0F".lower()
STABLE_SYMBOLS={"USDC","USDT","DAI"}
MAJOR_SYMBOLS={"SOL","WSOL","ETH","WETH","WBTC","BTC"}
MAX_CANDIDATES=68
def utc():return dt.datetime.now(dt.timezone.utc)
def valid_num(n):
    try:
        x=float(n)
        return x if math.isfinite(x) else None
    except (ValueError,TypeError):return None
def category_by_catalog(row):
    a=(row["token_a_symbol"] or "").upper()
    b=(row["token_b_symbol"] or "").upper()
    if a in STABLE_SYMBOLS and b in STABLE_SYMBOLS:return "stable_stable"
    if (a in MAJOR_SYMBOLS and b in STABLE_SYMBOLS) or (b in MAJOR_SYMBOLS and a in STABLE_SYMBOLS):
        return "major_stable"
    if a in STABLE_SYMBOLS or b in STABLE_SYMBOLS:return "unmatched_token_stable"
    return None
def classify_actual(chain,tokens):
    addresses={str(z.get("address","")).lower() for z in tokens}
    if chain=="solana":
        if addresses=={WSOL,SUSDC}:return "canonical_SOL_USDC"
        if addresses=={WSOL,SUSDT}:return "canonical_SOL_USDT"
        if addresses=={SUSDC,SUSDT}:return "canonical_stable_stable_depeg_exposure"
        if SUSDC in addresses:return "solana_USDC_other_unhedgeable_or_unverified"
        return "unverified_token_identity"
    if chain=="ethereum":
        if addresses=={WETH,EUSDC}:return "canonical_WETH_USDC"
        if addresses=={WETH,EUSDT}:return "canonical_WETH_USDT"
        if addresses=={NATIVE_ETH,EUSDC}:return "canonical_NATIVE_ETH_USDC"
        if addresses=={NATIVE_ETH,EUSDT}:return "canonical_NATIVE_ETH_USDT"
        if addresses=={EUSDC,EUSDT} or addresses=={EUSDC,DAI} or addresses=={EUSDT,DAI}:
            return "canonical_stable_stable_depeg_exposure"
        if EUSDC in addresses:return "ethereum_USDC_other_unhedgeable_or_unverified"
        return "unverified_token_identity"
    return "unsupported_chain"
def lp_share(family,protocol,chain):
    p=(protocol or "").lower()
    if chain=="solana" and "raydium" in p:
        if family=="solana_standard":return .88,"ASSUMED_AMM_V4_must_verify_subtype"
        if family=="solana_clmm":return .84,"published_CLMM_share_verify_config"
    # DLMM base and variable fees require pool config; cannot apply universal split.
    if chain=="ethereum" and family=="constant_product_v2" and "uniswap" in p:
        return 5/6,"UNISWAP_V2_mainnet_protocol_share_expected_verify_pool"
    return None,"unverified_pool_fee_share"
def standardized_fee_bps(row):
    """Old catalog V4 fees are in pips (1e-6), not basis points (1e-4).
    This is a unit conversion, NOT a currently confirmed on-chain fee.
    """
    raw=valid_num(row["fee_bps"])
    if raw is None or raw<=0:return None
    return raw/100.0 if row["pool_type"]=="uniswap_v4" else raw

def old_candidates(db,min_tvl=50000,max_tvl=3000000):
    rows=db.execute("""SELECT chain,protocol,pool_type,pool_address,pair,token_a_symbol,
       token_b_symbol,liquidity_usd,volume_24h_usd,fee_bps,updated_at,lp_fee_apr_estimate
       FROM pools WHERE lower(chain) IN ('solana','ethereum') AND liquidity_usd BETWEEN ? AND ?
         AND volume_24h_usd>=10000 AND
        ((pool_type='uniswap_v4' AND fee_bps BETWEEN 1 AND 10000)
         OR (pool_type<>'uniswap_v4' AND fee_bps BETWEEN 1 AND 400))
       ORDER BY volume_24h_usd DESC""",(min_tvl,max_tvl)).fetchall()
    allpool=[];count=Counter()
    for r in rows:
        row=dict(r); cat=category_by_catalog(row)
        if cat is None:continue
        count[cat]+=1
        fee=standardized_fee_bps(row);vol=valid_num(row["volume_24h_usd"])
        tvl=valid_num(row["liquidity_usd"])
        if not fee or not vol or not tvl:continue
        row["cohort"]=cat
        row["catalog_fee_turnover_score"]=(vol/tvl)*fee/10000
        allpool.append(row)
    # Stratify old catalogue to avoid only meme small-cap rankings; diverse protocols.
    strata=defaultdict(list)
    for row in allpool:
        strata[(row["chain"].lower(),row["cohort"],row["pool_type"])].append(row)
    for group in strata.values():
        group.sort(key=lambda r:r["catalog_fee_turnover_score"],reverse=True)
    selected=[];visited=set()
    # Leave enough slots for all categories & venue families.
    for phase in range(5):
        for key,group in sorted(strata.items()):
            if phase<len(group):
                row=group[phase]
                addr=(row["chain"].lower(),row["pool_address"].lower())
                if addr not in visited:
                    selected.append(row);visited.add(addr)
                if len(selected)>=MAX_CANDIDATES:break
        if len(selected)>=MAX_CANDIDATES:break
    return selected,dict(count),len(rows)
def public_pair(chain,address):
    url="https://api.dexscreener.com/latest/dex/pairs/"+chain+"/"+address
    req=Request(url,headers={"accept":"application/json","user-agent":"arbitage-small-pools-research/0.2"})
    try:
        with urlopen(req,timeout=15) as resp:raw=json.load(resp)
        matches=[p for p in raw.get("pairs") or [] if
                 p.get("chainId","").lower()==chain and
                 str(p.get("pairAddress","")).lower()==address.lower()]
        if not matches:return {"status":"pair_not_returned"}
        p=matches[0];base=p.get("baseToken") or {};quote=p.get("quoteToken") or {}
        tvl=valid_num((p.get("liquidity") or {}).get("usd"))
        volume=valid_num((p.get("volume") or {}).get("h24"))
        if not(tvl and tvl>0 and volume is not None and volume>=0):
            return {"status":"invalid_liquidity_volume"}
        trans=p.get("txns") or {};t=trans.get("h24") or {}
        txcount=int(t.get("buys") or 0)+int(t.get("sells") or 0)
        start=p.get("pairCreatedAt")
        age_days=(utc().timestamp()-float(start)/1000)/86400 if start else None
        change=valid_num((p.get("priceChange") or {}).get("h24"))
        return {"status":"ok","source":"DexScreener public pool pair",
           "observed_utc":utc().isoformat(),"dex_id":p.get("dexId"),
           "base_symbol":base.get("symbol"),"quote_symbol":quote.get("symbol"),
           "identity":classify_actual(chain,(base,quote)),
           "tvl_usd":tvl,"volume_24h_usd":volume,
           "volume_to_tvl_daily":volume/tvl,
           "tx_count_24h":txcount,"pool_age_days":round(age_days,1) if age_days is not None else None,
           "price_change_24h_pct":change,"price_usd":valid_num(p.get("priceUsd")),
           "fee_rate_onchain_verified":False}
    except Exception as exc:return {"status":"fetch_failed","error_type":type(exc).__name__}
def evaluate(row,live,horizon=14):
    result={"chain":row["chain"],"protocol":row["protocol"],"type":row["pool_type"],
      "pool":row["pool_address"],"old_catalog_pair":row["pair"],
      "catalog_timestamp":row["updated_at"],"cohort":row["cohort"],
      "catalog_fee_field_RAW_not_verified":row["fee_bps"],
      "catalog_fee_unit_interpretation":"Uniswap V4 pips converted /100 to bps" if row["pool_type"]=="uniswap_v4" else "catalog bps",
      "catalog_fee_bps_AFTER_UNIT_CONVERSION_not_verified":standardized_fee_bps(row),
      "live":live}
    if live.get("status")!="ok":return result
    t=live["tvl_usd"];u=live["volume_24h_usd"]
    share,status=lp_share(row["pool_type"],row["protocol"],row["chain"].lower())
    fee_bps=standardized_fee_bps(row)
    gross=u*fee_bps/10000
    result["candidate_lp_share"]=share
    result["share_provenance"]=status
    result["gross_pool_fees_usdc_per_24h_if_catalog_tier_correct"]=gross
    result["gross_pool_fee_apr_proxy"]=365*gross/t
    result["apr_proxy_after_assumed_share"]=(365*gross*share/t if share is not None else None)
    result["investments"]=[]
    for W in (2000.,10000.):
        pool_share=W/(t+W)
        gross_14=horizon*gross*pool_share
        result["investments"].append({
            "capital_usdc":W,"deposit_fraction_of_post_deposit_pool":pool_share,
            "fourteen_day_gross_fee_proxy_usdc":gross_14,
            "fourteen_day_after_assumed_fee_split_usdc":gross_14*share if share is not None else None,
            "minimum_fee_capture_pct_of_pool_pro_rata_to_cover_100usd":
                (100/(gross_14*share)*100 if share is not None and gross_14>0 else None),
            "caveat":"CPMM only initial pool share proxy; CLMM/DLMM are not paid by this TVL share."
        })
    flags=[]
    age=live["pool_age_days"];change=live["price_change_24h_pct"]
    if age is None:flags.append("UNKNOWN_POOL_AGE")
    elif age<30:flags.append("YOUNG_POOL_LT_30D")
    if abs(change or 0)>=20:flags.append("PRICE_MOVE_ABS_GE_20PCT_24H")
    if live["tx_count_24h"]<60:flags.append("LESS_THAN_60_TRADES_24H")
    if live["volume_to_tvl_daily"]>10:flags.append("DAILY_VOLUME_EXCEEDS_10X_TVL_SUSPICIOUS_OR_ROUTING")
    if live["identity"].endswith("unverified") or "unhedgeable" in live["identity"]:
        flags.append("TOKEN_IDENTITY_OR_HEDGE_UNVERIFIED")
    if row["pool_type"]=="uniswap_v4":flags.append("UNISWAP_V4_HOOK_AND_CURRENT_FEE_NOT_VERIFIED")
    if live["identity"]=="canonical_stable_stable_depeg_exposure":flags.append("STABLECOIN_DEPEG_TAIL_RISK")
    if row["pool_type"] in ("solana_clmm","solana_dlmm","solana_whirlpool","concentrated_v3","uniswap_v4"):
        flags.append("CONCENTRATED_FEE_CANNOT_USE_POOL_PRO_RATA")
    if share is None:flags.append("LP_FEE_SPLIT_NOT_CONFIRMED")
    flags.append("FEE_BPS_FROM_STALE_CATALOG")
    result["risk_flags"]=flags
    result["is_valid_major_asset_pair"]=live["identity"] in (
        "canonical_SOL_USDC","canonical_SOL_USDT","canonical_WETH_USDC",
        "canonical_WETH_USDT","canonical_NATIVE_ETH_USDC","canonical_NATIVE_ETH_USDT")
    result["is_major_stable_pair"]=live["identity"]=="canonical_stable_stable_depeg_exposure"
    # Observed h24 ratio can be temporary; ranking by scenario is NOT profit sorting.
    return result
def main():
    ap=argparse.ArgumentParser()
    ap.add_argument("--catalog",type=Path,default=Path("data/lp_screener.db"))
    ap.add_argument("--output",type=Path,default=Path("research-artifacts/small_pool_fee_screen.json"))
    ap.add_argument("--min-tvl",type=float,default=50000)
    ap.add_argument("--max-tvl",type=float,default=3000000)
    ap.add_argument("--max-requests",type=int,default=68)
    a=ap.parse_args()
    if a.catalog.open("rb").read(16)!=b"SQLite format 3\x00":raise ValueError("LFS object missing; catalog pointer not DB")
    db=sqlite3.connect("file:"+str(a.catalog.resolve())+"?mode=ro",uri=True);db.row_factory=sqlite3.Row
    selected,cohorts,catalog_query_count=old_candidates(db,a.min_tvl,a.max_tvl)
    records=[]
    for row in selected[:a.max_requests]:
        chain=row["chain"].lower()
        obs=public_pair(chain,row["pool_address"])
        analyzed=evaluate(row,obs)
        records.append(analyzed)
        print(json.dumps({"category":row["cohort"],"protocol":row["protocol"],"pool":row["pool_address"],
          "live_status":obs["status"],"identity":obs.get("identity"),"volume":obs.get("volume_24h_usd"),
          "tvl":obs.get("tvl_usd"),"feeAPR":analyzed.get("apr_proxy_after_assumed_share")}),flush=True)
        time.sleep(.20)
    if not records:raise ValueError("No old catalog candidates")
    cohorts_new=Counter((z["cohort"],z["live"].get("status")) for z in records)
    def qualified(z):
        v=z["live"];return (v.get("status")=="ok" and
          a.min_tvl<=v["tvl_usd"]<=a.max_tvl and v["volume_24h_usd"]>10000)
    verified=[z for z in records if qualified(z)]
    verified.sort(key=lambda r:r.get("apr_proxy_after_assumed_share") if
                  r.get("apr_proxy_after_assumed_share") is not None else r["gross_pool_fee_apr_proxy"],
                  reverse=True)
    head=[z for z in verified if z["is_valid_major_asset_pair"]]
    stables=[z for z in verified if z["is_major_stable_pair"]]
    unhedgeable=[z for z in verified if not z["is_valid_major_asset_pair"] and not z["is_major_stable_pair"]]
    output={"status":"ok","asof_utc":utc().isoformat(),
      "scan_parameters":{"min_tvl_usd":a.min_tvl,"max_tvl_usd":a.max_tvl,
        "max_requests":a.max_requests,"horizon_days_if_volume_persists":14},
      "old_catalog_timestamp_notice":"Catalog updated_at values are NOT live pool fee tiers or identity proof.",
      "source":"Git LFS pool catalogue + current public DexScreener pool pair metrics",
      "catalog_matching_raw_rows":catalog_query_count,
      "catalog_qualified_old_category_counts":cohorts,
      "live_status_cohorts":{str(k):v for k,v in cohorts_new.items()},
      "requested_live_pools":len(records),"currently_qualified_pools":len(verified),
      "bluechip_asset_pools":head,"canonical_stable_pairs":stables,
      "exotic_or_unhedgeable":unhedgeable,
      "all_checked":records,
      "caveats":["Latest 24h turnover is not expected future volume.",
        "Catalog fee fields are OLD; Uniswap V4 source values use pips, not bps. Current dynamic Hook fee must be read on-chain.",
        "CPMM full LP fee share roughly W/(TVL+W) only when assets and accounting align.",
        "DLMM/CLMM per-position fee income depends on active price Bin/tick and active liquidity.",
        "Pool pairCreatedAt, number of swaps, and 24h price move are imperfect risk proxies.",
        "No exhaustive universe search or advertised APR claims.",
        "High-APR exotic coins have unmatched option underlying and can suffer sharp jumps, rug and nonredeemable tokens.",
        "No user LP position, future price, fillable options or trade executed."]}
    a.output.parent.mkdir(parents=True,exist_ok=True)
    a.output.write_text(json.dumps(output,indent=2)+"\n")
    print(json.dumps({"status":"ok","selected":len(records),"qualified":len(verified),
         "bluechip":len(head),"canonical_stable":len(stables),"exotic":len(unhedgeable),
         "top_majors":[{"protocol":x["protocol"],"pool":x["pool"],"pair":x["live"]["base_symbol"]+"/"+x["live"]["quote_symbol"],
                         "TVL":x["live"]["tvl_usd"],"vol":x["live"]["volume_24h_usd"],
                         "APR_assumed":x.get("apr_proxy_after_assumed_share")} for x in head[:8]],
         "top_stable":[{"pool":x["pool"],"TVL":x["live"]["tvl_usd"],"vol":x["live"]["volume_24h_usd"],
                         "gross_APR":x.get("gross_pool_fee_apr_proxy")} for x in stables[:8]],
         "top_exotic":[{"pool":x["pool"],"symbols":x["live"]["base_symbol"]+"/"+x["live"]["quote_symbol"],
                          "TVL":x["live"]["tvl_usd"],"vol":x["live"]["volume_24h_usd"],
                          "gross_APR":x.get("gross_pool_fee_apr_proxy"),
                          "risk_flags":x["risk_flags"][:3]} for x in unhedgeable[:8]]}),flush=True)
if __name__=="__main__":main()
