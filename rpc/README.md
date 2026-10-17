# Local RPC layer

`rpc/` is the independent third layer between the backend and upstream chain
RPCs:

```text
frontend -> backend (lp-api / lp-refresh) -> local-rpc -> upstream RPCs
```

SQLite is the runtime source of truth. There is no JSON runtime catalog and no
dependency on the old RPC text files or Python registry.

The RPC layer stops at raw chain reads and price/history transport. It does not
calculate IV or volatility. The backend reads the stored raw price history,
builds token A/token B pair prices, and computes averages, APR, IV and the
1d/3d/7d realized volatility; the frontend only renders those backend fields.
The realized volatility is calculated by the backend from log returns. A single
hourly move is exposed as a low-sample dailyized estimate; the API labels it as
low-sample instead of presenting it as a mature series.

### Atomic pool price reads

The current-price interface is per pool. It does not require `lp-refresh` or a
pre-existing `pool_rpc_state` row:

```bash
curl -s http://127.0.0.1:8545/price/solana-mainnet/<POOL_ADDRESS> \
  -H 'content-type: application/json' \
  --data '{"pool_type":"solana_standard","token0_symbol":"NFLX","token1_symbol":"NUTFLEX","token0_anchor_usd":68.81}'
```

The RPC layer reads only the requested pool account and its required mint/vault
accounts. The response reports `rpc_anchored` when an absolute USD anchor is
available, or `relative_only` with `token1_per_token0` otherwise. Relative-only
pools still produce pair volatility; only absolute token USD volatility requires
an USD anchor. A refresh is just a composition of many atomic pool reads.

## Run

From the repository root:

```bash
cargo run --manifest-path rpc/Cargo.toml --bin local-rpc
```

The default database is `rpc/data/rpc.db`; override it with `RPC_DB_PATH`.
The database is created from [schema.sql](db/schema.sql) when it does not yet
exist. The default listener is `127.0.0.1:8545`.

### Optional Solana WebSocket listener

The local RPC service can also run a reconnecting logsSubscribe listener. It
is disabled by default. A token can have many LPs, so the preferred mode
discovers its pools from the local Solana pool catalog and subscribes to the
selected pools:

    SOLANA_LISTENER_ENABLED=true \
    SOLANA_LISTENER_TOKEN=<token-mint> \
    SOLANA_LISTENER_POOLS_PER_PROTOCOL=2 \
    cargo run --manifest-path rpc/Cargo.toml --bin local-rpc

Pools are ranked by liquidity and then 24-hour volume independently inside
each protocol. The default is two pools per protocol. Set
SOLANA_LISTENER_POOLS_PER_PROTOCOL=0 to include every matching catalog pool,
or use
SOLANA_LISTENER_POOL_ADDRESSES for an explicit comma-separated pool list.
SOLANA_LISTENER_POOL_DB overrides the default data/lp_screener.db catalog.
SOLANA_LISTENER_MENTION remains available for a single pool or program
address.

The listener also exposes read-only query endpoints backed by the SQLite
event database:

    GET /solana/events?token=<mint>&side=buy&min_usd=10000
    GET /solana/whale-alerts?token=<mint>&min_usd=10000
    GET /solana/pool-history?pool=<pool-address>&hours=24
    GET /solana/protocols

The first endpoint returns recent enriched events. Whale alerts filter those
events by the configured `SOLANA_LISTENER_WHALE_USD` threshold. Pool history
returns locally classified swaps and derived OHLCV data.

Event records are persisted in `rpc/data/solana_events.db` (override with
`SOLANA_LISTENER_EVENT_DB`). The recent event table is bounded. Whale alerts
live in their own durable `rpc/data/solana_whales.db` database (override with
`SOLANA_LISTENER_WHALE_DB`), so event pruning does not remove alert history.
On first startup after this split, alerts migrate from the event database and
the JSONL log. The web listener surface can resolve a pool address to its
catalog token and start or reuse the listener task while local-rpc is running.

After a pool event, the listener schedules a fresh `getAccountInfo` snapshot
for that pool. The per-pool cooldown defaults to 15 seconds and is controlled
by `SOLANA_LISTENER_POOL_REFRESH_SECS`. These snapshots are written to the
separate SQLite database `rpc/data/solana_realtime.db` (override with
`SOLANA_LISTENER_REALTIME_DB`), so high-frequency listener writes do not contend
with the main `data/lp_screener.db` catalog or price-history writes. The latest
snapshot is available through `GET /solana/pool-snapshots`; the web screen uses
its timestamp for the pool's Updated column. For catalog pools with a known
`pool_type`, the listener also calls the local RPC account decoder and stores a
`decoded_price` object for the no-swap current-price fallback.

