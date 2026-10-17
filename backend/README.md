# Backend workspaces

The backend layer is an independent Rust workspace. It is separate from the
RPC workspace and communicates with it only through the shared SQLite snapshot.

```text
backend/lp-api/       HTTP read/API service
backend/lp-metrics/   price history, averages, pair ratios, daily volatility
backend/lp-options/   hedge-plan calculations over RPC option quotes
```

`lp-api` owns HTTP routes, filters, and SQLite watchlist writes. `lp-metrics`
owns derived market metrics. It does not call RPCs, fetch external data, or
depend on the frontend.

```bash
cargo check --manifest-path backend/Cargo.toml
cargo build --manifest-path backend/Cargo.toml
```

The `frontend` package keeps the old Python database commands under explicit
`:legacy` names. They are migration/report utilities, not part of the running
API or price-refresh path.
