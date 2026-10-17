# Real DLMM Bin fee-growth data (first on-chain baseline, October 9, 2026)

This is a read-only audit of the user's existing Git LFS SQLite and a fresh Solana RPC account collection, not an actual position fee return, a trading signal or a historical backtest.

**Verified GitHub Actions:** https://github.com/evaristebernhard/arbitage/actions/runs/37925823991
**Sanitized public JSON artifact:** https://github.com/evaristebernhard/arbitage/actions/runs/37925823991/artifacts/11614206116

## 1. What old GitHub data actually contained

The existing research SQLite, data/sol_dlmm_deribit.db, contains **23 pool rows, 92 pool snapshots, 74,001 Bin snapshots and two Deribit snapshots**. The Bin table saves reserve amounts and liquidity_supply, but NEVER saved fee_amount_x_per_token_stored or fee_amount_y_per_token_stored. Thus old cumulative historical fees cannot be recovered from the old Bin rows.

Latest archived snapshots, all observed **2026-10-09 08:30:22 UTC**:

| Pool | Archived active bin ID | Populated Bin entries |
| --- | ---: | ---: |
| 1bp HTvjzsfX3yU6BUodCjZ5vZkUrAxMDTrBs3CJaq43ashR | -22068 | 12,264 |
| 4bp 5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6 | -5517 | 4,908 |
| 20bp BVRbyLjjfSBcoyiYFuxbgKYnWuiFaF9CSXEa5vdSZ9Hh | -1103 | 975 |

Do not confuse the archived 08:30 state with the new 11:46 live account state.

## 2. Actual live RPC fee configuration, not just old catalog tiers

In the **11:46:49–11:46:50 UTC** Solana mainnet sample at **slots 454857042–454857044**, three pool accounts and five adjacent BinArray PDAs per pool were fetched and structurally validated. This provides **350 decoded Bin entries per pool** with fresh reserve, liquidity-supply, and public fee-growth counters.

| Pool | New active bin | Base fee | Current variable-fee component | Total state-implied fee | On-chain protocol share |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1bp | -21922 | 1.00000 bps | 2.00000 bps | **3.00000 bps** | 10% |
| 4bp | -5483 | 4.00000 bps | 7.03439 bps | **11.03439 bps** | 10% |
| 20bp | -1098 | 20.00000 bps | 1.18193 bps | **21.18193 bps** | 10% |

The accumulator values come directly from Meteora StaticParameters and VariableParameters. The variable fee is state-dependent and may decay/update at trade time, so **state-implied fees are NOT realized trade-weighted fees**.