The listener reuses the enabled solana-mainnet catalog endpoints, converts
their HTTP schemes to WebSocket schemes, sends one logsSubscribe request per
selected pool on the connection, rotates to the next endpoint after a
disconnect, and writes one raw JSON event per line to
rpc/data/solana_logs.jsonl. Override the output path with
SOLANA_LISTENER_OUTPUT. An explicit comma-separated SOLANA_LISTENER_WS_URL can
be used when a provider has a dedicated WebSocket URL. SOLANA_LISTENER_ALL=true
is available for broad monitoring, but should only be used with an intentional
retention plan. With enrichment enabled by default, every new signature is
looked up with getTransaction and the output includes token balance deltas,
mint addresses, token instructions, program IDs, slot, block time and failure
status. Set SOLANA_LISTENER_ENRICH=false to keep only raw WebSocket events.
For swaps with a recognized quote mint, enrichment also calculates
`swap_price_usd` and `swap_ratio_quote_per_target` from the balance deltas;
the latter remains useful even when no USD quote is available. Trade ownership
is inferred from the wallet owner whose target and quote balances move in
opposite directions; the transaction signer is treated as a router/aggregator
and is not assumed to be the trader. Watched pool accounts are excluded from
that wallet candidate set. SOL quote notional is valued from the best local
SOL/USDC or SOL/USDT catalog price unless `SOLANA_LISTENER_SOL_PRICE_USD` is
provided explicitly.

Classified swaps are stored in `solana_pool_trades` inside the event database.
`/solana/pool-history` returns those trades and derives local candles. The API
persists queried pools and provider OHLCV in `rpc/data/solana_market.db`, a
separate market database. Pool account snapshots in `solana_realtime.db` are
kept as historical rows; only `solana_pool_latest` is one row per pool.

`GET /solana/protocols` documents the supported catalog pool types and their
program IDs: Meteora DLMM, Orca Whirlpools, and Raydium standard/CLMM pools.
The protocol-specific event decoders live in separate modules:

- `rpc/src/solana_decoders/meteora_dlmm.rs`: bin-position liquidity, swaps,
  rebalance, fees and rewards.
- `rpc/src/solana_decoders/orca_whirlpool.rs`: position open/increase,
  decrease/close, swaps, fees, rewards and tick-array maintenance.
- `rpc/src/solana_decoders/raydium.rs`: CPMM/legacy AMM deposit/withdraw and
  CLMM position liquidity, swaps and pool status operations.

They match both program IDs and the raw base58 instruction discriminator from
`getTransaction`, including nested CPI instructions. The implementation keeps
the useful generated-decoder pattern from Carbon in local code rather than
adding Carbon as a runtime dependency: Anchor-style 8-byte discriminators for
DLMM/Whirlpool/CLMM/CPMM and the legacy one-byte Raydium AMM v4 tags. Logs are
still a compatibility fallback for providers that omit raw instruction data.
A decoded event records `protocol_instruction`, `protocol_explanation`,
`decoded_protocol` and a protocol-specific `event_kind`. If the program matches
but neither raw data nor fallback logs identify the instruction, the event is
explicitly marked `protocol_event_unresolved`; the listener no longer guesses
that a no-delta transaction was maintenance.
If `logsSubscribe` matched a pool address but the transaction did not invoke
the watched pool program at all, it is marked `unrelated_router_event` and
excluded from the user-facing event feed.

### Raw token price worker

The price worker is a separate binary in the same independent RPC workspace.
It keeps catalog metadata in `data/lp_screener.db` and writes raw observations
to the separate `data/lp_price_history.db`:

```bash
cargo run --manifest-path rpc/Cargo.toml --bin rpc-price -- \
  --db data/lp_screener.db --price-db data/lp_price_history.db \
  --limit 0 --batch-size 50 --workers 8 --span-days 30
```

`--price-db` is optional; it defaults to the catalog database directory's
`lp_price_history.db` and can also be set with `LP_PRICE_DB`. On first run,
`rpc-price --schema-only true` migrates the three legacy price tables from the
catalog database into the separate price database without contacting RPCs.

For each token, the worker first requests sparse historical observations from
DefiLlama when available. Missing tokens fall back to the latest raw protocol
state in `pool_rpc_state`, using the decoder output, token decimals and stable
price propagation. This fallback is a current observation, not fabricated
historical data. The worker stores `source=defillama` or
`source=rpc_pool_state` in `token_price_history`; RPC observations are sampled
once per hour and are no longer deleted on the next run. The table is pruned to
the requested retention window (30 days by default), so a token can accumulate
up to roughly 720 local observations without duplicating them per pool.

