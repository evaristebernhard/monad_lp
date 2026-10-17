# Meteora DLMM SOL/USDC: discrete curvature and executable options research (2026-10-09)

## Data verified locally

Read-only local SQLite query on data/lp_screener.db returned 40,627 pools and these SOL/USDC DLMM catalog candidates:

| Pool | TVL USD (catalog) | 24h volume USD (catalog) | fee_bps field | bin_step | stored LP fee APR |
| --- | ---: | ---: | ---: | ---: | ---: |
| 5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6 | 4,270,504 | 49,176,004 | 4 | 4 | 159.72% |
| BGm1tav58oGcsQJehL9WXBFXF7D27vZsKefj4xJKD5Y | 3,532,715 | 12,378,860 | 10 | 10 | 121.50% |
| BVRbyLjjfSBcoyiYFuxbgKYnWuiFaF9CSXEa5vdSZ9Hh | 2,019,253 | 1,981,536 | 20 | 20 | 68.05% |

Catalog timestamps are old (Unix 1790837967, 2026-10-01 UTC); these are not current opportunities, realized fees or verified dynamic fee parameters. The fee_bps values in this file must not be interpreted as Meteora's real dynamic fee schedule. Live on-chain state and actual position bins must be read first. A prior remote GitHub Action obtained fresh pool-wide public TVL/volume for the first pool, but those observations do not reveal any specific LP's fee allocation.

## Mathematical distinction from CPMM/CLMM

For a passive fixed DLMM liquidity position in price bins K_i (ascending), assume a trade path under which each bin exchanges asset X (SOL) against quote Y (USDC) at a single fixed bin price. On each open interval between bin transition prices, let SOL inventory be x_i and quote inventory y_i, giving V(S)=x_i*S+y_i (mark-to-market). At a completed transition boundary K_i the inventory change satisfies y_{i+1}-y_i=K_i*(x_i-x_{i+1}), so V is continuous when crossing the boundary.

If increasing prices liquidate SOL, then x_{i+1}<=x_i. Put w_i=x_i-x_{i+1}>=0. Hence distributional gamma

    -V''(S) = sum_i w_i * delta(S-K_i).

Choose a reference S0 away from bin boundaries and the tangent/HODL benchmark B(S)=V(S0)+x(S0)*(S-S0). The exact nonlinear loss is

    D(S) = sum_{K_i<S0} w_i*(K_i-S)_+ + sum_{K_i>S0} w_i*(S-K_i)_+.

This is an EXACT ideal-vanilla-options decomposition of the principal loss, **not** a statement that real SOL option strikes, contract sizes or prices match bins. If the price reaches a boundary at which the asset conversion is partially filled, or a bin is actively recentered, use actual updated on-chain states rather than mechanically applying this static position model.

