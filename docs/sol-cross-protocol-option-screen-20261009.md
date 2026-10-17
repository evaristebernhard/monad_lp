# Other LP protocols: same-expiry SOL options, pool fee hurdles and ETH context (2026-10-09)

**Empirical, read-only first-pass research. No real wallet LP position, fill confirmation, prediction of future fees or trading.**

## 1. Verified GitHub Actions output

The first successful Actions run [37915656848](https://github.com/evaristebernhard/arbitage/actions/runs/37915656848) uploaded JSON artifact [11609717952](https://github.com/evaristebernhard/arbitage/actions/runs/37915656848/artifacts/11609717952), with both read-only option-chain and 20-level relevant books plus public pool observations.

- Option snapshot: **2026-10-09 10:08:10 UTC**; SOL_USDC index **110.3539**, common expiry **2026-10-23 08:00 UTC** (13.911 days remaining).
- Deribit listing: **112** options (100 two-sided); **34** relevant option books successfully fetched, zero depth errors. The book observations and pool observations are sequential, *not an atomic multi-leg fill*.
- Live public SOL/USDC pool h24 observations: **~10:08:19 UTC**, derived from DexScreener pair endpoint, pool identity checked against canonical WSOL and USDC mint addresses.
- The option/LP model compares equal **$10,000 initial LP principal** and a uniform **terminal ±10% spot pressure interval**, with option premium charged at stored best ask depth plus indicative standard exchange fee; the minimax penalty in the MILP is \`lambda=1\` USD per 1 USD maximum absolute HODL-relative residual.
- **The CLMM position interval is an explicitly hypothetical separate input**, not an actual NFT/tick position. The solver checks the exact worst absolute residual *even when the stress price is outside the LP's actual liquidity range*. CPMM has global continuous curvature. Its optimum under this objective can be no option at all.

## 2. Current pool snapshots and *non-position-level* fee estimates

The catalog fee tier is taken from a dated **2026-10-01** SQLite row and HAS NOT been refreshed from on-chain pool config. Raydium's protocol-level split is documented by [official Raydium fee rules](https://docs.raydium.io/raydium/protocol/protocol-fees). Orca's individual \`protocol_fee_rate\` needs on-chain account verification; the pool-wide gross APR is not an LP APR. Meteora's actual current base fee can differ with variable volatility-linked components.

| Pool (SOL/USDC) | Fresh TVL | Fresh 24h volume | Gross fee tier from old catalog | Protocol-share assumption | Pool pro-rata fee APR proxy |
| --- | ---: | ---: | ---: | --- | ---: |
| Raydium AMM V4 \`58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2\` | **$24.035M** | **$4.878M** | 25 bps | 88% LP if legacy AMM V4 | **16.30%** |
| Raydium CLMM \`3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv\` | **$7.603M** | **$34.040M** | 4 bps | 84% LP (pool config still to check) | **54.91%** |
| Orca Whirlpool \`Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE\` | **$27.751M** | **$124.442M** | 4 bps | Not decoded; 75–100% scenario | **65.47% gross** |
| Meteora DLMM reference \`5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6\` | **$4.112M** | **$28.087M** | 4 bps base | 90% LP from previously saved on-chain share | **89.76% base fee proxy** |

Gross fee proxy \`APR_gross = 365 * h24_volume/tvl * (catalog_fee_bps/10000)\`; after-share proxy multiplies \`alpha_LP\`. This is **not** real individual fee capture or future expected APR, particularly for concentrated or Bin positions. Raydium Standard subtype itself must be verified against on-chain account data rather than inferred only from screener naming.

## 3. Equal-SOL options hedge, fixed stress interval

The target is the HODL-relative *principal* nonlinear dollar loss \`D(S)=V(S0)+V'(S0)(S-S0)-V(S)\`. This is not mark-to-market total LP loss. For CPMM \`D=W0/2*(sqrt(S/S0)-1)^2\`; for CLMM a fixed sqrt-liquidity formula has curvature \`L/(2*S^(3/2))\` only inside the stated position range. With actual Deribit long put/call payoffs H, the optimized quantity is \`C_option + max_(terminal stress)|D-H|\`.

The following results are computed using the **same Deribit quotes, 13.911-day expiry, $10k size and ±10% terminal spot stress interval**. CLMM columns are distinct hypothetical ranges, not protocol-native *fees*.

| AMM payoff, assumed LP interval | Long SOL options selected | Actual ask-depth premium + estimated fees | Maximum unhedged D | Exact maximum \`|D-H|\` after hedge | Premium + max residual |
| --- | ---: | ---: | ---: | ---: | ---: |
| Raydium AMM V4 / ideal CPMM | **0** | **$0** | $13.17 | **$13.17** | **$13.17** |
| Raydium CLMM or Orca hypothetical LP ±2.5% | **8** | **$318.65** | $448.17 | **$40.91** | **$359.56** |
| Raydium CLMM or Orca hypothetical LP ±5% | **8** | **$279.65** | $392.56 | **$25.30** | **$304.95** |
| Raydium CLMM or Orca hypothetical LP ±10% | **7** | **$197.32** | $269.12 | **$11.86** | **$209.17** |

These were individually optimized over available whole 10-SOL Deribit contracts; all returned \`risk_certified\` under an analytic continuous-price oracle. The solver objective is **not expected P&L** and solver optimality depends on tolerances and the imposed long-only OTM available-strike/bid-depth restrictions. Prices are indicative, book time asynchronous; no fill or funding/settlement costs guaranteed.

### What is the conditional fee hurdle over 13.911 days?

At W0=$10k and assuming the pool's current TVL and h24 volume remain **constant for the entire option term**, its *full-pool-pro-rata benchmark* fee values are approximately:

- Raydium V4: **$62.11**, versus $13.17 worst CPMM nonlinear loss if no options; conditional stress margin **+$48.94** before other costs. Here the optimum is **NOT buying options**, so it should not be represented as a profitable option-trading opportunity.
- Raydium CLMM: **$209.27**, versus $(option cost + worst residual) of **$359.56 / $304.95 / $209.17** for ±2.5% / ±5% / ±10% ranges. Resulting theoretical margins **−$150.29 / −$95.68 / +$0.09**. A real single-range concentrated position's fee income is NOT given by W0/TVL, so these margins do not predict profit.
- Orca: gross fee benchmark **$249.52**. With an *illustrative* 75%–100% LP share that produces **$187.14–$249.52**, to compare to the same CLMM insurance hurdles. For ±10% range the comparison spans **−$22.03 to +$40.35**, depending on fee share and **before** the unknown user-specific active-liquidity factor. The 4-bps catalog fee tier still requires on-chain reconfirmation.
- Meteora DLMM: 90%-share base-fee pro-rata benchmark **$342.08**, but its discrete-Bin position and dynamic fee cannot use the CLMM hedge curve. Refer instead to [Meteora Bin allocation frontier](dlmm-bin-distribution-cross-protocol-frontier-20261009.md). Do not splice an old DLMM hedge cost into this fresh quote screen and call it contemporaneous.

The small theoretical margins in the wider CLMM scenario underscore that an error of a few dollars in realized fees or execution can change the sign; moreover the stress worst residual and ideal fee share are only illustrative. Negative margins similarly do not prove negative *expected* return.

### Exact hedge instruments in this run

- CPMM: **zero options** under \`lambda=1\`. At current model risk scale, buying market options reduces neither the risk-cost objective enough nor guaranteed net P&L.
- CLMM ±2.5%: two contracts each of **110 Put, 109 Put, 111 Call, 112 Call**.
- CLMM ±5%: two each of **109 Put, 108 Put, 112 Call**, one each of **113 Call, 114 Call**.
- CLMM ±10%: one each of **109, 106, 105, 103 Puts** and **112, 114, 115 Calls**.
- All contracts use the **2026-10-23 SOL_USDC** expiry. Strike choices and premiums are not permanent recommendations.

## 4. Ethereum V2 / V3 / V4 discovery

In the SAME Actions run the catalog also furnished real Ethereum WETH/USDC pool IDs, and the public pair API returned:

| Ethereum pool | Canonical pool ID/address | Fresh TVL | 24h volume | Status |
| --- | --- | ---: | ---: | --- |
| Uniswap V2 USDC/WETH | \`0xb4e16d0168e52d35cacd2c6185b44281ec28c9dc\` | **$20.23M** | **$0.752M** | verified tokens |
| Uniswap V3 USDC/WETH | \`0x88e6a0c2ddd26feeb64f039a2c41296fcb3f5640\` | **$95.77M** | **$80.92M** | verified tokens |
| Uniswap V4 ETH/USDC | \`0xdce6394339af00981949f5f3baf27e3610c76326a700af57e4b3e3ae4977f78d\` | Requires native ETH token representation check | Requires fresh check | raw pool ID is not an ordinary V2 pair contract |

Uniswap V2 corresponds to standard constant-product \`sqrt(S)\` short-Gamma LP principal, V3 has a bounded range, and V4's hook and fee logic must be verified per pool (a catalog value like \`fee_bps=3000\` may be in a different unit convention and MUST NOT be treated as 3000 basis points). **Do not apply SOL option prices to ETH pools.** Next: collect same-expiry Deribit **ETH_USDC** options with the **0.1-contract minimum order** (1 ETH multiplier per one full contract), decode exact pool fee tier and actual position bounds. See [official Deribit option multipliers](https://support.deribit.com/hc/en-us/articles/31424932728093-Linear-USDC-Options).

## 5. Mathematical conclusion and actual missing records

Cross-protocol comparison should fix: (i) initial capital, (ii) underlying/expiry, (iii) stress interval, (iv) risk aversion, (v) option depth/fees. Separate variables are **actual position width/shape and path-dependent fee growth**. The current study fixes the first four and uses matched same-expiry public books.

For fee economics, each position i earns a share of fee-generating flow only while active:
\[
F_T^{LP}=\int_0^T f_i(t)\rho_i(t)\,dV_i(t).
\]
Pool h24 volume and TVL do not determine \`rho_i(t)\`; CLMM / Orca require tick liquidity and Position ID, and DLMM requires bin shares and fee growth. Standard V4 needs its actual vault reserves/LP token share. The data collection priority is **account-derived fee rates/position liquidity shares at timestamp T**, followed by time-aligned price/volume and actual option depth if conducting full backtests.

### Reproduce

    python3 -m pip install numpy scipy
    python3 rpc/option/research/fetch_deribit_sol.py --days 12 --per-side 120 --output chain.json
    python3 rpc/option/research/fetch_sol_depth.py --input chain.json --output depth.json --depth 20
    python3 research/sol_cross_protocol_option_screen.py --catalog data/lp_screener.db --option-depth depth.json --output comparison.json

GitHub Actions at [sol-cross-protocol-options.yml](../.github/workflows/sol-cross-protocol-options.yml) runs the same workflow on push/manual invocation and, after merging into main, daily at **03:25 UTC**. This code does NOT read uploaded RPC endpoint URLs or wallet keys; it uses public REST and a read-only catalog SQLite, and writes only JSON research artifacts. Everything has an explicit observation timestamp and data-quality caveats.


## 6. Second independent live confirmation (2026-10-09 10:11 UTC)

After correcting the native-ETH identifier for Uniswap V4, [Actions run 37915942977](https://github.com/evaristebernhard/arbitage/actions/runs/37915942977), [raw artifact 11608604006](https://github.com/evaristebernhard/arbitage/actions/runs/37915942977/artifacts/11608604006), again finished successfully with 34 option books, producing:

- SOL index **$110.2662**; Oct 23 expiry.
- CPMM: **0 contracts, $0 cost, $13.17 max nonlinear loss** under ±10% stress.
- Hypothetical CLMM **±2.5%** position: **8 contracts, $316.65 indicative cost, $40.73 residual**.
- Hypothetical CLMM **±5%** position: **8 contracts, $278.65 cost, $25.51 residual**.
- Hypothetical CLMM **±10%** position: **6 contracts, $180.98 cost, $26.30 residual**.

These different integer combinations demonstrate sensitivity to a changing ask book. The first run and second run must never be combined into one fictional simultaneously available hedge.

Updated 24h fee-proxy rates on this second sample: Raydium AMM V4 **16.32%** after 88% share; Raydium CLMM **54.95%** after 84% share; Orca **65.53% gross**, share unknown; Meteora **89.02%** under the stored 90% base-fee LP-share assumption. Public total TVL/volume data remain **pool-wide**, not a user Position's actual earned revenue.

Uniswap V4's ETH/USDC pool ID in the catalog uses **native ETH** rather than wrapped WETH; the initial token-verification filter incorrectly rejected native ETH. With the fix, the same market query verified this pool as ETH/USDC with fresh **$34.93M TVL, $6.97M 24h volume, ETH about $2,497.76**. V4 hook- and pool-specific fees remain unverified, and none of the ETH pools has an ETH options strategy calculated in this SOL-specific job. The two other Ethereum pools confirmed in both runs were Uniswap V2 USDC/WETH (**$20.23M TVL, $0.752M 24h**) and V3 USDC/WETH (**$95.74M TVL, $80.94M 24h**).

**Next explicit task, separate from this SOL job:** collect Deribit ETH_USDC same-expiry order books, respect ETH fractional contracts (1 ETH per 1.0 contract; min 0.1 contract) and match the V2/V3/V4 actual pool fee, position and Hook logic. Do not reuse SOL's 10-SOL whole-contract integer lot model for ETH.