The same cycle composes the latest decoded state of every live pool into one
pool-local observation in `pool_price_history`. Priced observations contain
`token1_per_token0`, together with that pool's `token0_price_usd` and
`token1_price_usd` snapshots whenever a USD anchor is available. Pools with no
executable price (for example zero reserves, uninitialized concentrated
liquidity, or an unreadable Solana vault) receive an `unavailable` observation
with a reason and a NULL ratio rather than a fabricated value. Pool pair
volatility ignores those rows and is calculated from that pool's own priced
series; it does not borrow another pool's price series or fall back to a shared
token series. Collection is partitioned
by `(chain, protocol)` inside each cycle, so one chain/protocol can also be
collected independently:

```bash
# Only Polygon Uniswap V4 pools
cargo run --manifest-path rpc/Cargo.toml --bin rpc-price -- \
  --db data/lp_screener.db --rpc-only true \
  --chain polygon --protocol "Uniswap V4"
```

The JSON result contains one coverage record per `(chain, protocol)` group.
`--chain` and `--protocol` are optional case-insensitive filters; omitting both
scans every live group.

For EVM pools with missing token decimals, the worker calls ERC-20
`decimals()` through the local RPC proxy and stores the result in
`token_metadata`. Solana Raydium standard pools read both vault accounts
directly. Balancer weighted pools read `getPoolTokens(bytes32)` directly from
the vault and apply the normalized-weight spot-price formula; stable or
composable Balancer variants use an exact-in `queryBatchSwap` simulation so a
balance ratio is never mistaken for a stable-invariant spot price. These
Balancer reads are batched per chain. Fluid pools call
`getPricesAndExchangePrices()` and decode its custom-error payload. Missing
V2/V3 snapshots also have a pool-local `getReserves()`/`slot0()` fallback.
These direct reads are recorded with `source=rpc_pool_direct`; they do not
require a new full `lp-refresh` cycle.

External-source attempts are tracked in the price database's
`token_price_fetch_state`. A token that
DefiLlama does not know is retried once per day by default instead of being
requested on every refresh. Use `--refresh-external true` for an explicit full
external refresh. The worker never computes pair volatility or IV; those
derived values are calculated by the backend from the history that actually
exists.

```bash
# Cheap recurring sampler: compose atomic pool reads and grow local history.
# It does not define a prerequisite for on-demand price requests.
cargo run --manifest-path rpc/Cargo.toml --bin rpc-price -- \
  --db data/lp_screener.db --rpc-only true --span-days 30

# Keep the full live-pool sampler running for one day, once per hour.
cargo run --manifest-path rpc/Cargo.toml --bin rpc-price -- \
  --db data/lp_screener.db --rpc-only true --span-days 30 \
  --duration-secs 86400 --interval-secs 3600

# After correcting a price-direction decoder, rebuild only RPC-derived
# observations. DefiLlama history is preserved.
cargo run --manifest-path rpc/Cargo.toml --bin rpc-price -- \
  --db data/lp_screener.db --rpc-only true --span-days 30 \
  --rebuild-rpc-history true

# Explicitly recheck every token against DefiLlama
cargo run --manifest-path rpc/Cargo.toml --bin rpc-price -- \
  --db data/lp_screener.db --refresh-external true
```

The history worker is intentionally not called by the frontend and does not
import the backend. On-demand pool details are requested through backend ->
`/price/{chain}/{pool}`; the recurring sampler is an independent history
maintenance path. Its JSON summary reports `live_pools`, `pools_with_price`,
and `pools_without_price`, so decoder/metadata gaps are visible instead of
being presented as zero volatility.

For a seven-day RPC-backed calculation, run `lp-refresh` historical mode to
populate 168 hourly pool observations from archive block tags:

```bash
cargo run --manifest-path rpc/workers/lp-refresh/Cargo.toml -- \
  --history-only --history-hours 168 --batch-size 40 --concurrency 8 --limit 0
```

The backend exposes `iv_7d_annualized` only when that pool has a complete,
gap-free 168-point hourly series. If it is not complete, `iv_annualized` uses
the complete 24-point 1d RPC window and reports
`one_day_fallback_backend_realized_volatility`; it never uses a few-hour
series as a seven-day value.

```bash
curl http://127.0.0.1:8545/healthz
curl http://127.0.0.1:8545/routes
curl http://127.0.0.1:8545/rpc/monad/providers
curl -s http://127.0.0.1:8545/rpc/polygon \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]}'

# Historical reads keep the normal JSON-RPC shape; the block tag is forwarded
# to the upstream node by the local history route.
curl -s http://127.0.0.1:8545/history/base \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":["0x1000000",false]}'
```

`GET /rpc/{chain}/providers` is the replacement for the old “which RPCs does
this chain have?” operation. It returns provider title, masked endpoint, tier,
`selection_rank`, priority, benchmark observations, and live counters. It
never returns credential-bearing upstream URLs.

