# Fee-versus-Gamma economics of CPMM, CLMM, and Meteora DLMM

**Mathematical research note, October 9, 2026.** No trades, actual user Positions, realized LP fees, return predictions or historical backtests. Independently tested pure-Python formulas: [fee_gamma_lvr_theory.py](../research/fee_gamma_lvr_theory.py) and [successful Actions run 37924724772](https://github.com/evaristebernhard/arbitage/actions/runs/37924724772).

## 1. What are we optimizing?

For a fee-free, idealized passive LP with initial volatile-token inventory x0, initial spot S0, initial wealth W, and final spot S, define the initial HODL benchmark:

    B(S)=W+x0*(S-S0).

The spot-marked fee-free AMM value V(S) is a concave function when the pool implements a conventional negative-Gamma market maker. Its HODL-relative loss is D(S)=B(S)-V(S). A simplified terminal *relative-to-HODL* result is:

    excess P&L = realized fee cash flows F_T - D(S_T) - entry/withdrawal/rebalance costs.

Actual fee accrual changes reserves, and real pool price can diverge from external S: this decomposition is an *approximation*, not an accounting identity for all swap paths. Crucially, D is **NOT** the maximum USD capital loss: the volatile token price itself can collapse.

Pool-wide fee APR derived from daily turnover is not a real CLMM/DLMM Position's fee yield; individual active liquidity shares and local trade flows are missing.

## 2. CPMM has smooth Gamma and an analytic expected loss

For 50/50 SOL-stable initial assets and fee-free constant product x*y=k, relative price r=S/S0:

    V_CP(S)=W*sqrt(r)
    B(S)=W*(1+r)/2
    D_CP(S)=W/2*(sqrt(r)-1)^2.

Assume a **hypothetical** zero-interest risk-neutral, driftless geometric Brownian martingale dS=sigma*S*dB (not a physical price forecast). The exact risk-neutral expected divergence is:

    E_Q[D_CP(S_T)] = W*(1-exp(-sigma^2*T/8)).

Proof: E[r]=1 and E[sqrt(r)]=exp(-sigma^2*T/8).

For a continuous fee-free, perfectly arbitraged CPMM, V''(S)=-V(S)/(4*S^2). Its infinitesimal loss-versus-continuous-rebalancing (LVR) is:

    dLVR=-(1/2)*V''(S)*d<S> = V(S)*sigma^2/8*dt.

LVR and initial-HODL divergence are different **pathwise** (a round-trip in S can leave D=0 yet LVR accumulated), even though their expectations agree in this special martingale case. Transaction fees reduce arbitrage opportunities; do not deduct frictionless LVR unaltered from actual fee-bearing pool profits. See [Milionis et al., LVR](https://arxiv.org/abs/2208.06046) and [Milionis et al., arbitrage with fees](https://arxiv.org/abs/2305.14604).

## 3. DLMM Gamma is a finite set of atoms

For an ideal, STATIC, full-conversion DLMM basket: lower-price Bin K_i<S0 starts with user-owned Y_i stable tokens; if crossed downward, converts to volatile token quantity q_i=Y_i/K_i. Upper-price Bin K_i>S0 starts with X_i volatile tokens; if crossed upward, converts to stable at that fixed price, with q_i=X_i. The mixed active Bin is excluded here.

Then exactly under this construction,

    D_DLMM(S)=sum_lower q_i*(K_i-S)^+ + sum_upper q_i*(S-K_i)^+.

Both D and V=B-D are piecewise linear, with **distributional** negative Gamma:

    -V''(S)=sum_i q_i * DiracDelta(S-K_i).

This yields an Ito–Tanaka / local-time formula for the ideal frictionless passive LP, normalized so d(S-K)^+ includes (1/2)dL^K:

    LVR_DLMM(T) = (1/2)*sum_i q_i*L_T^Ki(S).

Here local time is weighted path occupation at each strike/bin in quadratic-variation time: it is NOT actual swap count. Real variable fees, asynchronous oracle/pool prices, jumps, partial fills, updates, arbitrage inhibition and position rebalancing require a separate pathwise model.

Under a risk-neutral GBM, expected ideal DLMM divergence has the exact vanilla option representation:

    E_Q[D_DLMM(S_T)] =
      sum_lower (Y_i/K_i)*Black_put(S0,K_i,sigma,T)
      + sum_upper X_i*Black_call(S0,K_i,sigma,T)
      = (1/2)*sum_i q_i E_Q[L_T^Ki(S)].

This is the core reason why purchasing options to fully insure concentrated passive liquidity can consume most of its fees: the market price of the insurance is the risk-neutral price of **the very short convexity supplied by the LP**. Thus full insurance is not a magical free-profit source.

## 4. Numerical example: fixed $10k and 70 funded Bins is not fixed price coverage

Research calculator assumptions: W=$10,000, S0=$110.25, risk-neutral GBM vol sigma=53.8% annual, T=14/365 years, 35 equal-dollar USDC Bin allocations just below spot and 35 equal-dollar SOL allocations just above, excluding the active Bin. The 53.8% is an old indicative option-implied-volatility input from prior research, NOT physical expected future volatility.

| Idealized position | Approx full price span from 70 Bin steps | E_Q divergence over 14 days | Worst divergence for terminal price +/-10% |
| --- | ---: | ---: | ---: |
| Full-range CPMM | All positive prices | **$13.87** | **$13.17** |
| DLMM 1 bp Bin Step | 0.702% | **$411.60** | $491.89 |
| DLMM 4 bps Bin Step | 2.839% | **$386.80** | $467.45 |
| DLMM 20 bps Bin Step | 15.011% | **$276.04** | $334.27 |

Each DLMM row uses a different **price range**, so lower convexity cost for the 20-bps row does NOT establish that high fees reduce Gamma. It follows from wider distribution of initial capital. Comparing equal PRICE intervals instead of equal Bin counts requires a second experiment.

These are mathematical payoff values, not historical profits and not actual Deribit ask fills. The expected-loss values are risk-neutral option values, not real-world probabilities.

## 5. Same-day fee observations, shown as conditional thought experiments ONLY

For a new full-range pro-rata deposit W into a pre-deposit pool TVL P with future volume U per day, fee tier f, LP share alpha:

    F_ref(14d) = 14*U*f*alpha*W/(P+W).

**This does not compute a CLMM/DLMM user Position fee return.** The entire exercise assumes today's single 24h flow remains unchanged for 14 days, which is unverified. Provenance is the [Raydium official pool API audit](small-pool-fees-vs-option-insurance-20261009.md) and dated original research snapshots.

| Pool | Fee and share input quality | $10k, 14-day hypothetical whole-pool pro-rata fees |
| --- | --- | ---: |
| Raydium AMM V4 SOL/USDT 7Xaw... | Official live 25 bps, published 88% share, $421,959 24h volume, ~$1.371m TVL | **$94.11** |
| Meteora 1-bp SOL/USDC HTvj... | Old catalog 1 bp, unknown share, $9.255m 24h volume, $315.6k TVL | **$397.93 GROSS** |
| Meteora 4-bps SOL/USDC 5rCf... | Saved chain-account 4-bps base, 90% share; $31.134m 24h volume, $4.111m TVL | **$380.79 after share** |
| Meteora 20-bps SOL/USDC BVR... | Old catalog 20 bps, unknown share; $1.912m 24h volume, $2.075m TVL | **$256.68 GROSS** |

The comparison of the mathematical 4-bps 70-Bin short convexity at **$386.80** with its entirely hypothetical pool-wide $380.79 fee allocation suggests why expensive insurance and thin margins are plausible. It does **not** show realized or expected actual profits are negative, since genuine concentrated positions have different fee shares, variable fees, price paths, rebate/rewards, and trading costs.

For the better-understood full-range Raydium example, $94.11 conditional fees minus $13.87 risk-neutral convexity value is an **$80.24 scenario margin** before costs. This does not imply a positive physical expected return or protection against SOL directional loss. Conditional on fee budget F=$94.11, the terminal SOL price interval where CPMM divergence is <= F is:

    (1-sqrt(2F/W))^2 <= S_T/S0 <= (1+sqrt(2F/W))^2
    i.e. approximately [0.74444,1.29320].

So even relative to HODL, a ~25.6% downside or ~29.3% upside price move breaks this particular fee-coverage bound if fees stay at the assumed total.

## 6. A closed-form solution for Bin allocation under a *frozen* flow assumption

Suppose each Bin has incumbent liquidity C_i>0, a hypothetical **exogenous** total fee budget A_i>=0 for the chosen horizon, a new invested amount w_i>=0, and an expected per-dollar inventory/divergence loss ell_i>0. In aligned capital/fee-share units, the new user's local fraction is approximately w_i/(C_i+w_i). Study the stylized concave optimization problem:

    max sum_i [ A_i*w_i/(C_i+w_i) - ell_i*w_i ]
    subject to w_i>=0, sum_i w_i<=W.

The second derivative of each fee-minus-loss term equals -2*A_i*C_i/(C_i+w_i)^3 <=0. The KKT conditions with shadow price lambda>=0 give an explicit optimum:

    w_i^* = max(0, sqrt(A_i*C_i/(lambda+ell_i))-C_i),

where lambda is chosen so the active capital budget is tight (or lambda=0 and some money remains in cash). Thus the optimizer automatically avoids a Bin with poor fee budget relative to risk and incumbent liquidity. It is NOT a rule saying "always put funds where pool reserves are lowest."

For ideal lower USDC-funded Bin K_i, under the same risk-neutral option model:

    ell_lower_i=Black_put(S0,K_i,sigma,T)/K_i.

For upper SOL-funded Bin at K_i:

    ell_upper_i=Black_call(S0,K_i,sigma,T)/S0.

An illustrative synthetic example with budget W=$2,000 and Bin inputs A=(50,120,80), C=(1000,2000,3000), ell=(1.2%,2.5%,1.8%) yields w≈(783.96,891.23,324.81) dollars, verified by the independent solver.

**Main limitation**: A_i is endogenous in a real AMM: future flow depends on swap fee, routing, market price, competitor depth, the amount we add, and adverse selection. Real DLMM ownership must be calculated from minted liquidity shares versus liquidity_supply, not merely dollar-marked reserves. The KKT result is a **conditional analytic benchmark**, not a uniquely profitable executable allocation.

## 7. Why raising the trading fee is not free money

Existing LPs normally cannot choose their own personal swap fee. Pool-level rules, authorized parameters and, for some AMMs, programmable Hooks govern fee calculation. Meteora's normal pool creation chooses base fee and Bin Step; the user-facing pool creation flow says these settings cannot change afterward, whereas authorized configuration changes may affect other dynamic fee fields. [Meteora official creation docs](https://github.com/MeteoraAg/docs/blob/main/user-guides/creating-a-liquidity-pool.mdx), [DLMM account definitions](https://github.com/MeteoraAg/docs/blob/main/developer-guides/dlmm/program/accounts.mdx).

For a pool operator choosing fee rate f, with trader flow Q(f) and adverse-selection cost H(f), net pre-fixed-cost economics are

    J(f)=alpha*f*Q(f)-H(f).

The interior first-order condition is

    alpha*(Q(f)+f*Q'(f))=H'(f).

Even holding H constant, revenue fQ peaks only at a fee where -fQ'/Q=1. Increasing f can drive organic traders to lower-fee venues while the pool still receives toxic, better-informed arbitrage flow. Meteora's variable fee changes with its volatility state, but does not automatically compensate every loss.

## 8. Decisive next measurement for the repository

1. Sample official on-chain account data for the Raydium V4 control pool and Meteora HTvj..., 5rCf..., BVR...; verify precise current fee, protocol share and Bin Step in the same slot/time.
2. Preserve per-Bin cumulative fee growth (fee_amount_x_per_token_stored, fee_amount_y_per_token_stored), real new PositionV2 minted shares and snapshots. Existing DB records basic price, reserves, liquidity_supply, but lacks full fee-growth time series. [Official Meteora account guide](https://github.com/MeteoraAg/docs/blob/main/developer-guides/dlmm/program/accounts.mdx).
3. Reconstruct actual user fees from the position's fee checkpoints and realized bin activity, not from the whole pool's 24h TVL ratio. Ingest [Swap events including bin range and fee components](https://github.com/MeteoraAg/docs/blob/main/developer-guides/dlmm/program/events.mdx).
4. Calibrate f and Q jointly and estimate physical expected fee earnings vs implied-vol market cost of LP Gamma. Do not silently set physical realized volatility equal to risk-neutral IV.
5. First compare **no options**, then affordable tail-only insurance, and lastly exact static Gamma replication. Fully buying back convexity has no inherent reason to preserve high APR.

**Bottom line:** analyze fee flow *per unit of negative Gamma* and conditional LP capital deployment, not fee APR or protocol brand in isolation.
