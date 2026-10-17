# Project architecture

The repository is organized as independent workspaces with explicit runtime
boundaries, not as one large Rust workspace:

```text
frontend/              independent React/Vite presentation workspace
backend/               independent Rust backend workspace
backend/lp-api/        HTTP read/API service
backend/lp-metrics/    price history and derived metric library
backend/lp-options/    option hedge-plan calculation library/CLI
rpc/                   independent Rust data-ingestion workspace
rpc/option/            public option quote ingestion and option DB layer
rpc/workers/lp-refresh/ RPC-state refresh worker inside the RPC layer
data/                  canonical SQLite snapshot
docs/                  protocol and engineering documentation
```

The runtime data path is:

```text
frontend -> backend/lp-api -> data/lp_screener.db + data/lp_price_history.db
rpc/workers/lp-refresh -> rpc/upstream RPC providers
rpc-price -> DefiLlama or rpc/pool_rpc_state -> data/lp_screener.db + data/lp_price_history.db
rpc/option -> Deribit public quotes -> data/lp_screener.db
backend/lp-options -> data/lp_screener.db (hedge_analysis) [optional research job]
```

The three runtime layers do not import each other. SQLite remains the narrow
shared boundary, with catalog/current state in `data/lp_screener.db` and raw
price history in `data/lp_price_history.db`; the API attaches the latter as
`price`. `rpc` writes raw observations, `backend` calculates averages, pair
prices, APR and volatility, and `frontend` only calls the backend HTTP API.
No browser request selects an upstream provider and the backend does not fan
out to chain RPCs.

Each layer can be built and deployed independently. The backend's internal
crates are grouped in their own workspace so the API does not own metric
implementation details:

```bash
cargo check --manifest-path rpc/Cargo.toml
cargo check --manifest-path backend/Cargo.toml
cargo check --manifest-path rpc/workers/lp-refresh/Cargo.toml
npm --prefix frontend run build
```

There is no Python runtime path. Price/history maintenance belongs to
`rpc/src/price`, and metric calculation belongs to `backend/lp-metrics`.
Pool volatility is derived only from the matching pool's `pool_price_history`;
`token_price_history` is an USD anchor and is never a pool-volatility fallback.

External option quote ingestion belongs to `rpc/option`; the backend only reads
the resulting SQLite snapshot and never calls Deribit directly.
`hedge_analysis` is experimental derived output and is not part of the pool
catalog. A catalog rebuild must leave it empty; it is populated only by an
explicit `backend/lp-options` run against a fresh option snapshot.

The `priority_pools` table is the fixed 1,000-pool research universe. It is
separate from the user watchlist and is refreshed with an explicit block per
EVM chain before being committed as one SQLite snapshot.
