# DLMM Bin allocation and Deribit SOL contract granularity (2026-10-09)

## Executive result

Deribit SOL_USDC one option contract represents **10 SOL of underlying risk**, with minimum positive trade one contract. At the saved SOL reference of $110.25 this is **$1,102.50 notional**, **not** a minimum $1,102.50 option premium or a minimum $10,000 LP capital requirement. The initial long-option cash requirement is its premium. Source: https://support.deribit.com/hc/en-us/articles/31424932728093-Linear-USDC-Options

The rough $10k capital scale emerges from **quantity rounding precision across Bin/strike buckets**, not from exchange policy. Critically, the October 9 earlier three-leg/seven-leg hedge and quoted costs were computed for an *artificial pro-rata slice of all 4,908 populated pool bins*, not a realistic single concentrated user PositionV2. Do NOT apply those portfolio results unchanged to a centered narrow Bin position.

## Mathematical Bin-to-option mapping

Let spot S0, LP entry value W, user-owned USDC Y_i in lower-price Bin K_i<S0 and SOL X_j in upper-price Bin K_j>S0, with full idealized conversion on crossing each Bin. Ignoring the active Bin partial conversion and path-dependent fees, the nonlinearity relative to initial HODL is

    D(S) = sum_(K_i<S0) (Y_i/K_i) (K_i-S)^+
         + sum_(K_j>S0) X_j (S-K_j)^+.

Thus ideal required option underlying quantities are Qput=sum Y_i/K_i and Qcall=sum X_j. Pool total reserves cannot replace the *user's* Bin weights.

For a hypothetical symmetric 50%/50% deposit into near-spot bins,

    Qput ≈ Qcall ≈ W/(2*S0).

Matching at least one full 10-SOL contract on EACH side requires roughly

    W >= 20*S0 = $2,205 at S0=$110.25.

For a one-sided portfolio, the corresponding quantity threshold is 10*S0 ≈ $1,103. With less capital a contract may still be bought, but would overhedge. One-sided hedging alone does not protect the other side.

Nearest integer-size rounding incurs no more than 5 SOL absolute error PER AGGREGATED SIDE, giving the sufficient worst-case bound

    relative side-quantity error <= 5/(W/(2*S0)) = 10*S0/W.

Therefore to guarantee <=20%, <=10%, <=5% *aggregate quantity* error using this conservative bound needs respectively ~$5,513, $11,025, $22,050. These are SUFFICIENT precision bounds, not actual minimum deposits; rounding error is not monotonic with W.

| W | Ideal SOL/side | Nearest 10-SOL contracts/side | Relative error |
| ---: | ---: | ---: | ---: |
| $1,000 | 4.54 | 0 | 100% |
| $2,000 | 9.07 | 1 | 10.3% over |
| $3,000 | 13.61 | 1 | 26.5% under |
| $5,000 | 22.68 | 2 | 11.8% under |
| $10,000 | 45.35 | 5 | 10.3% over |
| $20,000 | 90.70 | 9 | 0.8% under |

These numbers ignore multiple strike buckets and execution liquidity; the practical situation can be worse.

## Why Bin count matters

Saved target Meteora pool 5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6 has **4 bps Bin Step**. Seventy consecutive Bin intervals span 1.0004^70-1 ~=2.84% total price. A centered 35+35-bin LP spans roughly +/-1.41%. Default PositionV2 stores 70 shares inline but its dynamic extension allows more; 70 is not a hard max. Official layout: https://github.com/MeteoraAg/docs/blob/main/developer-guides/dlmm/program/accounts.mdx

Example only: $10k initial cash split equally across 70 bins gives ~$142.86 per Bin, or roughly 1.30 SOL of option-underlying per Bin. One contract FOR EACH Bin would require ~$77k under this artificial allocation, but that is not necessary because nearby bins can share an option.

