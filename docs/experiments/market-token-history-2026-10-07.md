# Market-source token history experiment

Date: 2026-10-07. Scope: Solana catalog, seven-day approximate history. No
oracle and no fabricated seed before the first observed market candle.

## Source coverage observed

At the latest audit, the catalog contained 2,378 unique Solana token
addresses and 3,228 pools. The price database contained approximately:

| Source | Token addresses / rows | Meaning |
| --- | ---: | --- |
| `defillama` | 736 / 29,737 | Sparse USD observations |
| `defillama_hourly_carry` | 691 / 116,496 | Last-known DefiLlama value carried into UTC hours |
| `binance_1h` | 299 / 50,065 | Explicit or unique-symbol Binance 1h spot mapping |
| `geckoterminal_pool_1h` | 19 / 2,963 | Representative Solana pool OHLCV, token-selected |

The counts are source rows, not additive coverage: the same stablecoin or
major asset can occur in several sources. A pool-level audit found 1,765 of
3,228 pools with both token histories and 1,745 with both sides spanning at
least five days.

## Runtime verification

For Orca `SOL/USDC` pool
`Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE`, the API returned 24, 72 and
168 hourly samples for 1d, 3d and 7d, respectively. The pair report selected
`token_price_history_market_proxy` and marked the price semantics as
`external_market_pair_proxy`.

For Meteora `JLP/USDC` pool
`J27e5izvX4nbaaRDjMKv7DogQzcPidCAECxzE6rK4bF7`, the same external-market
route returned 24, 72 and 168 samples for 1d, 3d and 7d. This confirms that a
token history from GeckoTerminal plus a stablecoin history can produce pair
volatility without reading historical pool state.

## Boundary

The public GeckoTerminal endpoint repeatedly returned HTTP 429 after a small
number of requests from the shared environment. Those attempts are stored as
provenance errors and are not converted into prices. The queue is resumable;
the remaining long-tail tokens remain missing until the public limit or a
provider with suitable quota is available.
