# Small LP pools vs expensive options — October 9, 2026

**Read-only, indicative small-pool research, not a realized APY or trading recommendation.**

Reproducibility: successful [GitHub Actions run 37921482691](https://github.com/evaristebernhard/arbitage/actions/runs/37921482691), [sanitized JSON artifact 11611399741](https://github.com/evaristebernhard/arbitage/actions/runs/37921482691/artifacts/11611399741). Inputs: dated Git LFS pool catalogue (many fee values from October 1) and fresh public DexScreener pool TVL, h24 volume, token contracts, transaction count, age and price changes around 2026-10-09 11:05 UTC.

## Discovery summary

Stratified **68 pool candidates** with older catalogue TVL $50k–$3m were live-checked. **61** still satisfied the liquidity and volume thresholds; **20** were canonical SOL/ETH plus stablecoin pairs, **7** canonical stable/stable, **34** other or otherwise not option-hedgeable. This is NOT an exhaustive scan of all 40k+ pools. Canonical WSOL, USDC, Solana USDT, Ethereum native ETH, WETH, USDC, USDT and DAI token addresses are verified against the current response, not inferred from ticker symbols.

Annualized proxy = 365 × current (24h volume / TVL) × *correctly dimensioned historical fee tier* × assumed LP share. For a hypothetical new full-range CPMM LP of initial capital W, pre-entry TVL T and persistent daily volume U:

    F_14_proxy = 14 × U × fee × LP_share × W/(T+W).

**This full-range formula is NOT the realized fee income of CLMM, DLMM or Orca positions**, for which each tick/Bin and future active liquidity share matter. A single h24 observation does not establish future volume.

## Promising canonical volatile/stable pools: fee scenarios, not realized yields

| Protocol / canonical pool | Observed TVL | Observed 24h volume | Old fee-tier scenario | Gross or after-share annualized fee proxy | Warning |
| --- | ---: | ---: | ---: | ---: | --- |
| Raydium Standard SOL/USDT — 7XawhbbxtsRcQA8KTkHT9f9nc6d69UwqCDh6U5EEbEmX | $1.371m | $0.467m | 25 bps, assume AMM V4 88% LP split | **27.3% after share** | MUST decode actual AMM subtype and account fee; USDT/stable basis |
| Meteora DLMM SOL/USDC — HTvjzsfX3yU6BUodCjZ5vZkUrAxMDTrBs3CJaq43ashR | $315.6k | $9.255m | 1 bp | **107.0% gross** | 29.3x daily turnover/TVL, dynamic fees, per-Bin fee share unknown |
| Meteora DLMM SOL/USDC — HRYEjwdo3bZ1TpXKWKcezqiwSV2Ywuh4LxMa2PzoCnG6 | $60.7k | $0.753m | 2 bps | **90.5% gross** | $10k changes pool's size substantially, actual active Bin unknown |
| Orca Whirlpool SOL/USDC — 83v8iPyZihDEjDdY8RdZddyZNyUtXngz69Lgo9Kt5d6d | $73.0k | $1.508m | 1 bp | **75.4% gross** | 20.7x turnover, current tick and fee share unverified |
| Meteora DLMM SOL/USDC — BVRbyLjjfSBcoyiYFuxbgKYnWuiFaF9CSXEa5vdSZ9Hh | $2.075m | $1.912m | 20 bps | **67.2% gross** | Higher base fee, 20-bps Bin step, but actual LP revenue unmeasured |
| Uniswap V4 native ETH/USDT — 0x2287a9620adcbf6250dc71be9ee9b2d3a1ec85a464fc6f5c06669e8d07b61bba | $241.6k | $2.785m | source 100 pips = **1 bp** | **42.1% gross** | Uniswap Hook/current protocol fee unverified, turnover 11.5x TVL |
| Raydium CLMM SOL/USDC — CYbD9RaToYMtWKA7QZyoLahnHdWq553Vm62Lh6qWtuxq | $368.3k | $1.370m | 2 bps, assumed 84% LP split | **22.8% after split** | no tick-level fee income |

One relatively credible *next-to-check* candidate is the Raydium Standard SOL/USDT pool: it contains canonical underlying assets, is an older pool, and would not need expensive narrow-range Gamma hedging if its account truly implements an ordinary full-range CPMM. Assuming 14 identical turnover days, old 25-bps trade fee, 88% LP share and $10k new liquidity, its **initial-pro-rata fee scenario is only about $104 over 14 days**, NOT $2,730. The previous $10k CPMM model had just ~$13.17 maximum HODL-relative principal nonlinear loss inside ±10% SOL terminal stress, and its risk-cost optimizer selected **no long option**. That stress result is NOT a guarantee against SOL directional losses or stablecoin depeg.

Another priority is the **20-bps Meteora** pool BVR... because a high base fee need not be paired with extreme 20x–30x turnover. It is still a Bin-position problem, so a whole-pool 67.2% gross fee proxy must not be assigned to a particular LP.

## Fee unit audit: a discovered 100x fake APR

The existing screener stores Uniswap V4 pool fee in **pips (parts per million)**, not basis points. Official Uniswap V4 documentation: https://developers.uniswap.org/docs/protocols/v4/guides/create-pool . Therefore a raw value 100 represents **0.01% = 1 bp**, and 3000 represents **0.30% = 30 bps**. The script explicitly normalizes V4 by dividing the raw field by 100 when producing bps.

For Uniswap V4 canonical USDC/USDT pool 0xe018f09af38956affdfeab72c2cefbcd4e6fee44d09df7525ec9dba3e51356a5, external [GeckoTerminal labels exactly 0.01%](https://www.geckoterminal.com/eth/pools/0xe018f09af38956affdfeab72c2cefbcd4e6fee44d09df7525ec9dba3e51356a5). Given the screen's $464.3k TVL and ~$211.5k h24 volume, the corrected **gross annualized turnover scenario is only 1.66%**, rather than the incorrect **165.7%**. Different providers' pool TVL numbers disagree; current on-chain Hook/pool fee, dynamic overrides and protocol split need separate verification.

The official [Uniswap fee guide](https://developers.uniswap.org/docs/get-started/concepts/fees) additionally describes activated Uniswap V2 protocol fees under current governance. Hence the old assumption that V2 LPs keep all swap fees is outdated: the new research code uses a conditional expected mainnet LP share of 5/6, subject to pool confirmation. Unit tests cover the conversion and canonical token identities.

## Canonical stable/stable pool fees are generally lower

- Uniswap V3 USDC/USDT 0x7858e59e0c01ea06df3af3d20ac7b0003275d4bf: **7.95% gross** turnover scenario, but only 38 h24 swaps.
- Uniswap V3 DAI/USDT 0x48da0965ab2d2cbf1c17c09cfb5cbe67ad5b1406: **7.09% gross**.
- Raydium CLMM USDC/USDT BZtgQEyS6eXUXicYPHecYQ7PybqodXQMvkjUbP4R8mUU: **4.21% gross**, about **3.54%** after assumed Raydium CLMM split.
- Uniswap V4 USDC/USDT 0xe018...: **1.66% gross** after correcting V4 fee pips.

Stable pairs reduce normal SOL/ETH volatility risks, but leave stablecoin depeg, issuer/bridge, smart-contract, fee-tier and withdrawal risks.

## Why some 1,000% fee figures are not straightforward investment opportunities

Live samples include:
- Meteora SI/USDC AQeDM5qkQnr3Eq17k8ntLzSGLBjuooN45gufviUkWFmX: ~$86.8k TVL, $1.151m daily volume, 25-bps old fee -> **~1,209% gross fee APR proxy**. Pool is ~10 days old; >13x daily turnover/TVL. Token-specific risk is not hedged by SOL/ETH options.
- Raydium CLMM NFLX/USDC E3mugzPiMhKGrDcfcz5ojo7HJBbYA6UMaJvSzbabPLAM: ~$119.8k TVL, ~$350k daily volume, 100-bps old fee -> **~1,065% gross proxy**. Token is new; ticker name alone does not prove legally redeemable equity claims or compatible listed options.
- Exotic and tokenized/wrapped tokens can diverge sharply from their name, issuer or supposed reference value. Losses can eclipse all fee income, and concentrated out-of-range positions stop collecting swaps in many paths.

Do NOT multiply these quoted 24h gross fee ratios by 365 and label them expected or realized APR; fee and token identity must be decoded per protocol and actual LP Position. Fee volatility and inventory adverse selection are correlated.

## Better strategy classification and next step

**A. Canonical full-range CPMM**, reasonable age, active volume and verified fee split: compare 14-day fee *range* to the actual HODL-relative short Gamma without compulsory options. A high-fee smaller V2 pool may offer a better premium-to-risk tradeoff, but an investor retains full directional token exposure.

**B. CLMM/DLMM/Whirlpools**: read live fee growth and actual bin/tick occupancy to compute realistic position revenue, then test wider, partial or **no** hedge before paying for near-ATM options.

**C. Stable/stable**: no need to buy SOL/ETH options to hedge normal 1:1 price movements; model rare depeg tails and contract/redemption risk.

**D. Exotic high-volume small pools**: separately label unhedgeable token tail risk, project/token validation, liquidity exits, age and abnormal routing. High projected fees alone are insufficient.

Recommended next concrete audits: (i) verify Raydium Standard 7Xaw... V4 state/fee, (ii) fetch *Bin fee-growth and position shares* for 1-bp HTvj... and 20-bps BVR... Meteora SOL/USDC pools, (iii) fetch Orca Whirlpool tick arrays and current protocol fee, and (iv) inspect Uniswap V4 PoolKey/Hook/current effective fee for unusual pools. Do not compare gross pool APR to fully hedged option costs without position-specific revenue.

All existing scripts and tests are research only: read-only catalogue and public REST, **no credential-bearing RPC endpoint database is loaded or uploaded**, no transactions or wallet access.


## 8. Subsequent OFFICIAL Raydium pool identity and fee check

The final CI run [37922131371](https://github.com/evaristebernhard/arbitage/actions/runs/37922131371), at **2026-10-09 11:11:30 UTC**, also called Raydium's **official public API V3 /pools/info/ids**, without keys or private RPC URLs.

**Pool 7XawhbbxtsRcQA8KTkHT9f9nc6d69UwqCDh6U5EEbEmX:**
- Pool type **Standard**; onchain program ID as returned by official API: 675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8 (Raydium AMM V4).
- Base mint canonical wrapped SOL and quote mint canonical Solana USDT.
- API feeRate = **0.0025 = 25 bps**, confirming the historical fee-tier interpretation.
- Official API $1,371,029.84 TVL and **$421,959.37/day** swap volume, versus DexScreener ~$1.371m and ~$467k. Official 24-hour gross swap fee sum **$1,054.90**. Separate providers' h24 windows/swap definitions need not match.
- Using official daily volume and published Raydium Standard AMM V4 **88% LP fee share**, a hypothetical $10k proportional full-pool deposit gets **approximately $94 of 14-day fees** if turnover and supply conditions persist unchanged. The earlier ~$104 calculation used the higher DexScreener volume; **$94–$104 is a conditional observation-dependent range, NOT a forecast or actual realized LP fees.**
- Under the prior ±10% SOL-price terminal stress model, ideal full-range CPMM HODL-relative nonlinear loss ~$13.17, and the risk-cost optimizer purchased zero long SOL options. However SOL directional losses, USDT credit/depeg, trading/withdrawal costs and outside-window price paths are NOT protected.

**Control pool CYbD9RaToYMtWKA7QZyoLahnHdWq553Vm62Lh6qWtuxq:**
- Raydium official API returned **Concentrated** and **feeRate = 0.0002 = 2 bps**, matching the old catalogue tier.
- Official reported $368,377.70 TVL and $1,118,237/day volume versus ~$1.370m DexScreener h24 volume, a material provider difference. This is precisely why we cannot assign guaranteed LP yield from pool-wide h24 figures; an actual concentrated Position must obtain fee growth by tick.

**Source:** [Raydium API v3 pool ID endpoint](https://api-v3.raydium.io/pools/info/ids) (query ids=7Xaw... or ids=CYb...), [published LP fee shares](https://docs.raydium.io/raydium/protocol/protocol-fees), and the successful sanitized [Actions run](https://github.com/evaristebernhard/arbitage/actions/runs/37922131371). The study uses no wallet or order placement.
