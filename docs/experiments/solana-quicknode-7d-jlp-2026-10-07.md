# Solana QuickNode 7-day single-pool probe — 2026-10-07

## Purpose

Validate the simpler historical path on one pool before attempting a Solana-wide backfill:

```text
pool address -> getTransactionsForAddress(full) -> protocol event -> hourly carry-forward -> log-return stddev
```

No oracle, historical `getAccountInfo`, block scan, or production price-DB write was used.

## Pool and provider

- Pool: `J27e5izvX4nbaaRDjMKv7DogQzcPidCAECxzE6rK4bF7`
- Family: Meteora DLMM (`solana_dlmm`)
- Pair: JLP/USDC
- Provider route: local RPC proxy, `quicknode-paid`
- Window: `1790780400` through `1791381600` (168 UTC hourly points)
- Current provider block time at the run: `1791382868`

## Result

QuickNode returned 5 pages of full transactions. The probe retained 879 successful transactions in the 7-day window and decoded 774 Meteora `Swap`/`Swap2Evt` event-CPI records. The final series contained 168 prices and 167 hourly log returns.

```text
hourly log-return sample stddev = 0.002480194805033719
volatility percent               = 0.24801948050337191%
min pool-local price             = 4.774684015872163
max pool-local price             = 4.949570593482334
elapsed                          = 11.279 s
```

The hourly price is the last verified post-swap `active_id` state at or before the bucket. Empty buckets carry that last verified state.

## Cross-provider check

The same aligned UTC window was fetched through the existing Helius signature/history path. It produced exactly the same 168 prices and volatility:

```text
0.002480194805033719 (0.24801948050337191%)
```

This is strong evidence that the QuickNode response shape and Meteora decoder are compatible for this pool. It is not yet evidence that all Solana pool families or all high-volume pools have complete coverage.

## QuickNode behavior observed

The documented method supports full transactions, block-time filters, and cursor pagination. In this local endpoint test, an unfiltered descending walk with a page size of 300 was the reliable form; the client stopped after the oldest returned `blockTime` crossed the window start. The filtered form returned a shorter page and did not advance as expected with its cursor, so the production adapter should retain client-side time filtering as a fallback and record page/cutoff coverage.

The probe implementation is [solana-quicknode-history-probe.py](/home/ubuntu/arbitage/rpc/tools/solana-quicknode-history-probe.py).