The new parser explicitly reads Bin fee growth U128s at **Bin offset +80 (token X)** and **+96 (token Y)** within 144-byte fixed-size Bin records after the 56-byte BinArray header, including negative Bin indexes. This follows the [Meteora official account reference](https://github.com/MeteoraAg/docs/blob/main/developer-guides/dlmm/program/accounts.mdx) and [generated Bin layout](https://docs.rs/crate/carbon-meteora-dlmm-decoder/latest/source/src/types/bin.rs).

All 350 sampled near-active bin records per pool had nonzero accumulated fee counters: **a NONZERO historical cumulative counter does not establish new swap fees within this 11:46 collection interval**.

## 3. Measuring real NEW fee growth instead of faking APR

The first saved artifact is a **baseline**. Without a previous fee-growth counter snapshot it is mathematically impossible to infer the daily incremental fee from this new accumulated value alone. The Actions job subsequently fetches the prior public research artifact by a repository-scoped read-only GitHub token and compares raw fee-growth counters for the same pool and Bin across Solana slots. It records:

- Number of observed common Bin IDs and each nonnegative increment in the two fee-growth U128 counters.
- Whether recorded liquidity_supply changed between snapshots.
- Negative deltas/counter resets as a warning, instead of replacing them with positive values.
- UTC observation time and Solana context slot.

Raw fee growth is a program fixed-point measure, **not a dollar amount**. To compute a real user's fees, decode the user-owned PositionV2 liquidity_shares and fee checkpoints, use the protocol's exact fixed-point rounding and compare against claimable amounts. We have NOT observed any user PositionV2 or earnings.

The raw 1/4/20 bps time series can eventually calibrate Bin-specific fee budgets A_i for the previously derived KKT allocation. One sample cannot reliably extrapolate A_i over a two-week holding horizon; update/exit/rebalance and price routing remain causal confounders.

## 4. RPC security

The read-only job uses rpc/data/rpc.db strictly as a source of configured Solana RPC endpoints and filters to allowlisted HTTPS RPC domain suffixes; it NEVER prints or uploads an endpoint URL, request headers, URL query, API key or RPC database file. The first run found seven compatible enabled configured endpoints, five of the scanned URLs with paths/queries that could embed access keys, and reached a working endpoint on the first attempt. The repository is public and may have live API credentials embedded in Git LFS history: **rotate any exposed keys and remove their old history**, rather than relying on a hidden Actions log.

GitHub Actions output contains only public pool addresses, counters, fee config, slot IDs and research calculations; no transaction or wallet mutation occurs.

## 5. Next economic comparison

Use a multi-snapshot set of Bin fee growth, explicit user PositionV2 shares or a clearly labeled hypothetical minted share, and relevant spot prices to compute fees by activity window. Only then compare:

- Raydium Standard full-range 25-bps verified fee, 88% LP share (no compulsory long options);
- Meteora 1-bp nominal pool (3.0-bps current state-implied);
- Meteora 4-bps nominal pool (11.034-bps current state-implied);
- Meteora 20-bps nominal pool (21.182-bps current state-implied).

For any chosen real LP position, compare actually earned fees to the divergence payoff, rebalancing costs and transaction gas; report both USD-directional risk and HODL-relative loss. Do NOT substitute pool-wide h24 APR for per-position fees or price-insurance economics.

Files: [read-only collector](../research/dlmm_fee_growth_rpc_probe.py) and [GitHub Actions workflow](../.github/workflows/dlmm-fee-growth-rpc.yml). Neither modifies the existing historical SQLite nor any trading service.

## 6. Verified two-slot delta and durable cache

Two independent Oct 9 Actions artifacts at 11:46 and 11:50 UTC were replayed by [run 37926481993](https://github.com/evaristebernhard/arbitage/actions/runs/37926481993), with 350 overlapping Bin IDs per pool. Over ~213 seconds, 1-bp fee counters increased in 67 bins (290 supply changes), 4-bps in 15 bins (249 supply changes), 20-bps in 2 bins (81 supply changes). No fee counter decreases. These figures represent *which counters changed*, not fee quantity or user APY.

The collector now supports a **repository-scoped Actions cache** of the prior sanitized observation, without having to download the original Git LFS DB as an artifact. The first cache-save run [37926563418](https://github.com/evaristebernhard/arbitage/actions/runs/37926563418) stored a public baseline, and the following [run 37926659902](https://github.com/evaristebernhard/arbitage/actions/runs/37926659902) **successfully restored it** and compared later account slots: 1-bp pool **8 bins** with fee increases (64 supply changes), 4-bps **2 bins** (104 supply changes), 20-bps **0 bins** (0 supply changes), with 350 common bins each, over ~54 seconds. No counter resets. These later data are not a substitute for 14-day observation.

The job is scheduled every four hours, 15 minutes past the hour (UTC), and manually triggerable. Cache restoration is opportunistic; failure is labeled baseline-only rather than inventing missed historical increments. The math still requires **actual per-position liquidity shares and accurate program token fee-growth scaling**, so we intentionally do not report a dollar APR. The collection is narrowly bounded to 5 near-active arrays per pool. 


## 6. Verified **two-timepoint growth** and continuously cached sampling

The first two real Actions samples were collected around **11:46:49** and **11:50:22 UTC**, separated by approximately **213 seconds**, and independently compared by [successful verification workflow 37926481993](https://github.com/evaristebernhard/arbitage/actions/runs/37926481993). This verification **downloaded both original Actions artifacts** instead of trusting a manually entered summary.

| DLMM pool | Same-bin counters observed | Bins with positive fee growth | Bins with changed liquidity_supply | Active Bin movement | Negative growth counters |
| --- | ---: | ---: | ---: | --- | ---: |
| 1 bp | 350 | **67** | 290 | -21922 to -21966 | 0 |
| 4 bps | 350 | **15** | 249 | -5483 to -5491 | 0 |
| 20 bps | 350 | **2** | 81 | -1098 to -1098 | 0 |

**Changing Bin count is not fee dollars.** Even bins without a supply change can accrue fees; many bins with a changed supply have no increment in the period. Raw fee-growth deltas are the authoritative on-chain evidence for activity; the number of growing bins is a spatial-activity statistic only. It does not support claiming 1 bp is more profitable than 20 bps. More active Bin migration does not necessarily mean less profitable flow.

A subsequent [successful 37926659902 run](https://github.com/evaristebernhard/arbitage/actions/runs/37926659902), at **11:54:06–11:55:00 UTC**, verified **Actions cache restoring the immediately previous research snapshot**: in ~54 seconds, 1-bp pool fee growth increased in **8** common bins with **64** supply changes, 4-bps in **2** bins with **104** supply changes, and 20-bps in **0** bins with **0** supply changes. Every sample retained monotone increasing Solana slot and zero decreasing fee counters. A run where active prices move beyond all sampled array IDs will be labeled **no_common_bins**, not falsely reported as zero fees.

**Dynamic-rate changes are FAST.** At 11:46 the 4-bps nominal pool was state-implied 11.03439 bps total; by 11:55 the stored variable component was **0.00899 bps**, total **4.00899 bps**. The 20-bps pool's variable component fell from ~1.18193 bps to ~0.00519 bps. The fee actually collected on individual swaps must be measured from events; extrapolating one instantaneous dynamic-fee state over an entire day is invalid.

The scheduled collection workflow uses GitHub Actions cache with unique run IDs and prefix-restore of the latest public-only Bin snapshot. It runs **every four hours**, with manual dispatch supported, and retains independently auditable public JSON artifacts for 30 days. GitHub Actions cache contains no credential-bearing URL or private wallet records. Cache eviction/missing snapshots are explicitly labeled baseline-only, never backfilled.

## 7. Audit other existing Solana SQLite datasets instead of duplicating history

[Successful GitHub Actions DB coverage audit 37926965433](https://github.com/evaristebernhard/arbitage/actions/runs/37926965433) fetched the actual existing Git LFS SQLite files and only emitted aggregate pool-specific counts:

| Existing SQLite source | Total records in relevant table | Target 1-bp | Target 4-bps | Target 20-bps |
| --- | ---: | ---: | ---: | ---: |
| solana_events.db / solana_pool_trades | **3,681** | 0 | **14** | 0 |
| solana_realtime.db / solana_pool_snapshots | **4,853** | 0 | **16** | 0 |
| solana_market.db / solana_ohlcv_candles | **10,145** | 0 | **1,003** | 0 |

These records were gathered under DIFFERENT historical observation windows than the current live fee-growth samples. The event database's 14 target-pool trades span a short, past interval; they cannot be used to reconstruct a continuously observed two-week volume profile and cannot be directly attributed to active Bin fee growth during 11:46–11:55. The OHLCV candles include bar aggregates, NOT user PositionV2 fee-share checkpoints.

Therefore the correct next quantitative step is a **time-aligned per-Bin fee-budget estimator**, with realized cumulative fee growth over several successive periods and real or hypothetical user liquidity-share paths, before solving KKT allocation. More historical market candles alone do not solve the missing fee-allocation problem.

The separate [coverage script](../research/dlmm_other_datasets_coverage.py) writes metadata only, never exports signatures, wallets or raw trade JSON. The [cross-snapshot verification script](../research/dlmm_compare_fee_growth.py) independently reproduces raw counter increments.
