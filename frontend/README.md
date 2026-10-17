# LP Screener

React/Vite LP pool discovery UI backed by the Rust API. The canonical source is the local SQLite snapshot.

## Run locally

From this directory:

```bash
npm install
npm run dev       # starts local-rpc on :8545, Rust API on :8100 and Vite on :5173
```

Open `http://127.0.0.1:5173`.

The browser talks to the Rust API through Vite's `/api` proxy. The Rust API
always reads the canonical SQLite snapshot at `../data/lp_screener.db` for pool
catalog and metrics. Runtime chain reads and raw price history come from the
independent `rpc/` workspace; the frontend never calls an upstream provider
directly.

The dashboard polls `/api/rpc-refresh` and `/api/rpc-state/summary` every 30
seconds. A red refresh banner means the latest local state cycle is older than
two refresh intervals.

Watchlist is persisted by the Rust API in SQLite; the browser keeps a mirror
only for offline fallback.
Star a pool in the table or drawer, then open Watchlist from the sidebar.
The pool API uses the comma-separated `addresses` query parameter so saved pools
are queried across pages rather than filtered only from the current table page.

The `Hedge surface` view defaults to the nearest Deribit expiry stored in the
SQLite `option_snapshots` table, so a short LP holding period is not compared
with a quarterly option by accident. Refresh it through the RPC option layer:

```bash
cargo run --manifest-path ../rpc/Cargo.toml -p option-engine -- \
  --db ../data/lp_screener.db --underlying SOL --tenor quarterly --days 90 --refresh
```

It intentionally shows the quarterly quote ladder separately from the existing
short-tenor hedge analysis until the LP replication is recalculated against the
same expiry.

The dashboard deliberately does not expose old arbitrage/replay report files.
Those experiments are outside the minimal runtime and do not feed pool metrics.

## Solana listener surface

The sidebar's Solana listener view is a dedicated multi-LP monitoring screen.
Enter a Solana token symbol, token mint, or pool address. `SOLANA` is the chain
name, not a token, and is not converted into WSOL. `SOL` and `WSOL` are matched
as independent symbols against the local Solana pool catalog; a symbol resolves
to the token address stored on the matched pool. Pool addresses are
resolved back to the token in the local catalog, then the listener is started
or reused for that token. The top pools are selected independently per
protocol. The default is two pools per protocol, matching
SOLANA_LISTENER_POOLS_PER_PROTOCOL=2 in the Rust listener.

The page refreshes the catalog and event feed every seven seconds through:

- /api/solana/pools
- /api/solana/resolve
- /api/solana/listener/start
- /api/solana/listener/status
- /api/solana/pool-snapshots
- /api/solana/pool-history
- /api/solana/events
- /api/solana/whale-alerts

The development command starts `local-rpc` as part of the same process group.
In a deployed environment it can instead be run as a separate service. When
the user submits an address, `lp-api` asks `local-rpc` to start or reuse the listener task; the
listener persists event records to `SOLANA_LISTENER_EVENT_DB`
(`rpc/data/solana_events.db`) and whale alerts to `SOLANA_LISTENER_WHALE_DB`
(`rpc/data/solana_whales.db`). Historical OHLCV is kept in the API's separate
market database (`rpc/data/solana_market.db`). The page reads event and whale
feeds independently. After each pool event, the listener refreshes the pool
account with a per-pool cooldown and writes those snapshots to the separate realtime database
(`SOLANA_LISTENER_REALTIME_DB`, default `rpc/data/solana_realtime.db`).
`SOLANA_LISTENER_ENABLED=true` remains available for starting an automatic
listener at process boot.

Decoded swap execution price and quote-per-target ratio are shown in the pool
table and recent-event feed. Clicking a subscribed pool opens its stored trade
history, current decoded price, whale count and a local 24-hour OHLCV chart.
The price is derived directly from the swap's token balance deltas, so this
path does not require a GeckoTerminal request. Events that do not contain a
wallet-owned target/quote balance pair remain explicitly marked as
`Unclassified pool event` and retain a Solscan transaction link instead of
being presented as a zero-price trade.

Monad listener, independent ESP endpoints and local-network setup:
[API documentation](../docs/monad-listener.md),
[ESP32 / STM32 example](../embedded/monad-display/README.md).
