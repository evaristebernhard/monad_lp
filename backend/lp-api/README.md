# LP Rust API

Rust/Axum API for the LP pool screener. It reads the catalog SQLite snapshot at
`data/lp_screener.db` and attaches the separate price-history database at
`data/lp_price_history.db`.

This workspace does not call upstream chain RPCs. It reads the shared database
snapshot written by the RPC/state workspaces and calculates derived values for
the UI, including token A/B 1d, 3d, 7d and 30d averages, pair prices,
fee-APR fields and daily realized pair volatility. Token USD anchors remain in
`token_price_history`, while each pool's executable price path is stored in
`pool_price_history`; unavailable observations are exposed as status rows, while
the API never invents a numeric pool ratio when a pool has no executable price.

## Monad wallet balances

```text
GET /api/monad/wallet
GET /api/monad/wallet?address=0x41d5D3B5f2e497d1F4bCF767ffac9DDAb85D2b79
GET /api/monad/wallet/0x41d5D3B5f2e497d1F4bCF767ffac9DDAb85D2b79
```

The first form defaults to the address above. This read-only endpoint uses the
existing local RPC router (`LP_RPC_PRICE_URL`, default `http://127.0.0.1:8545`)
and its configured Monad providers and failover. It verifies mainnet chain ID
143 and reads native MON and Circle native USDC
(`0x754704Bc059F8C67012fEd69BC8A327a5aafb603`) at the same block number.
The response contains `address`, `chain_id`, `block_number`, `block_tag`,
`observed_at` (Unix seconds), `source`, `status`, and `mon` / `usdc` objects.
Each asset includes `balance` and `raw_balance` as exact decimal strings,
plus `symbol` and `decimals`; USDC also includes `contract_address`.
No signing or private key is needed. Responses are uncached RPC reads.
Invalid addresses return HTTP 400, RPC failures return 502, and an overall
20-second timeout returns 504. Failures never appear as zero balances.

