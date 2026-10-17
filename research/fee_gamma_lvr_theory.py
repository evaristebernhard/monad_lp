#!/usr/bin/env python3
"""Analytic fee-vs-negative-Gamma framework for CPMM and ideal passive DLMM.

Research-only deterministic input assumptions, zero external API usage, zero private data.
Never interpret pool-wide fee scenario as actual concentrated LP fee income.
"""
import argparse
import datetime as dt
import json
import math
from pathlib import Path

SQRT2 = math.sqrt(2)
W = 10_000.0
S0 = 110.25
T_DAYS = 14.0
VOL = 0.538

# Historical same-day observed pool snapshots; NOT current prices or a 14d forecast.
# Documentation supplies corresponding Actions runs and time of observation.
OBSERVATIONS = {
    "raydium_standard_SOL_USDT": dict(
        tvl=1_371_029.84, daily_volume=421_959.37287223083,
        fee=0.0025, lp_share=0.88,
        provenance="Raydium API V3 official 2026-10-09 11:11 UTC, Standard AMM V4"),
    "meteora_DLMM_1bp_SOL_USDC": dict(
        tvl=315_608.60, daily_volume=9_255_006.22,
        fee=0.0001, lp_share=None,
        provenance="DexScreener Oct 9 11:05 UTC, catalog Oct 1 BASE fee, not on-chain verified"),
    "meteora_DLMM_4bp_SOL_USDC": dict(
        tvl=4_110_808.13, daily_volume=31_134_324.04,
        fee=0.0004, lp_share=0.9,
        provenance="DexScreener Oct 9 09:28 UTC, Meteora stored pool protocol share 10%"),
    "meteora_DLMM_20bp_SOL_USDC": dict(
        tvl=2_075_395.90, daily_volume=1_911_677.21,
        fee=0.0020, lp_share=None,
        provenance="DexScreener Oct 9 11:05 UTC, catalog Oct 1 BASE fee, not on-chain verified")
}

def phi(x):
    return 0.5 * (1 + math.erf(x / SQRT2))

def bs_option(spot, strike, sigma, years, side):
    """No rates/dividends, forward = spot: option value per volatile-token unit."""
    if side not in ("put", "call") or min(spot, strike) <= 0 or sigma < 0 or years < 0:
        raise ValueError("invalid Black-Scholes inputs")
    if sigma == 0 or years == 0:
        return max(spot - strike, 0.) if side == "call" else max(strike - spot, 0.)
    d1 = (math.log(spot / strike) + 0.5 * sigma**2 * years)/(sigma*math.sqrt(years))
    d2 = d1 - sigma*math.sqrt(years)
    if side == "call":
        return spot*phi(d1)-strike*phi(d2)
    return strike*phi(-d2)-spot*phi(-d1)

def cp_loss(wealth, spot, terminal):
    if min(spot, terminal) <= 0:raise ValueError("positive prices needed")
    return wealth/2. * (math.sqrt(terminal/spot)-1)**2

def cp_expected_loss(wealth, vol, years):
    """Exactly E[D_CP] when S is a GBM martingale with constant vol."""
    if min(wealth, vol, years) < 0:raise ValueError("negative input")
    return wealth * (-math.expm1(-vol*vol*years/8.0))

def dlmm_bins(wealth, spot, step_bps, n_lower=35, n_upper=35):
    """Artificial 50:50 USD split into equidollar lower and upper bins.
    Active bin omitted. Each bin holds initial one-sided correct token.
    """
    if not(wealth > 0 and spot > 0 and step_bps > 0 and n_lower and n_upper):
        raise ValueError("invalid DLMM allocation")
    h=1+step_bps/10000.
    lower_usdc=wealth/(2*n_lower)
    upper_usdc=wealth/(2*n_upper)
    # each triple: put/call, bin strike, amount of volatile token
    lower=[("put",spot*h**(-i),lower_usdc/(spot*h**(-i)))
           for i in range(1,n_lower+1)]
    upper=[("call",spot*h**i,upper_usdc/spot)
           for i in range(1,n_upper+1)]
    return lower+upper

def dlmm_loss(bins, terminal):
    return sum(q*(max(k-terminal,0.) if side=="put" else max(terminal-k,0.))
               for side,k,q in bins)

def dlmm_expected_loss(bins,spot,vol,years):
    """Risk-neutral expected HODL-relative short convexity; NOT actual IV forecast."""
    return sum(q*bs_option(spot,k,vol,years,side) for side,k,q in bins)

