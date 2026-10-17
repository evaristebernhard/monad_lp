# Actual Meteora Bin reserves, fee-competition frontier and cross-protocol option hedging

**Study dated 2026-10-09. Research only.** Uses 2026-10-09 saved Git LFS pool/Bin and Deribit option depth in the public repository and the previously captured public market turnover reference; no user's PositionV2, no signed transaction, no profits guaranteed. The Bin snapshot for overall distribution and the option-linked snapshot have different observation times, deliberately identified.

## 1. What the actual database contains

Successful [GitHub Actions run 37913560546](https://github.com/evaristebernhard/arbitage/actions/runs/37913560546), [artifact 11608920306](https://github.com/evaristebernhard/arbitage/actions/runs/37913560546/artifacts/11608920306), reads the actual Git LFS SQLite bytes in read-only mode.

- 23 distinct DLMM pool-account snapshots, main pool \`5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6\`.
- Latest saved main pool snapshot **70**, **2026-10-09 08:30:22 UTC**, active ID **-5517**, 4-bps Bin Step, **4,908** populated Bin reserve entries.
- Main-pool Bin reserve mark to spot: **$4,001,727.38**; active Bin $7,036.51 (0.176% of saved marked reserves).
- Below active Bin: **$1,303,326.96** stable-side reserves, zero SOL; above active: **$2,691,363.90** SOL-side reserves, zero stable; these do not include the mixed active Bin.
- Within **±0.25%, ±0.5%, ±1%, ±1.4%, ±2.5%, ±5%, ±10%, ±20%** around active spot the marked reserve fraction is respectively **2.27%, 4.33%, 8.51%, 11.51%, 18.59%, 32.27%, 64.24%, 81.76%**.
- The top five bins by individual marked reserves are **above** spot, near **$118.89–$119.08**, ~7.98–8.15% higher than spot. Largest one holds ~$11,971 in marked reserve value. This describes CURRENT competition, not where future swaps will happen.
- Other saved DLMM SOL pairs include pool \`BGm1tav...\` with 10-bps Bin Step and ~$3.45m marked reserves, \`BVRbyLj...\` with 20-bps and ~$2.05m, plus 1-bps, 80-bps and 100-bps examples. Per-Bin concentration and options strike coarseness differ across them.

The pool's *active* Bin and price can move rapidly, and the saved Bin snapshot is not a historical fee-growth series. One can calculate users' indicative local ownership after a hypothetical new deposit, but not individual fee APR solely from these reserves.

## 2. Joint allocation/hedge risk frontier actually run

Successful [GitHub Actions run 37913835071](https://github.com/evaristebernhard/arbitage/actions/runs/37913835071), [artifact 11607966225](https://github.com/evaristebernhard/arbitage/actions/runs/37913835071/artifacts/11607966225), ran [research/dlmm_bin_allocation_frontier.py](../research/dlmm_bin_allocation_frontier.py).

For risk/option synchronization, use **pool snapshot 47 at 08:23 UTC** (spot **$110.27709**) and the nearby saved Deribit snapshot 2 at **08:23:52 UTC** (SOL index **$110.24920**), same-expiry October 23 options (~13.98 days). These are earlier than the reserve-distribution snapshot 70; do not merge their Bin states silently.

Each case explicitly chooses hypothetical **new deposited capital per Bin** and one-sided or two-sided price locations. **No actual wallet's PositionV2 was assumed**, no user fee/bonus income was modeled. The active Bin itself is omitted from funded bins to avoid inventing partial-conversion composition. Note that 35 below + 35 above funds 70 Bin IDs but spans a **71-ID range** with the active Bin unfilled; the actual PositionV2 account may require extension beyond its 70 inline slots.

The optimizer solves over whole **10-SOL** Deribit option contracts, from recorded (non-atomic) ask depth, each listed strike max three contracts; risk penalty λ=1 USDC per USD of max absolute replication residual on the *terminal ±10% SOL price window*. The initial Bin conversion payoff is idealized path-independent, and the worst residual is exactly certified at the union of Bin breakpoints, option strikes and price-window endpoints.

| Hypothetical new LP Bin layout, W=$10k | Observed competing existing reserves in chosen Bin IDs | New deposit share of these local reserves (capital-weighted) | Deribit option contracts | Indicative buy cost incl. fee | Worst interval residual | Fee APR hurdle incl. residual |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 10 below + 10 above (20 funded bins) | $139,256 | **6.70%** | 8 | **$343.65** | **$90.06** | **113.21%** |
| 35 below + 35 above (70 funded bins) | $457,722 | **2.15%** | 8 | **$348.65** | **$61.64** | **107.10%** |
| 125 below + 125 above (250 funded bins) | $1,325,948 | **0.86%** | 8 | **$303.65** | **$24.68** | **85.70%** |
| 70 below, USDC-only deposit | $410,520 | **2.47%** | 9 puts | **$362.98** | **$13.46** | **98.26%** |
| 70 above, SOL-only deposit | $419,902 | **2.34%** | 9 calls | **$362.98** | **$19.25** | **99.77%** |
| 10 below + 60 above, 35/65 funding | $425,757 | **2.86%** | 9 | **$380.98** | **$20.54** | **104.81%** |

**Why the local deposit share is NOT an APR**: the model reports the deposit-capital-weighted instantaneous ownership-like ratio \`W_i / (R_i+W_i)\` of the marked bin reserve value for compatible one-sided deposits. A future fee is earned only when swaps traverse its bin, with real supply shares and dynamic fees. The actual future per-bin swap volume distribution, composition fees, price path and effect of LP entry on market microstructure are not observed. The pool's ~99.52% *pool-wide turnover APR proxy* from 09:28 is not a substitute for any of the above six position APRs.

The option-plus-worst-residual APR hurdle is \`365*(observed option premium+indicative fee+absolute max residual)/(W*T_days)\`. It is a **conservative stress-risk hurdle** on the fixed hypothetical payoff, not a necessary condition for positive *expected* net P&L nor a realized return. The ±10% price range is a terminal stress set, not a 10% time window.

### Capital-size sensitivity for same 35+35-bin layout

| Hypothetical capital | Contracts | Indicative cost | Worst residual | Full residual+cost annualized fee hurdle |
| --- | ---: | ---: | ---: | ---: |
| $2,000 | 2 | $81.66 | $8.39 | **117.53%** |
| $5,000 | 4 | $176.32 | $28.71 | **107.04%** |
| $10,000 | 8 | $348.65 | $61.64 | **107.10%** |

This shows that ~$10k capital is not required to buy any option: 2k can support two integer contracts, but it does NOT imply attractive economics. The non-monotone effects of minimum 10-SOL lot rounding, discrete strikes, and depth remain.

### Selected contract examples (all October 23 maturity)

- **35+35, $10k:** 109 Put x2, 110 Put x2, 111 Call x3, 112 Call x1. Total 8 contracts.
- **125+125, $10k:** 108 Put x3, 109 Put x1, 112 Call x3, 114 Call x1. Total 8 contracts.
- **10+10, $10k:** 108 Put x1, 109 Put x1, 110 Put x2, 111 Call x3, 112 Call x1. Total 8 contracts.

Do not compare this new 70-bin candidate hedge price **$348.65** to the previous pro-rata ALL-pool-Bin basket hedge price **$122.99** as if they represent the same position. The prior scenario spread funds across ~4,908 bins and has a totally different payoff. The new 70-Bin position is much more concentrated near spot, making insurance near-ATM.

## 3. The mathematically correct *joint* problem cannot be solved uniquely without per-Bin fee observations

Let w_i≥0 be the hypothetical USD deposited in Bin i, total sum W; let R_i be its current competing reserves and L_i the issued liquidity supply. If the deposited token composition matches the bin and fee growth is LP-proportional, its initial share of each Bin is approximately

    rho_i(w_i) ~= w_i/(R_i+w_i).

This is a proxy for **fraction of that particular Bin's LP fees if swaps execute there**, not for fraction of ALL POOL trading volume. A forward-fee model needs a (possibly stochastic) future fee-generating volume A_i at each Bin and its effective LP-received fee rate f_i(t). The position's modeled fees are

    F(w) = sum_i  E[ integral_0^T rho_i(w_i,t) f_i(t) dA_i(t) ].

For each Bin allocation w, the HODL-relative ideal full-conversion loss D_w(S) is an explicit weighted sum of vanilla hinge payoffs, and for Deribit integer option decision n the static portfolio H_n(S) is similarly a sum.

A joint *economic* objective would involve

    maximize_(w_i,n_j)   E[F(w)-D_w(S_T)+H_n(S_T)] - option_premium(n) - trading_cost(w,n)
    subject to sum_i w_i=W, w_i>=0, n_j integers>=0,
               plus tail-risk or worst shortfall constraints.

For purely *static risk-cost* research, the implemented frontier enumerates multiple w layouts and solves the exact integer option subproblem for each. This is not yet the global optimizer of expected profit since the data lacks per-Bin future volume A_i(t), historical fee-growth and user PositionV2 shares. A fabricated expected-profit ranking would be unsound.

Meteora's own [account documentation](https://github.com/MeteoraAg/docs/blob/main/developer-guides/dlmm/program/accounts.mdx) explicitly lists \`fee_amount_x_per_token_stored\`, \`fee_amount_y_per_token_stored\` and PositionV2 fee checkpoints. Its [Swap2Evt event](https://github.com/MeteoraAg/docs/blob/main/developer-guides/dlmm/program/events.mdx) separates market-maker, protocol, and limit-order fees. The current DB does not store a full time series for per-Bin fee growth and user positions, though it DOES store reserve amounts and liquidity_supply.

## 4. The other protocols already in the repository

The dated catalog in \`data/lp_screener.db\`, inspected from LFS in [run 37913560546](https://github.com/evaristebernhard/arbitage/actions/runs/37913560546), counts:

| Protocol pool family | Total catalog pools | Static LP curvature class | Further necessary data to evaluate true position earnings |
| --- | ---: | --- | --- |
| Raydium Standard (\`solana_standard\`) | 1,093 | Usually constant-product sqrt(S) but family also contains other pool types | Decode AMM v4 vs CP-Swap vs StableSwap; vault reserves; actual LP share, 88% AMM v4 vs 84% CP-Swap per official Raydium fee rules |
| Raydium CLMM (\`solana_clmm\`) | 1,250 | Continuous sqrt-price curvature over position range | Actual NFT/tick bounds, active liquidity and per-tick fee-growth |
| Meteora DLMM (\`solana_dlmm\`) | 722 | Discrete Bin hinges (with dynamic fee path dependence) | Position Bin liquidity shares and actual bin-local fee-growth/Swap2Evt |
| Orca Whirlpool (\`solana_whirlpool\`) | 163 | Concentrated liquidity with per-tick active liquidity | Position range, tick arrays, active liquidity, fee growth |
| EVM constant-product V2 (\`constant_product_v2\`) | 20,714 | CPMM continuous global curvature | Pool share, fee split, reliable volume and position history |
| EVM V3 (\`concentrated_v3\`) | 9,493 | CLMM bounded curvature | NFT liquidity, ticks and fee growth |
| Uniswap V4 (\`uniswap_v4\`) | 5,855 | Often concentrated v3-like, but hook-/pool-specific fees and behavior | Pool key, active liquidity, hook code, dynamic fee rules, position identity |

**These are catalog counts, not fully decoded ready-to-hedge positions**. Do not infer Uniswap V4 fee_bps uniformly; catalog's common raw 3000 entry is in a different fee convention from DLMM 4-bps fields.

An appropriate research ordering for SOL options is: current Meteora DLMM vs Raydium Standard vs Raydium CLMM vs Orca Whirlpool, using **the same SOL maturity and explicit position capital**, after confirming each pool is the desired specific market. For ETH derivatives on EVM, start with verified Uniswap V2/V3 and carefully subset V4 hooks. Other underlyings without liquid matching options are not comparable.

Raydium [official fees](https://docs.raydium.io/raydium/protocol/protocol-fees) list LP shares of 88% for AMM v4 and 84% for CPMM/CLMM. Official Meteora docs distinguish a per-bin fee (including dynamic components) and per-position fee-growth accounting. Orca has separate concentrated tick-interval accounting, not DLMM Bin curvature.

## Next data field to collect before using a predicted net APY

For each actual Bin snapshot, capture cumulative per-token fees for SOL/USDC, supply and fee parameter state; for each sampled user position, decode shares and pending fees by Bin and record their time/slot. Match swap events to individual bin transitions and the actually charged maker-fee components. Then calibrate how much pool-wide volume *visits each candidate interval* during the 14-day expiry, and use scenario bounds while history coverage is immature. Never assign a single pool APR to a particular Bin allocation just because it has a high observed local ownership ratio.

No existing production DB or trading function was modified in this research branch.