Native USDC contract reference:
[Circle's Monad mainnet announcement](https://www.circle.com/blog/now-available-usdc-cctp-wallets-and-contracts-on-monad).

```bash
cargo run --manifest-path backend/lp-api/Cargo.toml
```

Environment variables:

- `LP_SQLITE_PATH=data/lp_screener.db`
- `LP_PRICE_DB=data/lp_price_history.db` (defaults to the catalog database directory)
- `LP_API_ADDR=127.0.0.1:8100`
- `LP_WATCHLIST_USER=local` (temporary single-user watchlist namespace)

## Pool query API

The same pool query is available in query-parameter form and chain-scoped form:

```text
GET /api/pools?chain=solana&protocol=Raydium&page_size=50
GET /api/chains/solana/pools?protocol=Raydium&min_liquidity_usd=100000
```

Supported constraints:

- `chain`, `protocol`, `pair`: exact match
- `protocols`: comma-separated protocol allow-list, for example `Raydium,Orca Whirlpools`
- `pair_category`: `major_pair`, `stable_quote`, `stable_stable`, or `other`
- `pool_type`: canonical family such as `constant_product_v2`, `solidly_v2`, `concentrated_v3`, `stable_swap`, `weighted`, `liquidity_book`, `uniswap_v4`, `solana_whirlpool`, or `unsupported_protocol`
- `min_liquidity_usd`, `min_volume_24h_usd`, `min_fee_bps`, `min_lp_fee_apr`
- `fee_apr_status=available|missing`, `rpc_status=live|unavailable|unsupported|missing`
- `q`: search pair, address, protocol, or token symbols
- `sort`: `liquidity_usd`, `volume_24h_usd`, `volume_7d_usd`, `lp_fee_apr_estimate`, `volume_24h_to_tvl`, `price_usd`, or `fee_bps`
- `order`, `page`, `page_size` (`1`–`100`)

The response contains `items`, `total`, pagination fields, the selected
`filters`, and `source=sqlite`. Pool rows are indexed metadata snapshots; they
should be refreshed from the chain before execution.

Pools whose latest `pool_price_history` observation is `unavailable` are
excluded from the candidate list and pool detail endpoint. Their history is
retained, and they reappear automatically after a later priced observation.

Typical calls:

```bash
# All Solana pools from Raydium
curl 'http://127.0.0.1:8100/api/chains/solana/pools?protocol=Raydium'

# Candidate stable-quoted pools across two protocols
curl 'http://127.0.0.1:8100/api/pools?protocols=Raydium,Orca%20Whirlpools&pair_category=stable_quote&min_liquidity_usd=100000'

# One pool's indexed record
curl 'http://127.0.0.1:8100/api/pools/<pool_address>'
```

Research endpoints:

- `/api/rpc-refresh` reads `lp_refresh_latest.json` when present.
- `/api/price-history/status` reports raw history coverage and source counts.
- `/api/vol/{chain}/token/{address}` returns backend-owned token averages,
  daily volatility, sample counts, and data quality status.
- `/api/vol/{chain}/pool/{address}` returns both token reports plus pair-relative
  volatility. Pair volatility uses only that pool's own `pool_price_history`;
  token USD history is never used as a pool-volatility fallback.
- `/api/rpc-state/summary` reads the Rust refresher's live state from
  `pool_rpc_state`.

The 7d realized-volatility fields require 168 contiguous hourly points from
`pool_price_history`. Until that window exists, `iv_annualized` falls back only
to a complete 24-point 1d window and is marked
`one_day_fallback_backend_realized_volatility`; short series are not
annualized.
- `/api/watchlist` supports `GET`, `POST {"pool_address":"..."}`, and `DELETE`;
  `/api/watchlist/<pool_address>` removes one item in SQLite.
- `/api/option-hedge?currency=SOL&chain=solana&tenor=nearest&limit=40` returns
  only pools with a complete Deribit put/call hedge plan and the matching quote
  snapshot. The legacy `/api/hedge-surface` route remains as an alias.

Refresh option quotes with the RPC option data layer:

```bash
cargo run --manifest-path rpc/Cargo.toml -p option-engine -- \
  --db data/lp_screener.db --underlying SOL --tenor nearest --days 7 --refresh
```

Then calculate hedge plans in the backend workspace:

```bash
cargo run --manifest-path backend/lp-options/Cargo.toml -- \
  --db data/lp_screener.db --underlying SOL --tenor nearest \
  --chain solana --min-tvl 10000 --limit 100 --write-db
```

Override those report locations with `LP_ARB_REPORT_PATH`, `LP_RPC_REPLAY_PATH`,
`LP_RPC_HEALTH_PATH` and `LP_RPC_REFRESH_PATH` when running from another
working directory.

## Solana listener API

The Solana listener surface accepts either a token mint or a known Solana pool
address. A pool address can be resolved through
`GET /api/solana/resolve?address=...`; the frontend then calls
`POST /api/solana/listener/start` with `{"token":"..."}`. The backend proxies
that request to `local-rpc`, which starts the listener once and reuses the
existing task for repeated requests. Events and whale alerts are read through
`/api/solana/events` and `/api/solana/whale-alerts` and are persisted by
`local-rpc` in its SQLite event database.
`/api/solana/pool-snapshots` exposes the latest per-pool on-chain account
snapshots from the separate realtime database.
`/api/solana/pool-history?pool=...&hours=24` exposes the listener's locally
stored classified swaps and derived OHLCV candles for a selected pool, so
opening a pool detail does not require a new market-data provider request.
`/api/solana/protocols` exposes the protocol and pool-type decoder registry.

## Public MON → USDC swaps

`POST /api/monad/swap/quote` and `POST /api/monad/swap/build` expose the
website's existing swap path for external clients. Build returns an unsigned
transaction; keys, signing and broadcasting stay on the caller's computer.
See [API fields and Python usage](../../docs/monad-swap-api.md).

## Monad 多池报价

- `GET/POST /api/selection/quote`：传入字符串 `amount`、`token_in`、`token_out`，返回逐池报价、最佳池、来源区块、实时性及重放哈希。
- `GET/POST /api/selection/quote-stream`：相同参数，SSE 实时报价。
- `GET /api/selection/pools`：核验后的候选池目录。
- `GET /api/monad/prices-stream`：20 pool 实时价格。

完整请求与响应说明见 [`docs/quote-api.md`](../../docs/quote-api.md)。报价由后端执行，调用方不需要生成 calldata。