For grouped Bin weights w_i with strike min a, max b, define q=sum w_i and centroid k=sum(w_i*K_i)/q. Replacing group's full vanilla curvature payoff by ONE q-SOL option struck at k matches mass and first moment. The hinge-payoff Jensen inequality and convex secant bound imply

    0 <= group-replication-residual <= q*(b-a)/4

for the same option wing. It vanishes outside [a,b]. For disjoint contiguous groups the residual supports are disjoint, so the global ideal aggregation error equals the worst group's error, not their sum.

At $10k, a group of ~8 consecutive equal-dollar bins carries ~10 SOL; their ~7 Bin intervals cover ~$0.31 near S0 and yield group aggregation error <= ~$0.78 (for this illustrative smooth setup), BEFORE snapping to an actually listed Deribit strike and integer contract rounding. At $2k each Bin carries ~0.26 SOL; collecting ~10 SOL requires about 39 bins, more than the 35 bins on each side of the centered 70-bin position.

The actual number/width of groups should therefore be optimized jointly with available strikes and book depth. We do NOT need one contract per Bin.

## Economic effect: more principal does not make expensive option IV disappear

The saved Meteora on-chain parameters yield **4 bps base swap fee and 10% protocol share**. The Oct 9 09:28 UTC public snapshot gave ~$31.13m 24h volume / ~$4.11m TVL. Assuming this volume and TVL remain constant for 13.98 days, a FULL POOL-PRO-RATA $10k holding's hypothetical base LP fee is ~$381.27 (99.52% turnover-implied annualized). It is NOT a real concentrated Position's earned fees and ignores variable fees.

The earlier 3-contract $122.99 and 7-contract $177.32 cost results applied to the artificial whole-pool pro-rata basket, which carries curvature across many thousands of Bin prices. A 70-bin position's option exposures concentrate near the money, raising hedge cost substantially.

As a **separate illustrative Black-model calculation**, suppose 35 lower bins hold equal initial USDC dollars, 35 upper bins hold equal initial SOL-value dollars, W=$10k, S0=F=$110.25, T=13.9834/365, a *flat* IV of 53.8% and zero discount. Directly summing all fractional put/call prices over the 70 ideal Bin strikes gives **theoretical premium ≈$387.47 (~3.875% of capital)** before spreads, available listed strikes and contract multipliers. This is NOT a real Deribit executable ask quote. In the limiting ultra-narrow symmetric Bin range, the portfolio approximates a short ATM straddle and the fair protection price scales approximately as

    P_hedge/W ≈ IV * sqrt(T) / sqrt(2*pi) ≈ 4.20%,

independent of deposit size W to first order. The separate pool-pro-rata ~3.81% 14-day fee reference is already comparable to or BELOW this protection cost. A real concentrated Bin LP could receive more fees per invested dollar *while active*, or much fewer over time when out of range. Real spread and dynamic fees must be observed.

For perspective, 53.8% IV implies ~10.53% one-standard-deviation log-return scale across 14 days, much wider than +/-1.41%. An illustrative constant-vol diffusion starting in the center spends an estimated ~19.6% of calendar time within such a range on average over 14 days; option IV is not physical realized volatility, and time in range CANNOT be equated to fraction of fee income.

## Implications

- $10,000 is NOT a Deribit minimum capital requirement.
- ~$2,200 is enough to have approximately 10 SOL of symmetric option-underlying risk per wing in a near-spot 50/50 LP; not automatically profitable or low-error.
- ~$10k is a plausible scale to group bins into several 10-SOL buckets while controlling *relative* integer size error, but optimal capital is not universal and depends on Bin weights, market strikes, fee capture and price window.
- If realistic LP fees per dollar are insufficient to cover option premiums per dollar, increasing capital does NOT fix the underlying economics; it only reduces relative quantization friction, and can dilute local Bin fee share.
- To optimize real LP+options, decode the user's PositionV2 shares, aggregate bins by option strike/maturity, solve integer depth-constrained hedge, and separately calculate actual position fee-growth; do not reuse pool TVL average as user fee income.

This is theory/controlled examples, not a current trade instruction or realized backtest.