def hypothetical_fee(tvl, daily_volume, fee, share, wealth=W, days=T_DAYS):
    """Only a full-range pro-rata initial LP assumption. NOT DLMM realized LP fees."""
    return days*daily_volume*fee*share*wealth/(tvl+wealth)

def fee_vs_price_survival(wealth, observed_fee_usdc):
    """Maximum price ratios with unhedged CPMM D<=future fees, constant assumptions."""
    root=math.sqrt(2*observed_fee_usdc/wealth)
    return ((1-root)**2 if root<=1 else 0.,(1+root)**2)

def allocate_fee_budget(expected_bin_fee_budgets, incumbent_bin_capital,
                        per_dollar_loss, capital):
    """Example closed-form KKT solution in frozen fee-flow price-taking regime.

    maximize sum_i A_i * w_i/(C_i+w_i) - ell_i*w_i
    s.t. w_i>=0, sum w_i<=W. Idle capital allowed.
    NOT a forecast, since true A_i(w,f,S_path) is endogenous.
    """
    A=list(map(float,expected_bin_fee_budgets))
    C=list(map(float,incumbent_bin_capital))
    ell=list(map(float,per_dollar_loss))
    if not(len(A)==len(C)==len(ell)) or capital < 0:
        raise ValueError("invalid vector size or capital")
    if any(a < 0 or c <= 0 or l <= 0 for a,c,l in zip(A,C,ell)):
        raise ValueError("A>=0 C>0 ell>0 needed")
    def at(shadow):
        return [max(0.,math.sqrt(a*c/(shadow+l))-c) if a>0 else 0.
                for a,c,l in zip(A,C,ell)]
    w=at(0.)
    if sum(w)>capital:
        lower=0.
        upper=max(max(a/c-l,0.) for a,c,l in zip(A,C,ell))+1.
        for _ in range(96):
            midpoint=(lower+upper)/2
            if sum(at(midpoint))>capital:lower=midpoint
            else:upper=midpoint
        w=at(upper)
        shadow=upper
    else:shadow=0.
    return dict(allocations_usdc=w,idle_cash_usdc=max(0.,capital-sum(w)),
                shadow_price=shadow,
                expected_net_fee_minus_loss=sum(a*wi/(ci+wi)-li*wi
                    for a,ci,li,wi in zip(A,C,ell,w)))

def stress(wealth=W,spot=S0,vol=VOL,days=T_DAYS):
    years=days/365
    cp=cp_expected_loss(wealth,vol,years)
    scenarios={}
    for step in (1,4,20):
        bins=dlmm_bins(wealth,spot,step)
        expected=dlmm_expected_loss(bins,spot,vol,years)
        worst=max(dlmm_loss(bins,spot*.9),dlmm_loss(bins,spot*1.1))
        scenarios[f"dlmm_{step}bps_70bins"]=dict(
            expected_hodl_relative_loss_usdc=expected,
            downside_10pct_loss=dlmm_loss(bins,spot*.9),
            upside_10pct_loss=dlmm_loss(bins,spot*1.1),
            worst_loss_pm10pct=worst,
            full_range_span_pct=100*((1+step/10000.)**70-1),
            position_model="synthetic 35 bins USDC below + 35 bins SOL above")
    fee_scenarios={}
    for name,ob in OBSERVATIONS.items():
        share=ob["lp_share"]
        gross=hypothetical_fee(ob["tvl"],ob["daily_volume"],ob["fee"],1.,wealth,days)
        fee_scenarios[name]=dict(
            input_provenance=ob["provenance"],dated_tvl_usd=ob["tvl"],
            dated_volume24h_usd=ob["daily_volume"],assumed_fee_fraction=ob["fee"],
            lp_share_known_or_assumed=share,
            gross_pro_rata_14d_hypothetical_usdc=gross,
            after_share_hypothetical_usdc=(
                hypothetical_fee(ob["tvl"],ob["daily_volume"],ob["fee"],share,wealth,days)
                if share is not None else None),
            warning="NOT a concentrated position fee estimate; 24h turnover extrapolation is hypothetical")
    cp_fee=fee_scenarios["raydium_standard_SOL_USDT"]["after_share_hypothetical_usdc"]
    low,high=fee_vs_price_survival(wealth,cp_fee)
    return dict(asof_utc=dt.datetime.now(dt.timezone.utc).isoformat(),
         model="No-rate, driftless risk-neutral GBM; constant sigma ASSUMPTION not realized volatility",
         wealth_usdc=wealth,entry_spot=spot,annualized_sigma_assumption=vol,
         expiry_days=days,
         expected_HODL_relative_loss=dict(cpmm=cp,**scenarios),
         dated_pool_volume_fee_sensitivity=fee_scenarios,
         raydium_standard_conditional_metrics=dict(
             hypothetical_14d_fees_usdc=cp_fee,
             cpmm_riskneutral_expected_HODL_relative_loss_usdc=cp,
             fragile_fee_minus_RN_expected_loss_usdc=cp_fee-cp,
             terminal_price_ratio_down_to_cover_loss=low,
             terminal_price_ratio_up_to_cover_loss=high),
         example_KKT_allocation=allocate_fee_budget(
             expected_bin_fee_budgets=[50,120,80],
             incumbent_bin_capital=[1000,2000,3000],
             per_dollar_loss=[.012,.025,.018],capital=2000),
         warning="No future expected earned fee revenue, no actual PositionV2, fee-induced routing and inventory-path changes omitted.")

