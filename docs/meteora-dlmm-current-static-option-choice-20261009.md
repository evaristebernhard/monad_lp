# Meteora DLMM: current Bin snapshot vs Deribit SOL_USDC static options

**Research date:** 2026-10-09. **Status:** exact static price-window risk optimization using *saved current snapshot*, not a historical backtest, observed fill, or personal LP position.

## Aligned observation

- DLMM pool: `5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6`, pool snapshot `47`, 2026-10-09 **08:23:08 UTC**.
- DLMM active bin: `-5513`, Bin step **4 bps**, 4,908 populated bins for this pool; pool bin price **$110.27709** per SOL.
- Deribit option snapshot `2`: 2026-10-09 **08:23:52 UTC**, index **$110.24920**, pool/index discrepancy **0.0253%**, separated by **44.93 s**.
- Shared options maturity: `SOL_USDC-23OCT26-...-C/P`, **2026-10-23 08:00 UTC**, approximately **13.98 days** after snapshot. Options are USDC-quoted, 10 SOL per contract.
- Pool data is **whole-pool Bin inventory** (not any wallet PositionV2). Construct a hypothetical **$10,000 pro-rata share of every populated pool Bin**, scaling all inventories by `10000 / 4005902.1727`. This is an *illustrative static passive reserve basket*, not a claim that an ordinary position earns pool-wide fee APR.
- Bin-derived synthetic payoff uses `Y_i / K_i` SOL of put-side curvature for bins at or below active, `X_i` SOL of call-side curvature for bins at or above active. Both components of active bin are treated under an idealized full conversion convention. There is no realized accrued fee model or active-bin path dependence. Synthetic total bin weights are ~35.45 SOL put-side and ~60.38 SOL call-side.

### Data-integrity fix

`rpc/tools/collect_meteora_dlmm_deribit.py` previously double multiplied `BinArray.price_y_per_x` by `10**(token_x_decimals-token_y_decimals)`, even though the BinArray parser had already applied the token decimal scale. The stored active bin appeared around **$110,277** versus the independently stored pool price **$110.277**. The future collector now saves the already normalized price, while `research/dlmm_option_window_strategy.py` checks the active bin against the stored pool price and corrects the factor of **1,000** for this **old** snapshot *in memory*. Underlying Git LFS research DB remains unchanged.

## Objective, assumptions and method

For a hypothetical static position `D(S)` (HODL-relative ideal Bin conversion loss), a set of listed same-expiry long puts/calls `H_n(S)`, and integer contracts `n`:

```
min premium_at_ask_depth(n) + estimated_public_fee(n) + lambda * E
subject to |D(S)-H_n(S)| <= E for all S in [S0*(1-w), S0*(1+w)]
           n_j in nonnegative integers, at most 3 whole 10-SOL lots per eligible strike.
```

The exact maximum absolute static replication residual is attained at the interval endpoints or one of the DLMM Bin/option strike breakpoints, because both payoffs are piecewise affine. A single MILP checks every breakpoint. `lambda` is a **subjective penalty in USDC per USD worst-case residual**, not a forecast return, utility calibration or market probability.

Ask costs are integrated over the previously saved **non-atomic** Deribit displayed ask levels; public fees are estimated. No liquidity provision, LP fees, execution slippage outside observed depth, settlement funding or actual trades enter the objective. `lambda=0.1` and `lambda=1` show how risk preference changes the integer optimum.

## Exact current-snapshot results

Source: [successful GitHub Actions run 37910170471](https://github.com/evaristebernhard/arbitage/actions/runs/37910170471); [raw JSON artifact 11605754511](https://github.com/evaristebernhard/arbitage/actions/runs/37910170471/artifacts/11605754511).

| Price stress window | Risk lambda | Contracts | Premium + estimated fees | No-option worst D(S) | Worst residual after hedge |
| --- | ---: | ---: | ---: | ---: | ---: |
| +/-10% (99.249–121.305) | 0.1 | 0 | $0 | $202.45 | $202.45 |
| +/-10% (99.249–121.305) | 1.0 | **3** | **$122.99** | $202.45 | **$29.46** |
| +/-20% (88.222–132.332) | 0.1 | 0 | $0 | $745.67 | $745.67 |
| +/-20% (88.222–132.332) | 1.0 | **7** | **$177.32** | $745.67 | **$15.83** |

All four cases report `risk_certified`, HiGHS solver status 0 and a verified breakpoint residual certificate. These are **candidate hedges conditional on the proxy portfolio and the Oct 9 observed book**, not order recommendations for a personal position.

**+/-10%, lambda=1:**
- Buy **1 contract** `SOL_USDC-23OCT26-110-P` (110 Put).
- Buy **1 contract** `SOL_USDC-23OCT26-111-C` (111 Call).
- Buy **1 contract** `SOL_USDC-23OCT26-114-C` (114 Call).
- Observed sum of ask premiums is **$122.00**; indicative premium + fees **$122.99**.

**+/-20%, lambda=1:**
- Buy one each of **104 Put, 108 Put** (contracts `SOL_USDC-23OCT26-104-P`, `SOL_USDC-23OCT26-108-P`).
- Buy one each of **113 Call, 115 Call, 118 Call, 120 Call, 122 Call** (same `23OCT26` expiry).
- Observed sum of ask premiums **$175.00**; estimated inclusive cost **$177.32**.

### Critical interpretation: the time window is not the stress window

The options have ~14 days to expiry. `+/-10%` and `+/-20%` are **terminal SOL price scenario bounds at option expiry**, not 10%/20% time windows. The 72 saved hourly pool candles are not used by the optimizer and cannot be treated as a 14-day option P&L history.

In a real implementation the price interval should be selected by a *risk criterion* (e.g. quantiles under forward implied volatility), not by an arbitrary fixed width; the current model deliberately exposes both widths for sensitivity.

No historical PositionV2 replay is required to calculate these static theoretical candidate contracts. However, to apply this to a **real position**, first decode its actual current liquidity shares across bins and scale holdings from the user account; using aggregate pool Bin reserves assumes the user owns a proportional slice of all bins, which may be very wrong.

**Do not deploy based solely on this static illustrative portfolio.**
