# LP RPC refresher

`lp-refresh` is the Rust data-fetching worker for the LP registry. It reads
pool candidates from `data/lp_screener.db`, batches read-only RPC calls, and
upserts the latest chain state into the SQLite `pool_rpc_state` table. When a
protocol account contains authoritative token mints, the worker also fills
missing `pools.token_a_address/token_b_address` references so the price layer
can maintain history.

The shared worker contracts live in `src/model.rs`. The worker remains an
independent workspace and never imports the backend API or frontend.

## Run

```bash
# all configured EVM chains
cargo run --manifest-path rpc/workers/lp-refresh/Cargo.toml -- \
  --batch-size 100 --concurrency 16

# one chain, useful for scheduled/independent refresh jobs
cargo run --manifest-path rpc/workers/lp-refresh/Cargo.toml -- \
  --chain polygon --batch-size 100 --concurrency 16
```

The worker accepts `--sqlite`, `--chain`, `--limit`,
`--state-status`, `--maintenance-only`, `--priority-only`, `--batch-size`, `--concurrency`,
`--duration-secs`, `--interval-secs`, `--history-hours`, `--history-only`, and `--metrics`. RPC endpoints are loaded from the local
catalog or through the local RPC proxy. It does not read RPC source files or
CSV catalogs at runtime. Credentials are never written to the database or
committed to the repository.

To backfill a complete seven-day hourly window, use the historical mode. For
each EVM chain it estimates the block nearest to every hour, calls the pool
decoder with that explicit historical block tag, and writes the decoded ratio
to `data/lp_price_history.db` (`pool_price_history` with
`source='rpc_pool_historical'`). A pool is not
considered complete until it has 168 priced points; unsupported or unavailable
states are skipped rather than copied forward. The mode requires an archive-
capable upstream RPC for the selected chain:

```bash
cargo run --manifest-path rpc/workers/lp-refresh/Cargo.toml -- \
  --history-only --history-hours 168 \
  --batch-size 40 --concurrency 8 --limit 0
```

`--history-hours 168` can also be combined with a normal refresh. The worker
then refreshes the latest state and backfills only pools that still have fewer
than 168 priced hourly observations. Use `--chain` and `--limit` to stage a
large universe by chain or by batch. Solana historical account reads are not
fabricated: those pools receive points from the recurring live sampler only.

Use `--state-status transport_error` or `--state-status rpc_error` to retry
only a failed class without paying for another full refresh. The filter reads
the current status from `pool_rpc_state` and can be combined with `--chain`:

```bash
cargo run --manifest-path rpc/workers/lp-refresh/Cargo.toml -- \
  --chain ethereum --state-status transport_error \
  --batch-size 5 --concurrency 1
```

By default the worker runs one cycle. For a continuous one-hour observation
window, refresh the selected universe every 15 minutes and write live metrics:

```bash
cargo run --manifest-path rpc/workers/lp-refresh/Cargo.toml -- \
  --duration-secs 3600 --interval-secs 900 \
  --batch-size 40 --concurrency 4 \
  --metrics lp_refresh_latest.json
```

Each completed batch is committed to `pool_rpc_state`, so readers can observe
partial progress while a full cycle is still running. `--limit` now selects
the highest-liquidity/highest-volume pools first instead of database insertion
order. `--maintenance-only` limits selection to enabled rows in the
`lp_maintenance` table. The metrics file is consumed by the Rust API at
`/api/rpc-refresh`.

`priority_pools` is a separate fixed universe of up to 1,000 high-liquidity
pools; it is unrelated to the user watchlist. Run `--priority-only` for this
universe. Priority refreshes use an explicit EVM block tag captured once per
chain instead of `latest`, and commit the whole selected set in one SQLite
transaction. The normal full refresh excludes these rows so it cannot break a
priority snapshot halfway through a cycle.

```bash
cargo run --manifest-path rpc/workers/lp-refresh/Cargo.toml -- \
  --priority-only --batch-size 40 --concurrency 4 \
  --duration-secs 3600 --interval-secs 900 \
  --metrics priority_refresh_latest.json
```

For EVM pools it reads `token0`, `token1`, `slot0`, `liquidity`, `fee`, and
`getReserves` with `eth_call`, and records the latest block. Dedicated EVM
decoders currently include:

- Uniswap V4: `StateView.getSlot0(poolId)` and `getLiquidity(poolId)` on the
  chain's official StateView deployment.
- Algebra-compatible concentrated pools (Camelot V3, Blackhole V3, Thena V3,
  QuickSwap V3): `globalState()`, `liquidity()`, `token0()`, and `token1()`.
- Ramses-compatible concentrated pools (Etherex) and Uniswap V3-compatible
  Honeypop pools use the standard `slot0()`/`liquidity()` path after their
  database family is classified as `concentrated_v3`.
- Maverick V2: `getState()`, `fee(bool)`, `tokenA()`, and `tokenB()`; the state
  decoder keeps reserves, TWAP fields, active tick, bin counter, and both
  directional fees.
- Balancer WeightedPool: `getVault()`, `getPoolId()`,
  `getNormalizedWeights()`, and `getSwapFeePercentage()`.
- Fluid DEX: `readFromStorage(0)` and `readFromStorage(1)` for the packed DEX
  variables and fee state. The raw words are retained in `state_json` so the
  price/fee bit layout can be extended without losing source data.
- PancakeSwap StableSwap: Curve-compatible `coins(uint256)`,
  `balances(uint256)`, `fee()`, and `get_virtual_price()` reads.
- SyncSwap V2.1 classic pools: the constant-product `getReserves()` path.
- Ekubo V3: a direct read of the pool-id storage slot on the official singleton
  core; packed sqrt ratio, signed tick, and active liquidity are retained.

If an upstream gateway rejects a large JSON-RPC request with a transport/HTTP
error, the worker bisects that request until the failing call is isolated. A
single failed selector therefore becomes a pool-level decode error; it does
not turn every pool in the original batch into `transport_error`.

These protocol-specific calls are never sent to the pool as generic V2/V3
selectors. For Solana, `getAccountInfo` is decoded by protocol family:
Raydium CLMM (`raydium-clmm-account-v1`), Raydium CPMM
(`raydium-cpmm-account-v1`), Orca Whirlpool (`orca-whirlpool-account-v1`),
and Meteora DLMM (`meteora-dlmm-lbpair-account-v1`). The current account
decoder records the mint addresses and fixed-point price/liquidity or bin/fee
parameters available in the pool account. Raydium CPMM performs a second
read for `token_0_vault/token_1_vault` and stores their raw SPL balances so
constant-product pools can produce a spot price. Tick arrays, active bins,
and Raydium config fees remain separate follow-up reads because they are
independent accounts. RPC permission or transport failures are recorded
distinctly from `account_missing`/`rpc_error`; a transient transport or
missing-account result does not overwrite an existing live state. This keeps a
free-tier gateway's partial response from immediately erasing a usable price
anchor; an explicit successful decoder result can still replace it.

The protocol decoder design and SDK references are tracked in
[`docs/pool-decoder-sdk-map.md`](../../docs/pool-decoder-sdk-map.md). The
worker deliberately does not send V2/V3 selectors to protocol families that
need a dedicated decoder, such as Curve, Balancer, Liquidity Book, or V4.

The Rust API exposes the aggregate state at `/api/rpc-state/summary`.
