# Monad listener and display API

Run from `frontend`: `npm install` then `npm run dev`. The API binds
`LP_API_ADDR=0.0.0.0:8100`; Vite proxies `/api` and `/monad` to it.
The ESP32 must use the computer's LAN IP, not `localhost`. Connect the ESP32
and computer to the same router or phone hotspot and allow TCP 8100 locally.

The existing Solana API routes and history remain available. The navigation
entry now opens Monad. Monad browser storage uses `lp-screener.monad-listener-*`.
Native MON (zero address) and [canonical WMON](https://monad.docsbot.app/developer-essentials/network-information) (`0x3bd359c1119da7da1d913d1c4d2b7c461115433a`) are separate assets; catalog labels for these addresses are normalized on the Monad surface. Symbol matches with
multiple addresses require an explicit choice. V4 identifiers are 32-byte pool
IDs; their explorer links are never constructed as contract address links.

## Selection and persistence

`GET /api/monad/resolve?address=MON` accepts a symbol, token address, or catalog
pool address/ID. Ambiguous responses have `requires_selection=true`, `candidates`
and a null `resolved_token`. No match returns `success=false`.
The single input accepts names and addresses. Matching is case insensitive;
`ETH` / `Ethereum` also match `WETH`, without matching staking derivatives.
Resolved token metadata overrides stale pool-side symbols. Queries read only
matching catalog rows. Each submission resolves once, including repeated names;
pool lists and local history render independently of slower feeds and external
charts. External chart fetches coalesce only identical requests, so another
token's pending chart does not block a switch.

`POST /api/monad/listener/start` or `PUT /monad/display` accepts
`{"token":"0x...40 hex digits..."}`. The server selects up to five supported
pools with at most two per catalog protocol name: one per protocol first,
then second pools, ordered by descending TVL within each round. No pool list
from the client is trusted. `GET /api/monad/pools?token=0x...` returns these exact
saved IDs for the active token, with current catalog fields and display metrics.
Reactivating a token recalculates selection. `GET /monad/display` returns
`token`, `pool_ids`, `pools` (catalog metadata), and a monotonic `version`.
The RPC worker reads this display selection and continuously monitors the fixed 20-pool demo registry as well. Switching webpage tokens does not stop the fixed pools.
A configuration with unchanged token and pool IDs retains its version.

## Independent ESP metrics

| Request | `unit` | Meaning |
| --- | --- | --- |
| `/monad/{pool}/vol` | `daily_fraction` | Sample standard deviation of 23 log returns from 24 consecutive hourly prices, multiplied by sqrt(24) |
| `/monad/{pool}/fee_apr` | `annual_fraction` | Catalog 24h volume × normalized fee/10000 × 365 / TVL × LP fee share |
| `/monad/{pool}/price?base={token_address}` | `quote_per_base` | Units of counterpart token for 1 base token, from this pool's decoded state or a newer stored pool ratio |
| `/monad/{pool}/volume` | `USD` | Catalog 24h USD volume |
| `/monad/{pool}/tvl` | `USD` | Catalog USD liquidity |

Each returns `pool`, `value`, `unit`, `as_of`, `status`. Percentages are fractions:
`0.15` means 15%. Price also returns `base_token` and `quote_token` addresses.
A response with insufficient observations has a null `value` and a reason in
`status`; `as_of` remains the last actual source time, or null. A price older
than 30 seconds is `stale`; hourly volatility older than two hours is `stale`.
Catalog metrics retain catalog `updated_at` verbatim (UTC); no request time is
substituted. Fee APR additionally exposes `fee_as_of`. The formula and catalog
fee normalization are shared with the existing API: V4 requires a live RPC fee;
V2/V3 catalog tiers over 1000 are divided by 100. The Monad APR route has no
minimum TVL cutoff; TVL must only be positive to serve as a denominator.
This does not change the existing Solana API's estimation rules.

Unknown pools/metrics return 404. Missing/invalid base or a base outside the pool
returns 400. Price caches independently for 200ms; other metrics cache for
2 seconds, including null results. A metric request performs only its own read/calculation. It never calls
RPC, computes all pool metrics, or starts historical backfill. Selection requests
are separate from metrics and may read display metrics for the webpage.

### External pool volatility backfill and 20-pool audit

```sh
python3 rpc/tools/monad-pool-coverage.py --fetch
# Read-only audit of existing observations, no network request:
python3 rpc/tools/monad-pool-coverage.py
# Verify each of the 80 actual metric responses and export JSON plus CSV:
python3 rpc/tools/monad-pool-coverage.py --api http://127.0.0.1:8100
```

The fetcher requests GeckoTerminal hourly OHLCV for each exact Monad pool ID
with `currency=token&token=base&include_empty_intervals=false`. It verifies both
token addresses, normalizes direction and writes completed hourly closes to
the existing price database as `geckoterminal_pool_ratio_1h`. Real RPC observations
are preserved. It never fills gaps or substitutes token USD market proxies.
DefiLlama token USD histories remain a separate market-proxy data source.

The reports in `output/monad-four-metrics.json` and `.csv` select 20 pools with a complete
latest 24-hour observation window, a positive price, volume and Fee APR. It
deduplicates pair names across fees, protocols, letter case and reversed order.
Native MON and WMON remain distinct names.
prefers the requested 20 pairs (with USDC/GHO and AUSD/USDT0 replacements), then
tries other catalog pools if data are incomplete. There is no TVL threshold or
TVL ranking. Every entry retains its catalog timestamp, source and price time;
historical catalog volume and APR must not be presented as current quotes.
Public API calls are paced and HTTP 429 is retried with backoff. Partial work is
stored in SQLite and resumed on the next run; fewer than 20 qualifying pools
produces exit code 2, never invented entries. Re-run with `--fetch` hourly to
keep coverage current. Metric requests themselves never fetch or backfill.

The price endpoint uses the latest stored ratio if a listener snapshot is
absent or older, and preserves `status=stale` for observations over 30 seconds
old. Volatility exposes `source`, `sources`, `samples`, `window_start` and
`window_end`; it uses 24 completed, consecutive hourly observations. Sparse
history still yields null rather than zero.

## Events and chart sources

Each monitored pool has its own `eth_getLogs` timer (5 seconds by default), with
up to 128 blocks per range and a two-block safety lag. If a provider rejects a
range (JSON-RPC range limit or HTTP 400/413), the collector halves it immediately,
retains the smaller limit for that pool worker, and retries from the same start
block. Even failure at one block leaves the cursor unchanged. HTTP failures retain
the response body in the error log. State refreshes run independently
so a slow state read does not block event collection. Block and transaction
reads are batched in groups of at most ten, with single-request fallback. State/price refreshes
use a dedicated 1-second price batch and a 5-second state/history cycle. Live state writes the shared hourly price history. The existing price layer also
retains DefiLlama, Binance and GeckoTerminal market histories with their own
provenance; these token market proxies are not silently substituted for the
strict per-pool ESP volatility metric.

RPC archive backfill is optional (`MONAD_LISTENER_BACKFILL=1`), disabled by
default. When enabled, it fills one missing hour per cycle, restricted to the
recent 24-hour window, using the existing price probe's recent-block estimate
and verified time bracket. It does not start a genesis-to-head search. Block
headers use historical-provider routing, null results try alternative routed
providers, and unavailable pools do not stop other pools filling. It never
overwrites live snapshots or newer hourly observations. Metrics never start
or wait for backfill. Archive access is required; unavailable history stays null. Slow providers skip missed timer ticks without overlapping a pool range.
Changing configuration cancels workers for the previous selection. A first
cursor is persisted before collecting; failures retain the range. Subsequent
ranges resume from the persisted cursor after restart. Checkpoint block hashes
are verified and replacement blocks remove orphan events and checkpoints
before recollection. Transactions are keyed by pool + transaction hash + log
index. Transaction sender and router/event sender are separate fields.

V2 Swap/Mint/Burn, Uniswap/PancakeSwap V3 Swap/Mint/Burn, and V4 Swap/ModifyLiquidity use canonical
ABI signatures. V4 filters include BOTH the PoolManager address and indexed pool
ID. Zero-delta V4 position pokes are recorded as `liquidity_update`. Raw liquidity
units are not misreported as token amounts. V3 amounts are pool balance deltas;
V4 amounts are caller deltas, so their trade direction signs differ.

The default whale threshold is $10,000 (`MONAD_LISTENER_WHALE_USD`). USD values
require a trusted stable-token address anchor from pool state observed within 30 seconds of the
event. Missing reliable USD values leave the event intact with `whale=null` and
`whale_status=unknown_usd_value`. The default anchor is native [USDC issued on Monad by Circle](https://www.circle.com/blog/now-available-usdc-cctp-wallets-and-contracts-on-monad); estimates assume 1 USDC = 1 USD. Symbol-only stablecoin names are never trusted. No directory USD price is treated as a live
oracle. PancakeSwap V3 uses its extended Swap event with protocol fee fields
([official ABI](https://github.com/pancakeswap/pancake-v3-contracts/blob/main/projects/v3-core/contracts/interfaces/pool/IPancakeV3PoolEvents.sol)). Unvalued trades are counted separately in history summaries.

Web endpoints: `/api/monad/events`, `whale-alerts`, `pool-snapshots`, `protocols`,
`pool-history`, `pool-candles`, `listener/status`. Events accept `token`, `pool`,
`limit`. History requires `pool`, accepts `target_mint` (or `token`) and
`resolution=1m|5m|15m|1h`. Results are limited to currently selected pools and the
requested token, so switching tokens does not mix activity feeds.

The webpage requests two independent sources, as the Solana surface does:

- `/api/monad/pool-candles?pool=...&token=...&resolution=1h&limit=24`
  fetches external GeckoTerminal OHLCV for that **pool and token address** in USD.
  It never calls the listener or chain RPC. Responses carry `source`, `price_unit`,
  `as_of` (last candle's time), `fetched_at`, and `stale`. Monad-only cache entries
  are keyed by pool, token, resolution and limit, reused for 60 seconds. On
  upstream failure, saved candles retain their original times and are marked
  stale. Missing provider coverage stays unavailable; V4 IDs stay IDs.
- `/api/monad/pool-history` reads local swaps for the recent trade feed and a
  quote-per-target fallback chart. Its volume is **observed volume**, never the
  directory's full 24h volume. Up to 5000 events are aggregated. `hourly_prices`
  is a separate source for stored hourly pool prices.

The chart uses external USD candles when available, otherwise local ratio
candles, with visible source/unit labels. It never merges USD prices and token
ratios into the same series. Local observed volume and directory 24h volume
remain separate; directory TVL and volume keep their original timestamps.

## Storage and configuration

| Variable | Default / purpose |
| --- | --- |
| `MONAD_LISTENER_DB` | `rpc/data/monad_events.db` (resolved relative to project or frontend directory), shared API/RPC config, events, whale flags, cursor/checkpoints, snapshots |
| `MONAD_POOL_DB` | `data/lp_screener.db` (RPC catalog path) |
| `LP_SQLITE_PATH` | API catalog path |
| `LP_PRICE_DB` | `lp_price_history.db` beside the catalog; existing common hourly price DB |
| `LP_RPC_PRICE_URL` | `http://127.0.0.1:8545` |
| `MONAD_POOL_MANAGER` | `0x188d586ddcf52439676ca21a244753fa19f9ea8e` |
| `MONAD_LISTENER_BACKFILL` | `0`; set `1` to enable optional recent 24h RPC archive filling |
| `MONAD_MARKET_DB` | `monad_market.db` beside the Monad event DB, separate external chart cache |
| `MONAD_OHLCV_BASE_URL` | `https://api.geckoterminal.com/api/v2/networks/monad`; external chart provider base |
| `MONAD_LISTENER_POLL_MS` | `5000`; configurable from 2000 to 60000 milliseconds for state/log polling |
| `MONAD_LISTENER_WHALE_USD` | `10000` |
| `MONAD_USD_ANCHORS` | Comma-separated trusted 1-USD token addresses; defaults to native USDC `0x754704bc059f8c67012fed69bc8a327a5aafb603` |

Use absolute paths for both processes when launching outside the documented
working directory. No Solana database is reused for Monad event records.

ABI and deployment sources: [IPoolManager](https://github.com/Uniswap/v4-core/blob/main/src/interfaces/IPoolManager.sol),
[PoolManager event emission](https://github.com/Uniswap/v4-core/blob/main/src/PoolManager.sol),
[official Monad deployment addresses](https://developers.uniswap.org/docs/protocols/v4/deployments#monad-143).

## Verification

```sh
cargo test --manifest-path backend/Cargo.toml -p lp-api -p lp-metrics
cargo test --manifest-path rpc/Cargo.toml --bin local-rpc
npm --prefix frontend run build
# With Vite running and Playwright Chromium installed:
node frontend/tests/monad-browser.mjs
node frontend/tests/monad-switch-performance.mjs
# Optional live switch timing (prefer an API with an isolated display DB):
MONAD_API_ORIGIN=http://127.0.0.1:8101 node frontend/tests/monad-live-switch.mjs
# Optional live services + actual external OHLCV smoke test:
node frontend/tests/monad-live-browser.mjs
# Fixed 20-pool live demo and advancing RPC timestamps:
MONAD_WEB_ORIGIN=https://lp.chayu123.me node frontend/tests/monad-live-demo.mjs
```

The browser fixture checks ambiguous symbols, address selection, metric rendering,
external USD candles, local ratio fallback, events, whale alerts, V4 links and absence of Solana listener calls.

## GitHub Actions and hosting

`.github/workflows/monad-checks.yml` builds the frontend, runs browser fixtures
(including null resolution and delayed chart/feed responses), Rust tests on relevant pushes and pull requests. Live tests require
running services and are intentionally separate.

Actions can also build and deploy this application to a server. The Rust API,
RPC listener and writable SQLite databases need persistent hosting with a
process supervisor and a persistent data volume. GitHub Pages can host only the
static frontend; Vite's development proxies are not part of the production
bundle. A Pages deployment would need a configured public HTTPS API URL and
appropriate CORS, plus a repository base path for assets. Serving the frontend
and proxying `/api` and `/monad` on one HTTPS server is the simplest deployment
for the current relative API URLs. The checks workflow does not publish a site
or upload local databases or credentials.

## Hackathon live demo

`config/monad-demo-pools.json` freezes 20 distinct pair names, pool IDs and
explicit base token addresses. The collector keeps these pools running in
addition to any extra webpage display pools. Reconciliation reuses existing
workers; display changes do not restart all fixed pools. Pool identities,
decimals and immutable V3 fees are read once and cached; concurrent requests
share the latest block number for 500ms. State block timestamps remain the
price source time, and block replacement verification remains enabled.

Open **20 pool demo** in the sidebar for the fixed table. Prices refresh every
second; the other metrics refresh every 5 seconds. The table displays each price's age. `GET /monad/demo` (also
`/api/monad/demo`) returns the same 20 pools and their four metric results.
Existing independent metric URLs and the five-pool `/monad/display` contract
remain available. Price responses are cached for 200ms; other metric caches last 2 seconds. The configured interval is a target: provider
latency/failure can increase the actual age, which the UI exposes.

The market collector requests exact pool IDs from GeckoTerminal's multiple-pool
endpoint every 30 seconds for current indexed 24h volume and TVL. Both token
addresses must match the registry. These snapshots live in the independent
Monad event database's `live_market` table, not the historical catalog. Fee APR
uses this volume/TVL and the RPC snapshot's fee. API GETs never call providers.

The provider does not supply a per-field source timestamp. These responses
explicitly expose `as_of_kind=provider_snapshot_observation`, `observed_at`,
`provider_cache_age_seconds`, and `source_as_of=null`. APR also exposes
`fee_as_of`, `volume_as_of` and `tvl_as_of`; composite `as_of` is the oldest
component observation. Market snapshots expire after 120 seconds; RPC prices
and the fee component expire after 30 seconds. Catalog fallback stays marked
stale. An outage retains the last real data and never advances its timestamps.

Volatility still uses 24 completed consecutive hourly pool ratios; its value
normally changes on the hour, rather than on every 5-second poll. RPC writes
live hourly observations continuously. A separate, paced history repair loop
checks one fixed pool every 10 seconds and fills actual missing hourly closes
from GeckoTerminal (no empty-interval filling). Complete windows require no
market request. This keeps historical repair off the RPC state refresh path.

The demo does **not** label locally collected swaps as a complete RPC 24h USD
volume before a full day is covered. Building that volume exclusively from RPC
would require a complete, gap-free 24h event window and reliable USD valuations;
a faster polling interval alone cannot provide that history.

### Fast price API

`GET /monad/prices` (also `/api/monad/prices`) returns the fixed 20 pools:
`{"price_poll_ms":1000,"prices":[{"pool":"...","value":0.023349726,"unit":"quote_per_base","base_token":"...","quote_token":"...","as_of":1792161893,"state_block":113924677,"status":"ok"}]}`.
Optional `?pools={pool_id},{pool_id}` selects up to 20 monitored pools; optional
`&base={token_address}` sets the price direction and must belong to every selected
pool. Unknown monitored pool IDs return 404, invalid parameters return 400.
The single-pool `/monad/{pool}/price?base={token_address}` endpoint remains available.
Both routes read stored RPC snapshots without computing other metrics.

`MONAD_PRICE_POLL_MS` defaults to 1000 (allowed range 500–5000). The collector
batches state calls against a shared block, checks its hash before saving, and
prevents older state refreshes from overwriting a newer block. The polling target
is independent of event collection and market snapshots. Upstream latency can
make actual intervals longer. No trade means the pool ratio can remain unchanged.
Use at least 6–8 significant digits on a small display: MON/USDC around 0.02335
will look static if rounded to two decimal places. Firmware is maintained separately
and is not included in this repository.

## MON ↔ USDC trading on the webpage

The **20 pool demo** now includes **Buy USDC with MON**. This first trading
integration supports native MON → USDC and USDC → native MON on the existing Uniswap V4 pool
`0x18a9fc874581f3ba12b7898f80a683c66fd5877fd74b26a85ba9a3a79c549954`.
It does not route through another pool. Other demo pools remain read-only.

1. Open `https://lp.chayu123.me` with MetaMask, Rabby or another injected EVM
   wallet (EIP-1193; EIP-6963 wallet discovery is supported).
2. Connect and switch/add Monad Mainnet, chain ID 143. Hold native MON on Monad.
3. Enter the MON amount and request a pool quote. The V4 Quoter simulates that
   exact PoolKey; no monitor price is used as an executable quote.
4. Review expected USDC, minimum received and slippage (0.1%, 0.5% or 1%).
   Quotes expire after 30 seconds; changing the amount, wallet, chain or slippage
   invalidates the previous quote.
5. Confirm purchase. The app simulates the transaction and checks MON for the
   input plus a gas reserve. The wallet then displays its transaction approval.
   Only the wallet signs and sends. No private keys or signing requests reach
   the backend. Native MON input needs no ERC20 or Permit2 approval.
6. The receipt shows the actual USDC transfer or native MON pool output to the connected wallet and links
   to Monadscan. Reverts are distinguished from successful receipts. If receipt
   polling times out, the transaction remains pending and another purchase is
   blocked until confirmation is checked.

The PoolKey is `{currency0: address(0), currency1:
0x754704bc059f8c67012fed69bc8a327a5aafb603, fee: 500, tickSpacing: 10,
hooks: address(0)}`; its ABI hash must equal the selected pool ID. Fee 500 means
0.05%. USDC uses 6 decimals and MON uses 18; amounts use integer arithmetic.

Addresses come from [Uniswap's official Monad deployments](https://developers.uniswap.org/docs/protocols/v4/deployments#monad-143):
Quoter `0xa222dd357a9076d1091ed6aa2e16c9742dd26891`, original Universal Router
`0x0d97dc33264bfc1c226207428a79b26757fb9dc3`. The encoding deliberately targets
this original router's V4 action ABI (without the newer `minHopPriceX36` field).
Do not replace the router address with a new version without checking its ABI.
The local encoder uses V4_SWAP (SWAP_EXACT_IN_SINGLE, SETTLE_ALL, TAKE_ALL), then
SWEEP to refund any unused MON. The minimum output is enforced by the router;
USDC goes to the wallet caller. The deadline is the latest chain time +120s.

`POST /api/monad/trade-rpc` forwards a restricted set of read-only JSON-RPC
methods to the existing local Monad provider router. It supports quotes,
simulation, balances and receipts. It rejects transaction submission and signing
methods. The ESP metric APIs are unchanged. No aggregator API key is needed.

Verification:

```sh
node --test frontend/tests/monad-trade.test.mjs
# With Vite running; wallet and submission are mocked, no funds move:
node frontend/tests/monad-trade-browser.mjs
node frontend/tests/monad-trade-reverse-browser.mjs
# Optional real read-only preflight with a funded public wallet address:
MONAD_TEST_WALLET=0x... node frontend/tests/monad-trade-live.mjs
```

`output/monad-trade-live-check.json` records a real pool quote and successful
read-only router execution with synthetic test-account balance overrides. This
is not a funded mainnet transaction. The final wallet signature and funded
transaction require the user's wallet.

### USDC → MON direction and authorization

Select **USDC → MON** in the Direction control. The entered amount is USDC
(6 decimals); the output is native MON (18 decimals), using the same PoolKey
with `zeroForOne=false`. Native transaction value is zero; MON is still required
for gas. Direction changes clear the previous input and quote.

If existing allowances are insufficient, the webpage exposes two distinct
wallet transactions before the swap:

1. **1. Approve USDC**: USDC `approve(Permit2, enteredAmount)`, for exactly the
   input amount, never an unlimited value.
2. **2. Authorize swap**: Permit2 `approve(USDC, UniversalRouter, enteredAmount,
   chainTimestamp + 600)`. This gives only the official router permission for
   that amount for ten minutes. Sufficient existing authorization is reused.

Then request a fresh quote and **Confirm purchase**. The app checks both current
allowances and the USDC balance again before simulating and asking the wallet to
send. A pending approval has its own receipt link and blocks further submissions;
confirmation checks can resume after a timeout. Editing the amount may require a
new approval. These approval transactions cost MON; they do not execute a swap.

Native MON receipt amounts come from this pool's V4 Swap event emitted by the
verified PoolManager and router, rather than ERC20 Transfer events. Displayed
native output is before the separately charged network fee. Missing receipt
amounts are marked unavailable, rather than reported as a fabricated zero.

`node frontend/tests/monad-trade-reverse-live.mjs` performs a read-only live
reverse quote and full router simulation using synthetic USDC balance and
allowance storage overrides. It verifies the same transaction fails without
router authorization. The result is saved in
`output/monad-trade-reverse-live-check.json`. It never signs or broadcasts.


## Fixed-pool hourly repair

The RPC collector fills missing completed hours for the fixed 20 pools as well
as the active listener selection. Archive repair runs by default every 30 seconds;
set MONAD_LISTENER_BACKFILL=0 to disable it. It checks all 24 completed hours and
samples verified historical block state near each hour's end. Existing valid
observations are preserved, and historical reads never replace live snapshots.

To repair the current window immediately, run from frontend/:

    MONAD_POOL_DB=../data/lp_screener.db MONAD_LISTENER_DB=../rpc/data/monad_events.db RPC_DB_PATH=../rpc/data/rpc.db ../rpc/target/debug/local-rpc --backfill-monad-hours

This command uses configured archive providers and exits after up to 24 repair
passes. Provider failures are reported; unavailable hours remain missing.
