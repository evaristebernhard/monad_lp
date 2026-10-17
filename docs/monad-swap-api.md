# MON → USDC public API

> This document describes the legacy fixed-pool API. For the best-quote API and executable multi-protocol example, use [the current operations guide](./最优报价与主网交易操作指南.md) and `examples/monad-best-buy/`.

Base URL: `https://api.chayu123.me`. These endpoints expose the website's
Uniswap v4 MON/native-USDC pool on Monad mainnet (chain ID 143).
They never sign or broadcast transactions, accept keys, or require API keys.
USDC and unused MON go to the wallet that signs and sends the transaction.

## Quote

```bash
curl https://api.chayu123.me/api/monad/swap/quote \
  -H 'Content-Type: application/json' \
  -d '{"amount_mon":"0.1","slippage_bps":50}'
```

`amount_mon` is an exact positive decimal string with at most 18 decimals;
`slippage_bps` defaults to 50 (0.5%) and must be 1–100. No floating-point
amounts are accepted. Unknown fields, including private keys, are rejected.
This is a fixed single-pool route, not a best-price aggregator.

## Build an unsigned transaction

```bash
curl https://api.chayu123.me/api/monad/swap/build \
  -H 'Content-Type: application/json' \
  -d '{"wallet":"0xYOUR_WALLET_ADDRESS","amount_mon":"0.1","slippage_bps":50}'
```

Build obtains a fresh quote; it does not accept a previous quote or caller
supplied calldata. Both endpoints return:

| Field | Meaning |
| --- | --- |
| `chain_id`, `direction` | `143`, `MON_TO_USDC` |
| `router`, `pool_id`, `token_out` | Fixed deployed router, pool and native USDC |
| `amount_in_wei` | MON input, integer decimal string (18 decimals) |
| `amount_out_raw`, `minimum_out_raw` | Expected/minimum USDC, integer strings (6 decimals) |
| `slippage_bps` | Selected slippage |
| `block_number` | Hex block tag used for the quote |
| `quoted_at`, `expires_at` | Unix seconds; refresh the quote after 30 seconds |
| `deadline` | Chain timestamp + 120 seconds, enforced by the router |
| `recipient` | `transaction_sender`; signing wallet receives the USDC |

Build also returns `transaction` with `chainId`, `from`, `to`, `value` (wei
as decimal string), and `data`. The client supplies pending nonce and gas,
simulates/estimates with its signing address, and checks its MON balance
covers the purchase and gas. A quote does not reserve liquidity or guarantee
execution. The contract enforces the minimum output and deadline atomically.

Malformed inputs return 400/422, upstream RPC or unusable quote errors 502,
and overall quote timeouts 504. Provider errors are not exposed. RPC must
report chain 143 and a latest block no older than 60 seconds.

## Python from another computer

Copy `examples/monad-buy-usdc/` to that computer:

```bash
python -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
cp .env.example .env
```

Set `PRIVATE_KEY` in **that computer's** `.env`. `MONAD_API_URL` defaults to
this website; `MONAD_RPC_URL` defaults to `https://rpc.monad.xyz`.
Private keys are never uploaded. The example reconstructs the complete allowed
calldata locally and checks chain, wallet, router, amount, minimum and expiry
before signing. The API cannot change the recipient or insert other commands.

Preview without a private key (no signing or broadcast):

```bash
python buy_usdc.py 0.1 --wallet 0xYOUR_WALLET_ADDRESS
```

Execute using the local wallet (a real purchase):

```bash
python buy_usdc.py 0.1 --send
```

Optionally set `--min-usdc 0.001` to impose your own minimum receipt. The
example limits gas to 1,000,000 and gas price to 1,000 gwei; these ceilings
can be set with `--max-gas` and `--max-gas-price-gwei`. It prints the transaction
hash immediately after broadcast and then waits for a successful receipt.
If confirmation times out, inspect that hash before trying again. Never run
concurrent sends from the same wallet without coordinating nonces.

Broadcast uses the remote computer's Monad RPC and Web3.py's
[`sign_transaction` / `send_raw_transaction`](https://web3py.readthedocs.io/en/stable/transactions.html).
Contract addresses follow the
[Uniswap v4 Monad deployments](https://developers.uniswap.org/docs/protocols/v4/deployments).

## Server

The endpoints are part of `lp-api` on port 8100 and use the existing local
RPC router (`LP_RPC_PRICE_URL`, default `http://127.0.0.1:8545`). Cloudflare's
existing API hostname routes them automatically. No new tunnel is needed.

## Domain paths and Cloudflare

The website is `https://lp.chayu123.me/`. The API hostname root
`https://api.chayu123.me/` has no page and returns 404; use
`/api/health` for a browser health check and POST to the swap paths.
Cloudflare currently rejects the default `Python-urllib` User-Agent with
HTTP 403 / error 1010. The example identifies itself as
`User-Agent: monad-buy-usdc/1.0`, which is accepted by the current configuration.
