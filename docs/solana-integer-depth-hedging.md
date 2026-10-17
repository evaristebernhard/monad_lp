# SOL option orderbook depth + integer LP hedging

Research-only, no keys, no trades, no writes to the runtime SQLite database.

## Model

Use the existing CLMM/CPMM HODL-relative loss D(S)=W0+V'(S0)(S-S0)-V(S).
Restrict to S in [0.8*S0,1.2*S0] and a single Deribit SOL_USDC expiry.
Each selected long option uses a listed strike, a nonnegative INTEGER count of contracts,
and the actual contract_size multiplier expressed in SOL. Only OTM puts and calls
are considered here; this is a deliberate model restriction, not global optimality.

Each orderbook ask level supplies [USDC per SOL, available SOL].
Compute cumulative premiums for 1,2,... whole contracts by integrating across ask levels.
Represent a whole contract purchase at marginal book cost with a binary variable.
Prefix constraints forbid consuming a deeper level without the preceding cheaper lots.
Do not infer orderbook depth from open interest or best ask alone.

Minimize ask-ladder premiums + indicative public fees + lambda * E,
subject to |D(S)-H(S)| <= E for EVERY S in the chosen risk interval.

## Exact continuous-price certification

On intervals between consecutive option strikes, the hedging payout is affine
and D(S) is convex. The maximum signed residual is at the endpoints, while
the maximum ABSOLUTE residual can occur at an interior minimum.
The inherited exact_error() function checks all endpoints and the analytic stationary
points (for CPMM S*=(L_eff/(x0-c))^2; for CLMM S*=(L/(x0+L/sqrt(B)-c))^2).
The mixed-integer solver starts with finite strike/end-point cuts and adds new constraints
when the exact oracle finds a violated price. The result is labelled risk_certified
only after the entire continuous interval meets the claimed E.
Solver status and MIP gap are recorded separately: risk certification is NOT an
unconditional certificate of globally minimal execution cost.

Zero purchased contracts are allowed and compared with all other strategies.
The economic objective does NOT include stochastic LP fees, collateral, funding,
dynamic hedging, execution latency, or a probability-weighted expected P&L.
Therefore no profit or positive APR conclusion follows from the result.

## Quote integrity

Deribit contract_size, min_trade_amount and lot_size are different fields.
See https://support.deribit.com/hc/en-us/articles/38507223482781-API-guidance
SOL_USDC multiplier and instrument rules:
https://support.deribit.com/hc/en-us/articles/31424932728093-Linear-USDC-Options

get_order_book is queried per instrument. These are NON-ATOMIC sequential books,
not simultaneous executable multi-leg quotes. Each output includes per-book timestamps.
Missing or failed depth reads are treated as zero available capacity.
Results are uploaded as GitHub Actions artifacts for 14 days.

## Reproduce

Install numpy and scipy, run both unittest discovery directories, then:

    python3 rpc/option/research/fetch_deribit_sol.py --days 7 --per-side 120 --output snap.json
    python3 rpc/option/research/fetch_sol_depth.py --input snap.json --output depth.json
    python3 backend/lp-options/research/integer_hedge.py --snapshot depth.json --kind cpmm --wealth 10000 --output answer.json

All outputs are experimental. Existing Rust hedge_analysis remains untouched.
Do not replace actual user CLMM ranges with the illustrative +/-20% interval in production.

## First verified GitHub Actions depth experiment — 2026-10-09

