# Full-range CPMM LP: 7-day/30-day fee persistence and no-option economics

**Research dated 2026-10-09, around 12:32 UTC. No real LP investment or realized profit.**

Successful [GitHub Actions 37930647479](https://github.com/evaristebernhard/arbitage/actions/runs/37930647479), public, sanitized JSON research artifact. Source code: [research/cpmm_7d30d_sustainable_fee.py](../research/cpmm_7d30d_sustainable_fee.py).

## 1. Research question

Can a low-maintenance Raydium Standard full-range SOL/stablecoin CPMM, *without compulsory options*, have more attractive fee-versus-short-Gamma economics than actively managed CLMM/DLMM?

For 50/50 initial wealth W, external volatile price ratio r=S_T/S_0:

    LP_without_fees = W * sqrt(r)
    same_initial_asset_HODL = W*(1+r)/2
    divergence_loss = (W/2)*(sqrt(r)-1)^2

Simplified profit **relative to the same initial 50/50 HODL** is hypothetical LP fee receipts minus divergence loss minus entry/exit costs. Total USD investment loss also includes SOL price movement and can be far larger. The formula holds for an ideal frictionless fee-free invariant and is not exact accounting of fee-reinvesting AMM reserves.

## 2. Actual Raydium 7d/30d pool fee research

Raydium API V3 [official reference](https://api-v3.raydium.io/docs/) provides 24-hour, 7-day and 30-day per-pool actual historical fee aggregates through fields day.volumeFee, week.volumeFee and month.volumeFee. The [official SDK schema](https://github.com/raydium-io/raydium-sdk-V2/blob/master/src/api/type.ts) documents the time-window fields. These overlapping rolling aggregates are NOT 30 independent daily observations and are NOT returns earned by an individual LP.

The Actions program discovers candidate pools in existing Git LFS data/lp_screener.db AND the official Raydium pool-by-mint endpoint, verifies canonical WSOL with canonical USDC or USDT token addresses, and **requires the actual Raydium AMM V4 program address 675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8**. Raydium CP-Swap pools and CLMM pools are not accidentally classified as AMM V4. API 24h/7d/30d fee totals must match volume multiplied by verified feeRate within tolerance. No user wallet or private RPC secret is read.

The official SOL/USDC and SOL/USDT pair lists returned 31 and 14 Standard pools respectively (many were CP-Swap, not AMM V4). **21 canonical legacy AMM V4 pools** passed pool program and token validation. Of these, **19 had nearly zero present TVL**, all below about $3,200 and unsuitable for $2k-$10k deposits on size/liquidity grounds. Only two had substantive current liquidity in these asset pairs.

Verified successful Actions run: https://github.com/evaristebernhard/arbitage/actions/runs/37930647479

| Metric (official Raydium, 2026-10-09 12:32 UTC) | AMM V4 SOL/USDT 7Xawhbb... | AMM V4 SOL/USDC 58oQChx... |
| --- | ---: | ---: |
| Current pool TVL | $1,367,029 | $24,019,415 |
| Gross 30-day realized pool swap fees | $28,702 | $2,409,507 |
| Gross fees in latest 24 hours | $1,087 | $10,632 |
| Past-7d daily average / past-30d daily average | **1.060** | **0.174** |
| $10,000 new LP: conditional 14d fees if trailing-30d daily fee level persists | **$85.60** | **$411.79** |
| $10,000: conditional 14d fees if trailing-7d daily fee level persists | **$90.76** | **$71.65** |
| $2,000: conditional 14d fees if trailing-7d level persists | **$18.26** | **$14.33** |
| Historical creation age from API for these pools | Unknown | Unknown |

The smaller SOL/USDT pool's *latest 7-day fee activity* is consistent with its trailing 30-day daily average. The larger SOL/USDC pool has a **roughly 82.6% lower recent 7d daily fee level** compared with its own trailing-30d average. Ranking by 30d average alone would favor the huge pool; using recent 7d averages gives the opposite ranking for a new fixed-size investor. Neither ranking establishes future yield.

### Fee assumption and its limitations

From [Raydium AMM V4 official fee specifications](https://docs.raydium.io/raydium/products/amm-v4/fees), the default gross fee rate is 0.25%, with **88% to LP and 12% to protocol**. The Actions job directly verifies the current feeRate field of each actual pool and treats the published 88% as a working assumption requiring individual on-chain AmmInfo confirmation.

For a hypothetical new whole-pool share W/(T+W), with CURRENT pool TVL T, retrospective aggregate gross fees B of lookback length L days, and hypothetical holding H days:

    F_scenario = (B/L) * H * 0.88 * W/(T+W)

This is **NOT the actual earned fee** of an LP who had invested in the previous 7 or 30 days; historical LP-token supply, TVL trajectory and fee compounding are absent. It is equally **NOT a forecast** of the next H days. Swap volume and pool fee flow are endogenous to routing and the competing depth.

For the smaller pool, the initial $10k share is around 0.73%, $2k around 0.146%. Nothing in this observation guarantees the LP can enter/exit without slippage or fees.

## 3. Risk: profitable versus HODL is not profitable versus USDC cash

Ideal fee-free 50/50 CPMM, initial W=$10k:

| SOL terminal change | LP USD value without fees | Loss relative to initial same-asset HODL |
| --- | ---: | ---: |
| -10% | $9,486.83 | $13.17 |
| -25% | $8,660.25 | $89.75 |
| -50% | $7,071.07 | $428.93 |
| +25% | $11,180.34 | $69.66 |

At -25% SOL, adding the entirely hypothetical +$90.76 fourteen-day fee scenario would leave around **$8,751.01**, a **$1,248.99 USD loss** versus initial $10k, even though divergence relative to HODL would almost be covered. Never market this as a low-risk principal-protected investment.

For 14 days and an ASSUMED risk-neutral lognormal volatility of 55%/year, the risk-neutral market value of fee-free short convexity is

    E_Q[D] = W*(1-exp(-sigma^2*T/8)) ≈ $14.50 (W=$10k).

Comparing this theoretical price to extrapolated historic fees gives a *conditional* arithmetic stress budget, **not physical expected P&L**: realized volatility, price drift, fee-flow/price correlation and USDT credit risk remain unknown. Stablecoin risks remain even if spot prices appear pegged.

## 4. Research screening, not an automatic trade recommendation

A simple maintenance-conscious candidate-audit screen checks:
- exact verified WSOL + native canonical USDC/USDT mint addresses, AMM V4 program and pool trade fee;
- current pool TVL between $250k and $5m, $10k deposit not more than 2% of the pool TVL;
- latest 7-day daily fee average at least 50% of trailing 30-day daily average;
- hypothetical fee budget exceeds 14-day 55%-annual-vol risk-neutral convexity benchmark;
- actual pool age >=30 days verified separately.

**No pool is marked automatically investment-approved**: the Raydium API returned unknown creation age for both materially liquid reference pools; the small SOL/USDT passes the other four screening gates but is explicitly labeled **PENDING AGE / ON-CHAIN STATE VERIFICATION**. The huge SOL/USDC fails the study's small-TVL and fee-persistence filters. These arbitrary numeric gates are research parameters, not an investment or safety rating.

## 5. What this changes about the arbitage project

- Prefer **low-maintenance verified CPMM pools with real 7d/30d fee history** over generic one-day APR leaderboards. Do not force costly SOL/ETH put+call hedge trades.
- Preserve the existing DLMM/CLMM data pipelines as *research*, since narrow-range Gamma insurance and frequent position management can eat the fee advantage.
- Next verification: on-chain individual AMM V4 AmmInfo swap fee numerators, LP mint total supply, adjusted vault reserves and need_take_pnl; confirm pool creation/open time. Only then begin recording a consecutive daily true LP-token NAV and historical HODL comparison.
- Broaden the research to Raydium **CP-Swap full-range CPMM** (different program and 84% LP fee split), and separately Ethereum Uniswap V2 (currently 0.25% LP out of 0.30% gross trading fee under governance protocol split; [official Uniswap fee docs](https://developers.uniswap.org/docs/protocols/protocol-fee/concepts/fees)). Their economics should not be faked by inheriting Standard AMM V4's 88% LP take.
- Real backtesting requires timestamp-aligned actual LP-token NAV or swap-receipt history, historical pool supply/liquidity, position deposit amounts, pricing data and transaction costs. A 30-day fee aggregate and today's TVL are NOT enough to reconstruct an actual past deposit's total return.

## Reproduction

Run the [GitHub Actions workflow](../.github/workflows/cpmm-7d30d-fee-screen.yml) or the read-only [research script](../research/cpmm_7d30d_sustainable_fee.py). The workflow retrieves only Git LFS data/lp_screener.db and Raydium's public HTTP endpoints. It never reads public-history credential-bearing rpc/data/rpc.db, private wallets or executes transactions. After merge to main, it is set to collect one new official aggregate snapshot daily at **07:35 UTC / 15:35 Asia/Singapore**, subject to GitHub Actions cron delays.

Research status: **A good evidence-based candidate to audit (small AMM V4 SOL/USDT), not an approved or verified profitable investment.**
