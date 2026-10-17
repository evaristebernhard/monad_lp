# Solana SOL/USDC LP fees versus Deribit options — 2026-10-09

**Status: read-only cross-source feasibility screen; NOT a seven-day historical backtest or actual P&L.**

## Sources and timestamps

- rpc/data/rpc.db on Git LFS contains routing/provider settings: 32 enabled chains and 169 endpoints, **not** pool APR. Do NOT publish endpoint URLs, because API tokens could be included.
- data/lp_screener.db: 40,627 pools, of which Solana includes Raydium Standard 1,093, Raydium CLMM 1,250, Meteora DLMM 722 and Orca Whirlpools 163. Key representative pool metadata timestamp: 2026-10-01 06:59:27 UTC.
- data/lp_price_history.db: 252,927 pool price observations spanning 2026-10-07 06:54 to 12:28 UTC; 525,278 token price observations mainly 2026-10-06 to 2026-10-07. Counts do NOT establish seven days per pool.
- rpc/data/solana_events.db: 3,681 classified swaps observed between 2026-10-08 09:51 and 15:17 UTC (~5.4h); pool subscriptions are selective, not full trade coverage.
- Latest public DexScreener Solana pair lookup: 2026-10-09 06:56 UTC, captured by successful GitHub Actions run https://github.com/evaristebernhard/arbitage/actions/runs/37896138507 . API reference: https://docs.dexscreener.com/api/reference
- Deribit live research order books and integer hedges: https://github.com/evaristebernhard/arbitage/actions/runs/37885383115 . Its options are for the 2026-10-16 08:00 UTC expiry.

## Four pools: archived vs newly queried h24 market statistics

| Pool type and actual Solana pool | Oct 1 TVL / h24 volume | Oct 9 TVL / h24 volume | Proxy fee APR derived from Oct 9 |
| --- | --- | --- | --- |
| Raydium Standard 58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2 | $37.23M / $14.11M | **$23.99M / $4.80M** | **16.05%** if AMM V4 LP share is 88% |
| Raydium CLMM 3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv | $7.82M / $44.32M | **$7.63M / $34.87M** | **56.02%** proxy (4 bps gross, LP fraction 84%) |
| Meteora DLMM 5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6 | $4.27M / $49.18M | **$4.11M / $44.01M** | **148.64%** proxy (4 bps base, assumed 95%; dynamic fees unmodeled) |
| Orca Whirlpools Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE | $31.20M / $241.33M | **$28.02M / $127.48M** | **66.42%** proxy (4 bps, unverified 100% LP share) |

Proxy fee APR = 365 * (24h swap volume / TVL) * (gross fee bps / 10000) * LP share. It is neither a realized yield nor a forecast. Current volume, fee tier, and LP share need continuous refreshing and on-chain verification. Do not use pool-wide TVL share to estimate a concentrated LP position's actual fees.

### Important Raydium subtype and fee correction

The Oct 1 catalog's Standard row has fee_bps=25, lp_fee_share_assumption=1.0, and lp_fee_apr_estimate=0.34576 (34.58% APR). If the on-chain pool is legacy Raydium AMM V4, the LP share is 88%, so the corresponding OLD-turnover estimate is only 30.43%. The NEW 24h turnover implies about 16.05% before additional costs. Raydium CPMM/CLMM typically allocate only 84% of the trade fee to LPs. A single solana_standard family mixes legacy AMM V4, CP-Swap and stable variants, so no blanket fee-share update can safely be applied. Official Raydium docs: https://docs.raydium.io/raydium/protocol/protocol-fees

## Conditional seven-day LP / Deribit option comparison (W0 = $10,000)

Define initial HODL benchmark B(S), static LP principal V(S), D(S)=B(S)-V(S), options payoff H(S), options and execution costs C, and realized position fees F. Excess payoff equals F-D(S_T)+H(S_T)-C.

Previous integer SOL_USDC hedges bought only long puts/calls, using observed (non-atomic) Deribit orderbook asks. For a hypothetical +/-20% SOL risk interval, CPMM needed 2 contracts at ~$8.66 indicative purchase plus fee cost with $24.90 maximum residual. CLMM needed 9 contracts at ~$95.97 cost with $9.94 maximum residual.

| Seven-day deterministic thought experiment | CPMM / legacy Standard | Concentrated CLMM (not actual position fees) |
| --- | ---: | ---: |
| W0 * latest turnover-implied APR * 7/365 | **$30.79** | **$107.44** |
| Deribit option + estimated fees | $8.66 | $95.97 |
| Max principal replication residual in hypothetical +/-20% region | $24.90 | $9.94 |
| Fee proxy - option cost - maximum residual | **-$2.77** | **+$1.53** |

**Do not call the above backtested profit**: LP fees are not constant over a week; the pool-wide CLMM proxy does NOT determine the user's actual fee revenue; Deribit data is not atomic with DexScreener, and a worst-case static residual is not an expected residual. Premium-only annualized fee hurdles are ~4.52% (CPMM) and ~50.04% (CLMM), and premium plus max residual hurdles are ~17.50% and ~55.22% respectively. The CLMM proxy is so close to the hurdle that slight slippage, a tick going inactive, or smaller active-liquidity share can flip the sign.

## Minimum data needed for a credible historical or executable result

1. Refresh pool account subtype and fee split on the same timestamp as TVL and volume. Handle Raydium AMM V4, CPMM, StableSwap separately.
2. Read the user's actual concentrated LP position, bounds, fee-growth counters, and per-tick active liquidity; use observed fee accrual rather than TVL-based allocated fees.
3. Collect timestamp-aligned seven-day prices and swaps. The existing events and prices cover distinct short windows; never fabricate gap-free history.
4. Capture Deribit orderbook depth, option expiry and fills at entry and exit; include commissions, collateral, option settlement and funding, as well as realistic cross-venue execution.
5. Replay HODL, unhedged LP, static options and dynamic delta hedges against the *same* price and volume paths; report realized P&L, drawdown and CVaR.

All associated Actions output only sanitized aggregate JSON, not public RPC endpoint URLs or wallet data. No main production database writes or trades were made.
