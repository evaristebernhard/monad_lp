# RPC option data layer

This is the option-data module inside the independent RPC ingestion workspace.
It stores public Deribit option snapshots in the shared SQLite database:

```text
option_snapshots  -> snapshot metadata and expiry
option_quotes     -> bid/ask, strike, IV and contract data
hedge_analysis    -> LP hedge plans consumed by backend/lp-api
```

The Rust command is the runtime entry point. It never writes a JSON snapshot:

```bash
# Create the option tables and inspect the latest SOL snapshot
cargo run --manifest-path rpc/Cargo.toml -p option-engine -- \
  --db data/lp_screener.db --underlying SOL --tenor nearest

# Refresh public Deribit quotes into SQLite
cargo run --manifest-path rpc/Cargo.toml -p option-engine -- \
  --db data/lp_screener.db --underlying SOL --tenor nearest --days 7 --refresh

# Calculate LP hedge plans in the backend layer
cargo run --manifest-path backend/lp-options/Cargo.toml -- \
  --db data/lp_screener.db --underlying SOL --tenor nearest \
  --chain solana --min-tvl 10000 --limit 100 --write-db
```

The backend reads `option_snapshots` and `option_quotes` directly from SQLite;
all hedge-plan calculations live in `backend/lp-options`.

The module never places orders and does not need Deribit credentials. `MON`
and other assets without a direct Deribit option universe are reported as
unhedgeable rather than being given a synthetic option quote.
