# Option-Lot-Constrained Hedging of Automated Market Maker Liquidity

**Status:** complete first academic manuscript, not journal-submission ready. The paper is a quantitative-finance research study, **not** evidence of executed or profitable trades.

## Manuscript

- \`main.tex\` and \`intro_model.tex\`, \`theory.tex\`, \`method.tex\`, \`experiments.tex\`, \`discussion.tex\`: 11pt full English mathematical research article.
- \`data/paper_results.json\`: small, sanitized result manifest with archived real pool and Deribit option snapshot IDs, exact hedge costs, continuous-price stress residuals, MILP gaps and fee-study scope.
- \`verify_paper_results.py\`: zero-dependency data-level consistency tests; **does not** independently rerun all source MILPs.
- GitHub Actions \`.github/workflows/build-lp-option-paper.yml\`: compiles PDF with \`latexmk\`, tests numeric consistency, uploads PDF as an Actions artifact. Never opens a wallet or submits a trade.

Compile locally:

\`\`\`bash
python3 verify_paper_results.py
latexmk -pdf -file-line-error -halt-on-error -interaction=nonstopmode main.tex
\`\`\`

For the source data, do **not** copy \`rpc/data/rpc.db\` into a public paper distribution: its historical endpoint URLs may contain credentials. Paper inputs are the **public, sanitized JSON artifacts**, not a credential-bearing RPC catalog.

## Scope of the result

The main motivation is **practical quantization** of previously established AMM option-replication ideas. Static LP convexity replication is not a novel discovery; the manuscript explicitly cites Deng et al. (2023), *Operations Research Letters*, doi:10.1016/j.orl.2023.03.002, and Milionis et al. on LVR.

Main statements:

1. For smooth AMM divergence \(0\le D''\le M\), linear interpolation with strike gap \(h\) yields uniform error at most \(Mh^2/8\). This is a classical approximation estimate used as a frictionless benchmark.
2. For an isolated piecewise-linear Bin kink of magnitude \(q\), a hedge with same-strike contract increment \(m\), and no other strikes in radius \(h\), any residual must have worst absolute error at least \(h\,|q-nm|/4\). The result is **local and conditional**, not a universal lower bound for all AMMs.
3. The integer hedge solver is evaluated by an exact continuous-price maximum-absolute-residual oracle; positive MILP gaps are reported and **never** represented as a proof of globally optimal integer portfolios.
4. Fee-growth observations are real public on-chain Bin counter changes but actual user LP fees are **simulated**, not realized. They are not contemporaneous with hedge books and cannot support an arbitrage P&L claim.

## Original data / experimental provenance

| Paper results | Original source | Archived artifact / code |
| --- | --- | --- |
| Meteora 4-bp DLMM SOL \(\pm10\%,\pm20\%\) synthetic portfolio, Oct 9 08:23 UTC | [Successful Actions run 37910170471](https://github.com/evaristebernhard/arbitage/actions/runs/37910170471) | Artifact \`11605754511\`, \`research/dlmm_option_window_strategy.py\` |
| Independent Solana Raydium CPMM / CLMM, Oct 9 ~10:08 UTC | [Public Solana cross-protocol paper-input report](../../docs/sol-cross-protocol-option-screen-20261009.md) | Archived public artifact \`11609717952\`, \`research/sol_cross_protocol_option_screen.py\` |
| ETH Uniswap V2 / V3, Oct 9 ~10:18 UTC | [Successful Actions run 37916764413](https://github.com/evaristebernhard/arbitage/actions/runs/37916764413) | Artifact \`11609824369\`, \`research/eth_uniswap_option_screen.py\` |
| Actual DLMM Bin fee counter increments during a distinct 704-second interval | [Successful Actions run 37928429533](https://github.com/evaristebernhard/arbitage/actions/runs/37928429533) | Artifact \`11614961224\`, \`research/dlmm_fixed_bin_cohort.py\` |

**Note:** The source workflows have their own unit tests, MILP solvers, Bin price normalization, fee arithmetic and worst-error verification. The PDF manuscript is a research synthesis of those previously reproducible results. In particular, the 4-bp DLMM full-pool proportional inventory is not equivalent to an actual single Meteora PositionV2. Deribit displayed asks are indicative, not executable quotes guaranteed for the whole multi-leg portfolio.

## Before formal submission

Obtain author affiliations, ensure all primary literature metadata, improve novelty comparison beyond Deng et al. (2023) and LVR, audit the linear/fee-growth reconciliation with actual claimed LP fees, create synchronized independent-date observations and expanded statistical robustness, verify all exchange settlement specifics, and select an appropriate publication venue. Current evidence supports a technical empirical case study, not risk-free alpha.
