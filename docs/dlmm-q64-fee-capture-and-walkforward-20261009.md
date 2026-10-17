# Meteora DLMM fee-growth measured in USD: real Bin counters, shadow LP positions and first walk-forward test

**Study as of 2026-10-09 12:12 UTC. Research only: on-chain **public Bin fee counters are real**, but the evaluated $2,000 and $10,000 LP positions were **NEVER deposited**, and their fee capture is a *market-impact-free counterfactual*. Do not call their dollar estimates actual user earnings, a backtest PnL or forward APR.

## 1. Provenance and completed checks

Successful Actions [37928429533](https://github.com/evaristebernhard/arbitage/actions/runs/37928429533), [artifact](https://github.com/evaristebernhard/arbitage/actions/runs/37928429533/artifacts) used five previous successful public Solana RPC snapshots; no user RPC DB, wallet or key was read in the USDC estimator itself:

- 2026-10-09 11:46:49 UTC, slots ~454857042–44, [raw baseline](https://github.com/evaristebernhard/arbitage/actions/runs/37925823991)
- 11:50:22 UTC, slots ~454857839–40, [raw](https://github.com/evaristebernhard/arbitage/actions/runs/37926179416)
- 11:54:06 UTC, slots ~454858668–71, [raw](https://github.com/evaristebernhard/arbitage/actions/runs/37926563418)
- 11:55:00 UTC, slots ~454858874–76, [raw](https://github.com/evaristebernhard/arbitage/actions/runs/37926659902)
- 11:58:34 UTC, slots ~454859675–78, [raw](https://github.com/evaristebernhard/arbitage/actions/runs/37927033686)

The four adjacent observation intervals are ~213s, ~224s, ~54s and ~213s, total 704s. Every sample includes actual public on-chain cumulative LP fee-per-token counters, reserves and liquidity supply for five near-active 70-Bin arrays, for each of the canonical SOL/USDC 1-bp, 4-bps, 20-bps pools.

## 2. Exact integer Q64 arithmetic used for fees

Meteora cumulative fee_amount_x_per_token_stored and fee_amount_y_per_token_stored are U128 fixed-point values; the on-chain claim approximation per liquidity share L, excluding old pending fees, follows:

    raw_new_fee = ((L >> 64) * (G_new - G_complete)) >> 64

with INTEGER truncation at each right shift. See [published Meteora claim fee calculation](https://docs.rs/waterpump-solana-meteora-dlmm-client/latest/waterpump_solana_meteora_dlmm_client/math/fee/fn.calculate_bin_fees.html). Negative deltas, changed pool identity, incorrect canonical SOL/USDC orientation and non-forward slots are rejected. The exact Q64 shift/rounding was unit tested.

For *all original LPs* in Bin i, one can estimate how much the observed cumulative fee grew by applying that formula to its total **existing liquidity supply**. Because LPs added/removed liquidity between snapshots, neither start nor end liquidity supply necessarily equals liquidity over the interval. The reported midpoint-of-endpoint-supplies value is therefore a **sensitivity proxy**, NOT a provable lower/upper bound on true earned fees. Fees stored in the per-Bin growth counters are already after the protocol's fee allocation: **do not multiply by a second 90% share**.

The decimals are 9 for SOL and 6 for USDC. SOL is valued using the recorded active Bin index and bin_step as a coarse spot proxy. Actual external SOL oracle pricing / within-Bin variability is not present. This gets especially inaccurate for a 20-bps Bin with unchanging active ID.

## 3. Actually measured pool fee-growth in selected 350-bin windows

Using midpoint observed supply for each Bin and valuing X=SOL at the corresponding entry Bin-price proxy:

| Historical interval | 1-bp: total sampled LP fee | 4-bps: total sampled LP fee | 20-bps: total sampled LP fee |
| --- | ---: | ---: | ---: |
| 11:46:49 → 11:50:22 (~213s) | **$6.12** | **$111.33** | **$61.76** |
| 11:50:22 → 11:54:06 (~224s) | $1.79 | $30.67 | $0.00 |
| 11:54:06 → 11:55:00 (~54s) | $0.32 | $5.23 | $0.00 |
| 11:55:00 → 11:58:34 (~213s) | $0.72 | $5.19 | $0.00 |

These are **estimated aggregate public LP fees accruing within only the observed BinArrays**, not the entire pool's realized fee total, volume, daily fee or any user's profit. Positive-counter Bin counts were respectively 67/15/2 in the first interval, then 33/5/0, 8/2/0, 10/3/0.

The largest 4-bps interval $111 versus 1-bp $6 does **not** establish 4-bps LP alpha, because distribution of fee flow and new liquidity share matters. The high-volume pools undergo large supply changes even during a four-minute observation interval.

## 4. Fixed 70-Bin SHADOW deposit held for ~704 seconds

Construct a hypothetical $10k LP at 11:46 UTC, equally investing $5k into 35 USDC-only bins below initial active price, and $5k into 35 SOL-only bins above it. It must be constructed as a suitable contiguous 71-ID PositionV2 or multiple actual position instructions (the active Bin is omitted); no actual position is minted here.

The synthetic new liquidity share q_i is estimated via initial marked capital C_i and total liquidity supply L_i as q_i ≈ L_i*(w_i/C_i). For each subsequent interval, use that *same fixed q_i* and estimate its fractional share of old public Bin fee volume as q_i/(L_i,avg+q_i), where L_i,avg is the average of the two recorded incumbent liquidity supplies. This adjusts for observed competitor LP entry/exit but **assumes our new deposit would not affect swaps, the fee state, competing LP decisions or future fee growth**. Fee values are public counter movements; the hypothetical fee ownership is modeled.

The ideal static HODL-relative loss D(S_final) is evaluated using *price implied by the initial and final active Bin IDs*; this omits within-bin price movement and real path-dependent composition/rebalance effects.

| Not-actually-invested starting capital | Pool | Hypothetical fee capture | Ideal divergence vs initial HODL | Fee − divergence, before costs |
| --- | --- | ---: | ---: | ---: |
| $2,000 | 1-bp | $1.327 | $3.394 | **−$2.067** |
| $2,000 | 4-bps | $0.662 | $0.628 | **+$0.035** |
| $2,000 | 20-bps | $0.108 | $0.000* | **+$0.108*** |
| $10,000 | 1-bp | **$3.567** | **$16.968** | **−$13.401** |
| $10,000 | 4-bps | **$3.250** | **$3.138** | **+$0.112** |
| $10,000 | 20-bps | $0.533 | $0.000* | **+$0.533*** |

*The 20-bps pool remained on the SAME active Bin ID across the observation window. Its output zero loss is a **price-quantization artifact**, NOT evidence that SOL did not move or LP inventory risk was zero. Do not interpret the positive number as actual/expected profit.

In this illustrative sample, 1-bp overconcentrated capital lost more to fee-free inventory conversion than its shadow fees; the 4-bps case was almost breakeven (a few cents to a tenth of a dollar per $2k-$10k). Any composition fee, slippage, within-Bin spot, withdrawal cost, or change in flow assumptions can reverse the result. No USD-directional SOL/USDC principal risk is protected by these HODL-relative figures. **704 seconds is not a meaningful holding-horizon return estimate.**

## 5. A small chronologically held-out fee allocation test

The earlier KKT proof in [fee-Gamma theory](lp-fee-gamma-lvr-analytic-theory-20261009.md) states that with *exogenously known* future per-Bin fee budgets A_i, incumbent capital C_i and per-dollar risk ell_i,

    w_i^* = [sqrt(A_i*C_i/(lambda+ell_i)) - C_i]_+

under a limited budget and option-to-stay-in-cash. The challenge is estimating A_i *before* the price visits a Bin.

[research/dlmm_fee_walkforward.py](../research/dlmm_fee_walkforward.py) takes **only the previous interval** fee-growth to determine a KKT allocation at the next entry time, with a nominal fixed 240-second risk horizon at assumed flat risk-neutral volatility of 53.8%; the following interval's public fee counters and active price are used strictly for evaluation. The 70 equal-funded-Bin alternative is selected without seeing future flows too. **These are two market-impact-free shadow positions, not a backtest of actual minted capital.**

$10k shadow fee minus ideal divergence in three held-out intervals:

| Pool | Forward-window KKT heuristic | Equal-dollar 70-Bin benchmark |
| --- | --- | --- |
| 1-bp | **+$0.0499, +$0.0116, +$0.0737** | **+$0.6940, +$0.0093, +$0.1160** |
| 4-bps | **$0, $0, +$0.0318** | **+$0.3439, +$0.1056, +$0.0022** |
| 20-bps | **$0, $0, $0** | **$0, $0, $0** |

This naive past-fee hotspot policy beat the equal-Bin comparator in only one of three 4-bps windows and only one of three 1-bp windows. With a sample this short, there is no statistically meaningful advantage to either policy. For 20-bps, most forward samples were not fee-generating at the sampled bins; the zero-D from coarse active-price observations makes P&L comparisons especially uncertain.

**Mechanism:** high-growth Bin IDs migrate when SOL price shifts. A_i(t) cannot be treated as a time-stationary exogenous fee budget! Flow becomes endogenous to current active Bin, external routing, volatility accumulator and competitor liquidity. A smarter forward model must predict fee flow *conditional on price path/active Bin transitions*, not allocate based only on last snapshot's fee heat map.

## 6. The next theory upgrade actually suggested by the data

Instead of forecasting a fixed A_i, introduce a state-dependent fee intensity:

    A_i(t,t+dt) = E[ fee flow at Bin i during dt | active_id(t), S(t),
                      volatility_accumulator(t), fee_config, competing liquidity,
                      route order flow, recent flow features ].

The per-Bin economic objective becomes

    max_w  E[ integral_0^T sum_i ( w_i/(C_i(t)+w_i) ) dA_i(t)
             - D_w(S_T) - execution, composition and rebalance costs ],

subject to range, amount, pool account, inventory and drawdown constraints.

The KKT closed form remains a useful **conditional subproblem** only if fee flow and risk costs are treated as fixed input curves; the real optimization is dynamic with feedback because A_i depends on price and liquidity. More time-aligned real data and a price feed finer than 20 bps are needed before claiming alpha.

## Source files and reproducibility

- [Q64 fee-growth/value/70-Bin interval script](../research/dlmm_q64_fee_capture_study.py)
- [Fixed-share dilution-aware shadow ledger](../research/dlmm_fixed_bin_cohort.py)
- [Simple chrono out-of-sample KKT fee hotspot experiment](../research/dlmm_fee_walkforward.py)
- [Read-only repeatable GitHub Actions](../.github/workflows/dlmm-q64-fee-capture.yml)

Action downloads public artifacts from prior successful on-chain RPC research and runs all Python calculations in a clean Ubuntu runner. No private RPC URLs, user accounts, seed phrases or trading code is involved. These inputs are not the user's actual LP position data.