`POST /history/{chain}` is the read-only historical transport. It accepts a
single JSON-RPC request or a batch and is intended for `eth_call` with a block
tag, `eth_getLogs`, `eth_getBlockByNumber`, and `eth_getBlockByHash`. Write-like
methods are rejected. If an endpoint has `supports_historical=1`, the history
route prefers it; while the capability is unknown it uses the normal
free-first route and lets the provider response determine whether the block is
available. The capability is stored in SQLite, not in another config format.

The current local database contains 32 routes, including `monad-mainnet`, and keeps
the prior multi-chain variants as separate chain records. It also includes the
expanded Solana and Polygon provider catalogs. The database has three
independent parts:

- `chains` and `chain_aliases`: canonical names, network slugs, protocols and
  chain IDs;
- `rpc_endpoints`: provider URL, title, free/paid tier, priority, the
  per-endpoint concurrency ceiling and optional historical-read capability;
- `rpc_benchmarks`: pressure-test observations used for operational review.

An endpoint that fails the latest pressure test remains in the catalog but is
marked `enabled=0`; only enabled endpoints participate in default routing.
`chain_rpc_catalog` shows all candidates, while `active_chain_rpc_catalog`
shows only the default routing pool. This keeps historical chain support
discoverable without sending production requests to a known 403 or otherwise
failed URL.

Use SQLite directly to inspect or change the catalog:

```bash
sqlite3 rpc/data/rpc.db \
  'SELECT slug, network_slug, protocol, chain_id FROM chains ORDER BY slug;'

sqlite3 rpc/data/rpc.db \
  "SELECT c.slug, e.title, e.provider, e.tier, e.priority, e.enabled
   FROM chains c JOIN rpc_endpoints e ON e.chain_id=c.id
   WHERE c.slug='monad-mainnet' ORDER BY e.tier, e.priority;"

# chain_rpc_catalog accepts monad, monad-mainnet, and other aliases;
# active_chain_rpc_catalog restricts the result to enabled endpoints
sqlite3 -header -column rpc/data/rpc.db \
  "SELECT chain_name, title, provider, tier, priority, enabled
   FROM chain_rpc_catalog
   WHERE chain_name='monad-mainnet'
   ORDER BY priority ASC;"

# one row per chain, including chains with no enabled RPC
sqlite3 -header -column rpc/data/rpc.db \
  'SELECT slug, network_slug, active_rpc_count, active_rpcs
   FROM chain_active_rpc_summary ORDER BY slug;'

# historical capability is nullable until a provider has been probed
sqlite3 -header -column rpc/data/rpc.db \
  "SELECT c.slug, e.title, e.tier, e.priority, e.supports_historical
   FROM chains c JOIN rpc_endpoints e ON e.chain_id=c.id
   WHERE c.slug='base-mainnet' ORDER BY e.tier, e.priority;"
```

To add a paid endpoint, insert its URL into `rpc_endpoints` and record its
pressure-test result in `rpc_benchmarks`. Do not add a second config format.

## Routing policy

The normal route selects enabled endpoints in this order:

```text
free -> priority -> current in-flight load -> paid fallback
```

Free endpoints use the lower numeric priority range; paid endpoints are kept
at `priority >= 1000` and are always a fallback tier.

`?title=...` pins a specific endpoint for diagnostics. Without a title, paid
endpoints are only selected when free endpoints are saturated or a retryable
read-only request has exhausted them. Write-like methods are not retried on a
different provider.

Both free and paid entries use the same pressure-test format. Benchmark rows
are stored in `rpc_benchmarks`; they are part of the RPC catalog and are not
duplicated into JSON files. Restart the local proxy after a benchmark so it
reloads endpoint state. `/providers` reports the highest stage whose stored
benchmark has `sustainable=1`; a later failed stress stage is not advertised
as usable capacity.

The previous direct-provider observations are stored as migrated benchmark
rows, but they are not a substitute for rerunning the local proxy test after
keys, plans, or network conditions change. In particular, historical
cross-chain variants should be revalidated before their capacity is treated
as guaranteed.

## Backend integration

`lp-refresh` uses the local proxy by default. Its endpoint URL can be changed
with `RPC_PROXY_URL`:

```bash
export RPC_PROXY_URL=http://127.0.0.1:8545
cargo run --manifest-path rpc/workers/lp-refresh/Cargo.toml -- --help
```

The browser continues to use the backend API; it never receives upstream RPC
credentials. The old direct catalog path remains only as an explicit
migration/debug option with `RPC_DIRECT_CATALOG=1`.

The Rust service is split by responsibility: `state.rs` owns runtime state and
endpoint selection, `catalog.rs` loads the SQLite catalog and serves provider
inspection, `forward.rs` handles raw JSON-RPC forwarding, and `history.rs`
handles historical read-only transport. `main.rs` only assembles these routes.