def selftest():
    assert abs(bs_option(100,108,.5,.04,"call")-
               bs_option(100,108,.5,.04,"put")-(100-108))<1e-8
    b=dlmm_bins(10000,100,4)
    assert len(b)==70
    assert abs(dlmm_loss(b,100))<1e-10
    assert all(dlmm_loss(b,p)>=0 for p in (60,85,99,100,101,115,140))
    # Relative-to-HODL fee-free expected loss equals fractional Bin option price:
    a=dlmm_expected_loss(b,100,.538,14/365)
    assert 0<a<1000
    assert dlmm_expected_loss(dlmm_bins(10000,100,1),100,.538,14/365)>a
    assert dlmm_expected_loss(dlmm_bins(10000,100,20),100,.538,14/365)<a
    # Independent numerical standard-normal expectation verifies CPMM closed form.
    sigma=.538;T=14/365;zmin=-8.;dz=16/32000;total=0.
    for i in range(32001):
        z=zmin+i*dz
        terminal=100*math.exp(-.5*sigma*sigma*T+sigma*math.sqrt(T)*z)
        dens=math.exp(-z*z/2)/math.sqrt(2*math.pi)
        total+=((.5 if i in (0,32000) else 1)*dens*cp_loss(10000,100,terminal)*dz)
    assert abs(total-cp_expected_loss(10000,sigma,T))<1e-5
    ex=allocate_fee_budget([50,120,80],[1000,2000,3000],[.012,.025,.018],2000)
    assert sum(ex["allocations_usdc"])<=2000+1e-5
    for wi in ex["allocations_usdc"]:
        assert wi>=0
    lo,hi=fee_vs_price_survival(10000,94.)
    assert lo<1<hi
    print("PASS: put-call parity, DLMM bin convexity/step, CPMM GBM expectation, KKT budget, break-even bounds")

def main():
    p=argparse.ArgumentParser()
    p.add_argument("--self-test",action="store_true")
    p.add_argument("--output",type=Path,default=Path("research-artifacts/fee_gamma_lvr_theory.json"))
    a=p.parse_args()
    if a.self_test:return selftest()
    r=stress()
    a.output.parent.mkdir(parents=True,exist_ok=True)
    a.output.write_text(json.dumps(r,indent=2)+"\n")
    cp=r["expected_HODL_relative_loss"]
    fs=r["dated_pool_volume_fee_sensitivity"]
    print(json.dumps(dict(status="ok",model=r["model"],
       cpmm_RN_expected_IL=cp["cpmm"],
       bin_step_expected_IL={s:{k:v for k,v in cp[f"dlmm_{s}bps_70bins"].items() if
           k in ("expected_hodl_relative_loss_usdc","worst_loss_pm10pct","full_range_span_pct")} for s in (1,4,20)},
       pool_fee_reference={k:v["after_share_hypothetical_usdc"] or v["gross_pro_rata_14d_hypothetical_usdc"]
                           for k,v in fs.items()},
       CPMM_fee_and_expected_IL=r["raydium_standard_conditional_metrics"],
       illustrative_KKT=r["example_KKT_allocation"]),indent=2))

if __name__=="__main__":main()