Source: [Actions run 37885383115](https://github.com/evaristebernhard/arbitage/actions/runs/37885383115), [raw artifact 11595798455](https://github.com/evaristebernhard/arbitage/actions/runs/37885383115/artifacts/11595798455). Time collected: **2026-10-09 04:45:54–04:46:00 UTC**.

- SOL_USDC index: **109.9363 USDC/SOL**.
- Single expiry: **2026-10-16 08:00 UTC**.
- Active instrument list: **124** options; **100** had two-sided summaries.
- **36** relevant OTM put/call instruments were fetched with 20-level order books; **36** had asks and **0** depth requests failed.
- All sample LP positions use exactly **10,000 USDC** capital and an illustrative **[0.8 S0,1.2 S0]** range. No actual wallet LP range was read.
- Risk objective was observed ask-ladder premium + indicative fees + **1 USDC of objective penalty per 1 USDC of worst absolute replication residual**.
- Integer size: **10 SOL** per option contract; zero lots allowed. The solver independently checked the exact continuous-price error.

| AMM | No-options worst residual | Integer option positions | Observed premium | Estimated fees | Final worst residual |
| --- | ---: | ---: | ---: | ---: | ---: |
| Raydium Standard (CPMM) | $55.73 | 2 contracts | $8.00 | $0.66 | **$24.90** |
| Fixed-range CLMM | $578.39 | 9 contracts | $93.00 | $2.97 | **$9.94** |

**CPMM selected contracts:** 1x SOL_USDC-16OCT26-96-P and 1x SOL_USDC-16OCT26-126-C. The ask on each was 0.40 USDC/SOL, multiplied by 10 SOL per contract. The reported model objective was ~$33.56, comprising $8.66 cost plus $24.90 worst residual.

**CLMM selected contracts:** one each of puts K=92,94,100,104,107 and calls K=113,116,123,129 (all same 2026-10-16 expiry). Model objective ~$105.91, comprising $95.97 cost plus $9.94 worst residual.

The optimizer result was **risk_certified** in both cases. SciPy MILP returned solver_status=0 for both; the reported MILP relative gap was 0 for CPMM and about 0.002 for CLMM, so cost optimality is only established within solver tolerance, under these long-only, OTM-only and displayed-depth constraints.

This is a striking improvement over rounding a predetermined ideal-strike hedge: the optimizer *reallocates* the integer lots to real listed strikes. It does **not** prove positive return, capture volume outside the stress interval, or guarantee all displayed levels are still available after sequential reads.

The fund's initial HODL-relative principal shortfall is not a loss forecast: the no-option worst case is a deterministic **adversarial price-scenario bound**. Avoid using the reported cost or maximum residual as LP APY or expected P&L.

### Risk-weight sensitivity (offline replay of the saved depth snapshot)

Changing the chosen penalty lambda changes the preferred amount of coverage:

| Capital | Model | lambda | Contracts | Cost incl. indicative fees | Worst residual |
| ---: | --- | ---: | ---: | ---: | ---: |
| $10k | CPMM | 0.1 | 0 | $0 | $55.73 |
| $10k | CPMM | 1 | 2 | $8.66 | $24.90 |
| $10k | CLMM | 0.1 | 2 | $6.66 | $472.74 |
| $10k | CLMM | 1 | 9 | $95.97 | $9.94 |
| $50k | CPMM | 1 | 4 | $44.32 | $12.96 |
| $50k | CLMM | 1 | 40 | $461.19 | $40.21 |

The last sensitivity table is an *offline replay* with the same collected order-book snapshot, not an extra GitHub Actions execution. Outcomes depend on the objective and assumed LP range, and higher trade size changes available depth.

## Main-branch verification and hard risk-budget replay

Main-branch [Actions run 37885555241](https://github.com/evaristebernhard/arbitage/actions/runs/37885555241), [artifact 11596023153](https://github.com/evaristebernhard/arbitage/actions/runs/37885555241/artifacts/11596023153), also succeeded. A second independent live capture at **2026-10-09 04:48:12 UTC** saw SOL index **109.9156** and 36 books with depth.

The **same** 10k CPMM model selected 96-P and 126-C, with ~$8.66 indicative cost and ~$24.97 exact interval error. The default 10k CLMM model again selected 9 contracts with ~$95.97 indicative cost, but the exact worst residual became **$10.44**, rather than $9.94 in the earlier 04:45 capture. This small spot move illustrates why a portfolio satisfying a hard $10 error cap at one snapshot need not continue satisfying it after the market moves.

An offline replay of this **second** snapshot with a genuine constrained requirement E<=10 USDC found:

- CPMM (long OTM SOL options, max 3 contracts per eligible strike): **infeasible** under the stated 10k / 20%-range assumptions.
- CLMM: **feasible**, 9 contracts, ~$97.97 indicative premium plus fee cost, **$9.54** exact worst residual.

That constrained solution is different from simply minimizing premium+lambda*E with lambda=1. The hard constraint is a conditional static contract selection requirement at the observation time, **not** a guarantee the hedge retains that error ceiling after spot, the LP position, or its book prices change.