For the real Deribit chain, choose integer contract counts n_j, option multiplier h_j and listed strikes K'_j to minimize observed orderbook cash premium plus an absolute-error risk penalty:

    min sum_j C_j(n_j) + lambda * E
    subject to |D(S)-sum_j h_j*n_j*phi_j(S,K'_j)|<=E for all S in risk interval.

Here BOTH D and the option payoffs are piecewise affine. Therefore the **exact** maximum absolute residual on a bounded risk interval lies at the interval endpoints and at the union of (i) DLMM bin transition prices and (ii) selected option strikes. Unlike continuous CLMM, there are no interior smooth stationary points to examine. This can make a DLMM-specific MILP smaller and exactly finite without iterative nonlinear cut discovery.

With an ideal one-option-per-bin decomposition the error is zero, subject to an ideal strike at every K_i and fractional SOL exposure. Market strike gaps and 10-SOL contracts break this ideal identity. Nearby bins can be grouped via moment matching: q=sum_{i in group} w_i and k=sum_i w_i*K_i/q. Jensen gives a conservative one-option hedge; exact residual is evaluated at every bin and option strike. The optimal grouping depends on the *traded strike lattice*, orderbook liquidity and premium cost, not merely bin count.

## Fee accounting: why high DLMM APR is not enough

Meteora DLMM trading fees can be dynamic and bin-dependent. A user earns fees only on liquidity actually available and executed in the active bin(s), adjusted by the protocol's fee ownership. A proxy based on total pool daily volume, assumed gross fee_bps and W0/TVL can be substantially wrong. Correctly model the timestamped sequence of active bin, variable fee, bin-specific active liquidity, position bin weights, and changes of inventory; add swaps and protocol reward payouts as separate flows.

Terminal excess net P&L (vs initial HODL) is

    realized LP fees + realized rewards - D(S_T) + options payoff(S_T) - paid premium - commissions - hedge rebalance/settlement costs.

Funding and required collateral must be included if a perp hedge is used. Static options only address terminal principal loss; dynamically changing position bins or fees require a pathwise replay. This model currently describes a passive principal benchmark, not an executable trading recommendation.

## Implemented read-only account probe

`rpc/tools/meteora-dlmm-live-probe.py` now decodes the checked-in Meteora DLMM
account layouts for `LbPair`, `PositionV2`, `BinArray`, and SPL mint decimals.
It exposes the pool's static/dynamic fee inputs and, when a position is
provided, splits each bin's `amount_x`/`amount_y` by
`position_liquidity_share / bin_liquidity_supply`:

```bash
python3 rpc/tools/meteora-dlmm-live-probe.py \
  --pool 5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6 \
  --position <POSITION_ACCOUNT>
```

`--owner <OWNER>` can be used instead of `--position` to discover all matching
user-scoped PositionV2 accounts through `getProgramAccounts`. With neither
argument, the probe intentionally returns a pool-only snapshot; it does not
scan every position in the pool. The output is sanitized: it
contains no upstream RPC URL or credential. If token X is WSOL, each enriched
bin also includes `delta_sol`, which is the input weight for the rising-price
orientation used by the hedge model. Positions wider than 70 bins use the
IDL's dynamic `PositionBinData` extension; legacy Position accounts are
intentionally rejected rather than decoded with an unverified layout.

The pool fee output is the raw on-chain state required to calculate base and
variable fees: `base_factor`, `base_fee_power_factor`, `variable_fee_control`,
`max_volatility_accumulator`, `volatility_accumulator`, protocol share and the
fee timing parameters. A single snapshot is not realized LP income; fee P&L
still requires synchronized swap events, active-bin liquidity and position fee
growth checkpoints.

## Dedicated LP + Deribit pool database

The collector stores SOL/stable Meteora DLMM observations separately from the
general catalog and RPC databases:

```bash
python3 rpc/tools/collect_meteora_dlmm_deribit.py --limit 40
```

The default database is `data/sol_dlmm_deribit.db`. It keeps a pool registry,
timestamped `pool_snapshots` with active bin and fee parameters, and
`bin_snapshots` with each initialized Bin's price, reserves and liquidity
supply. The collector chooses the highest-volume SOL/USDC, SOL/USDT and
SOL/USDS candidates from the local catalog. Pool state is saved even when an
RPC provider rejects the larger BinArray query; those rows are marked
`pool_only` with an error stage. The database is checked into Git LFS so the
research snapshot survives local cleanup and can be pulled on another machine.

At the latest collection on 2026-10-09 08:23 UTC, all 23 candidate pools had
complete on-chain Bin snapshots: 1,053 BinArrays and 30,213 populated bins.
The median active-bin price was $110.372/SOL (median restricted to $90-$120:
$110.370); the simple mean was pulled up to $114.547 by one thin USDT/SOL pool
at $198.455, which remains an outlier requiring validation. The Deribit index
in the nearby option sample was $110.249, about 0.11% below the normal-range
pool median. Four of the 23 pools had zero catalog 24-hour volume, and the
local catalog volume fields are dated 2026-10-01, so neither the catalog
ranking nor these pool prices should be treated as current executable volume
or a mark-price oracle. Bin distributions remain pool-wide, not any
individual LP position.

Deribit option quotes and ask-side depth append to the same database:

```bash
python3 rpc/tools/collect_deribit_sol_dlmm.py \
  --days 7 --per-side 250 --risk-low 0.8 --risk-high 1.2 --depth 20
```

This stores one common SOL_USDC expiry, index price, listed strike metadata,
bid/ask and IV, plus ask levels for out-of-the-money options inside the risk
band. Each option snapshot records the nearest Pool snapshot and its time
offset; the venues are sampled sequentially, not atomically. It uses only
Deribit's public market-data API and places no orders.

The first two saved option snapshots (2026-10-09 08:22:33 and 08:23:53 UTC)
each contain 112 strikes for the 2026-10-23 08:00 UTC expiry (56 calls and 56
puts, strikes $70-$165). The second snapshot has 102 two-sided quotes and 34
of 34 requested ask books with 95 ask levels, linked to pool snapshot 69 at a
44.9-second offset. The chain's mean mark IV is 66.05% across all strikes;
near-ATM ($110-$112) mark IV is about 53.7%-53.9%. Mean relative spread across
the two-sided quotes is 18.19%, so midpoint/mark valuations can materially
understate executable cost. Ask depth is reported in underlying SOL, consistent
with Deribit's option quantity units; the instrument's 10-SOL contract size and
10-SOL minimum trade amount are separate metadata and must be respected by an
optimizer. See Deribit's [order-book API](https://docs.deribit.com/api-reference/market-data/public-get_order_book)
and [instrument-size guidance](https://support.deribit.com/hc/en-us/articles/38507223482781-API-guidance).
The 95 level amounts are across different options and are not additive hedge
capacity for one payoff. Quotes are a point-in-time, non-atomic snapshot, not
an execution guarantee.

## Three-day historical market data (2026-10-06 to 2026-10-09)

`rpc/tools/backfill_dlmm_deribit_history.py` backfills the prior 72 hours into
the same database. Pool candles come from the pool-address-specific
[GeckoTerminal OHLCV API](https://api.geckoterminal.com/docs/index.html),
requested in USD; Deribit history comes from its public
[instrument trade-history API](https://docs.deribit.com/api-reference/market-data/public-get_last_trades_by_instrument_and_time).
Run the sources separately to respect the public pool-data rate limit:

```bash
python3 rpc/tools/backfill_dlmm_deribit_history.py --hours 72 --only-pools --missing-pools --pool-pause 15
python3 rpc/tools/backfill_dlmm_deribit_history.py --hours 72 --only-deribit --deribit-pause 0.5
```

The completed backfill contains 1,253 pool candles across 21 of 23 pools;
seven pools have a full 72 hourly candles and two returned no candles. The
primary pool `5rCf...HAS6` has a complete window from 2026-10-06 08:00 through
2026-10-09 07:00 UTC: 72 hourly closes, 71 hourly log returns, about $97.0M
reported pool volume, and a close-to-close move from $120.416 to $110.483
(-8.25%). The observed hourly low/high were $105.659/$121.934. The standard
deviation of hourly close log returns scaled by sqrt(24) was 3.29% (24-hour
realized-volatility scale; not annualized).

The same 72-hour interval was queried for all 112 options in the saved
2026-10-23 SOL_USDC expiry. The API returned 93 executions across 20 contracts
(5,630 SOL of reported option trade amount); observed trade IV ranged from
47.36% to 67.09% with a 54.33% mean. Each returned trade includes its actual
execution price and, when supplied, index price, mark price, and IV. Deribit
defines option trade amount in the underlying coin, so the amounts are SOL.
See the API's [trade response fields](https://docs.deribit.com/api-reference/market-data/public-get_last_trades_by_instrument_and_time).

Interpretation boundary: GeckoTerminal's pool-address OHLCV is a pool trade
price/volume history, not a historical DLMM `active_id`, Bin reserve snapshot,
or an LP's BinArray share. Coverage varies by pool and is stored as returned;
missing hours are not synthesized. Deribit historical trades are executions
by market participants, not archived bid/ask ladders or guaranteed hedge
fills. This improves the market-path evidence but still does not provide
historical position-specific fees, executable option entry costs, or net LP +
Deribit P&L.

## Remaining implementation boundary

1. Provide a real PositionV2 address or owner to derive one LP's shares across the collected pool bins. Current Bin data is pool-wide.
2. Build a sorted finite breakpoint array from bin prices and same-expiry Deribit option strikes. Derive maximum |D-H| from these breakpoints exactly.
3. Feed real ask-ladder depth and whole SOL_USDC contracts to the integer MILP. Report no-hedge versus hedged residual and costs.
4. For realized yield, replay real swaps and changes in active bin and liquidity on synchronized timestamps, and compare to actual position fee-growth deltas. Do not fabricate a seven-day backtest from short coverage.

No destructive reset, credential disclosure, or live trade occurred. Pool
and option collectors are read-only and do not place orders.
