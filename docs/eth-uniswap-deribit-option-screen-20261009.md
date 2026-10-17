# Ethereum Uniswap V2/V3 versus Deribit ETH_USDC options — 2026-10-09

This is a **read-only, static, hypothetical-position screen**, not realized LP revenue, an order or a fillable trade recommendation.

## Verified data provenance and ETH option units

[Successful GitHub Actions run 37916764413](https://github.com/evaristebernhard/arbitage/actions/runs/37916764413) and [research JSON artifact 11609824369](https://github.com/evaristebernhard/arbitage/actions/runs/37916764413/artifacts/11609824369).

- Public Deribit index at **2026-10-09 10:18:35 UTC**: **$2,497.74/ETH**, expiry **2026-10-23 08:00 UTC** (**13.904 days** remaining).
- **38** ETH_USDC options on the expiry; **11** relevant OTM ETH ask books successfully read.
- Verified on each selected instrument: one full contract represents **1 ETH**, minimum positive order **0.1 contract** (0.1 ETH). This is NOT SOL's 10-SOL whole-contract size. Order book amounts are underlying ETH and premiums are USDC per ETH.
- Per-option quantities are optimized as **integer increments of 0.1 ETH**. For example, 2 units = 0.2 Deribit contracts, not 2 full contracts. See [Deribit contract specs](https://support.deribit.com/hc/en-us/articles/31424932728093-Linear-USDC-Options).

## Hedge comparison: fixed $10k principal, ±10% terminal ETH stress range

Same ETH expiry and public ask book across every model; CLMM positions explicitly hypothetical. Objective: indicative ask-ladder premium + estimated exchange fee + 1 × exact max absolute HODL-relative nonlinear residual across stress prices.

| Position principal risk curve | Number of 0.1-contract lots | Actual full-contract equivalents | Cost incl. indicative fees | Worst unhedged HODL-relative loss | Max residual after hedge |
| --- | ---: | ---: | ---: | ---: | ---: |
| Uniswap V2 CPMM | **2** | **0.2 contracts** | **$6.57** | $13.17 | **$4.51** |
| Uniswap V3 hypothetical position ±5% | **40** | **4.0 contracts** | **$225.64** | $392.56 | **$19.01** |
| Uniswap V3 hypothetical position ±10% | **33** | **3.3 contracts** | **$160.09** | $269.12 | **$11.07** |

All three reported risk_certified with an exact continuous-price error oracle and integer lot optimization. They **do not prove expected profitability** or actual multi-leg fillability.

Selected candidate legs (all ETH_USDC October 23 expiry):
- V2: 0.1 of 2350 Put and 0.1 of 2650 Call.
- V3 ±5%: 1.2 of 2450 Put, 0.8 of 2400 Put, 0.1 of 2350 Put; 0.1 of 2500 Call, 1.2 of 2550 Call, 0.6 of 2600 Call.
- V3 ±10%: 0.6 of 2450 Put, 0.5 of 2400 Put, 0.7 of 2350 Put, 0.1 of 2300 Put; 0.7 of 2550 Call, 0.6 of 2600 Call, 0.1 of 2650 Call.

## Live ETH/USDC markets and fee proxy, NOT position-level earnings

At approximately 2026-10-09 10:18 UTC, public pool readings:

| Pool | TVL | 24-hour volume | Old catalog fee tier | Annualized gross pool-wide turnover fee proxy | 13.904-day gross pro-rata fee at $10k, if turnover persists |
| --- | ---: | ---: | ---: | ---: | ---: |
| Uniswap V2 WETH/USDC, pool 0xb4e16d0168e52d35cacd2c6185b44281ec28c9dc | $20.23M | $0.752M | 30 bps | **4.07%** | **$15.50** |
| Uniswap V3 WETH/USDC, pool 0x88e6a0c2ddd26feeb64f039a2c41296fcb3f5640 | $95.74M | $80.88M | 5 bps | **15.42%** | **$58.73** |
| Uniswap V4 native ETH/USDC, pool ID 0xdce6394339af00981949f5f3baf27e3610c76326a700af57e4b3e3ae4977f78d | $34.93M | $6.97M | Unverified Hook-dependent | **Not computed** | **Not computed** |

These are gross, *before unknown pool-specific protocol fee shares*. V2's LP share depends on the fee switch being off; actual V3 position fee shares depend on its tick range/active liquidity. Uniswap V4 may not share V3 fee or Hook behavior. Do not treat dated catalog fee tiers as freshly verified on-chain.

Gross proxy compared to cost plus exact max residual (still only a hypothetical deterministic stress comparison):

- V2: **$15.50** hypothetical gross fees versus **$6.57 + $4.51 = $11.08**. The **$4.42** difference is fragile and can be reversed by Ethereum gas, the fee switch, future volume, spread and other costs.
- V3 hypothetical ±5%: **$58.73** gross pro-rata fee reference versus **$225.64 + $19.01 = $244.65**.
- V3 hypothetical ±10%: **$58.73** versus **$160.09 + $11.07 = $171.16**.

A real V3 concentrated position can earn more *or less* than whole-pool pro-rata while active. The saved data is insufficient to forecast it. Therefore the negative proxies above do not mathematically prove all V3 LP+options strategies are unprofitable.

## Research and production boundaries

1. V2: read exact pool vault reserves, LP token shares and any activated fee switch. Standard constant-product principal is analytically tractable.
2. V3: read actual NFT liquidity, tickLower/tickUpper, feeGrowth checkpoints and pool fee tier. Model finite-range short Gamma and out-of-range time properly.
3. V4: decode actual PoolKey, Hook and protocol/dynamic fee; native ETH is not the same address as WETH. Never apply generic V3 payoff under arbitrary Hook.
4. For additional protocols, compare underlying-specific *same-maturity* option chains and minimum lot size. Do not apply ETH prices to SOL/BTC pools.
5. Include gas, funding, liquidity deposits/withdrawals, book updates, settlement basis and LP position-specific earned fees before claiming any profitable backtest.

Run the read-only research program from repo root:

    python3 -m pip install 'numpy>=1.26,<3' 'scipy>=1.13,<2'
    python3 research/eth_uniswap_option_screen.py --self-test
    python3 research/eth_uniswap_option_screen.py --output eth_uniswap_option_screen.json

No private RPC keys, user wallet addresses, database writes or trades are required.
