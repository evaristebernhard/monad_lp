# Practitioner-readiness build prompts

Paste these into Cursor one at a time, in order. Each is self-contained.
Prompts 1 and 2 are the gates: without them a practitioner cannot point the
tool at their own work. 3-5 are credibility. 6-7 are polish.

Run `python -m pytest tests/test_v2_*.py -q` green before starting each one.

---

## Prompt 1 — Bring your own data (file upload)

```
Context: algotrader (this repo) can only load prices for a ticker string from
Yahoo, via `load_ohlcv(symbol, start, end, interval, source)` in
algotrader/data.py. Practitioners have their own adjusted data — their own
CSVs, Parquet files, minute bars, futures continuous contracts. Right now
there is no way to test a strategy on data they bring. Fix that.

Goal: a user can hand the Lab a CSV/Parquet file (CLI, Python API, or the
Gradio UI) and get the full validation panel on their own prices.

Requirements:

1. In algotrader/data.py add:
     load_user_ohlcv(source: str | Path | pd.DataFrame,
                     symbol: str | None = None,
                     interval: str | None = None) -> MarketData
   - Accepts a .csv, .parquet, or an in-memory DataFrame.
   - Column mapping is forgiving: case-insensitive, and accepts common aliases
     (Date/Datetime/timestamp/time -> index; Adj Close/adj_close/AdjClose ->
     close; o/h/l/c/v and Open/High/Low/Close/Volume). If `volume` is missing,
     fill with NaN and record it in the note rather than failing — only
     `close` is strictly required; if open/high/low are missing, synthesise
     them from close and set a note saying intrabar tests are degraded.
   - Reuse the existing `_normalise(df)` helper so the frame matches
     OHLCV_COLUMNS and is sorted with a unique DatetimeIndex.
   - Infer `interval` from the median index spacing when not given.
   - Raise a clear, actionable ValueError for: unparseable dates, a
     non-monotonic index it can't sort, fewer than 120 rows, all-NaN close,
     non-positive prices, and duplicate timestamps. Each message must name the
     specific problem and the fix. No silent coercion.

2. Add "user" to the MarketData provenance vocabulary in algotrader/types.py.
   MarketData.is_real must return True for "user". Update the docstring.

3. In algotrader/lab.py add a `data` field to LabConfig:
       data: str | Path | pd.DataFrame | MarketData | None = None
   When set, run_lab uses it and SKIPS the Yahoo download entirely (symbol/
   start/end/interval/source are then only used for labelling, and `start`/
   `end` still slice the frame if provided). When None, behaviour is exactly
   as today. Do the same for PortfolioLabConfig in algotrader/portfolio_lab.py,
   accepting either a directory of per-symbol files or a long-format
   (date, symbol, close, ...) file, and wire it through algotrader/panel.py's
   `load_panel`.

4. CLI (algotrader/cli.py): add `--file PATH` to the `lab` and `portfolio`
   subcommands, mutually exclusive with `--source`. `--symbol` becomes the
   display label when `--file` is used.

5. Gradio (app.py, "The Lab" tab): add a `gr.File` upload accepting .csv and
   .parquet. When a file is present it overrides the symbol dropdown. The
   provenance line (around app.py:135 and app.py:520) must show the uploaded
   filename and row count. Do the same on the Portfolio tab.

6. Tests in tests/test_v2_data_user.py covering: CSV round-trip, Parquet
   round-trip, DataFrame passthrough, each alias-mapping case, close-only
   input, and every ValueError branch. Add one end-to-end test that runs
   run_lab(LabConfig(data=<tmp csv>, strategy="sma_cross", n_permutations=20))
   and asserts a grade comes back and market.source == "user".

Constraints:
- Do not change the meaning of the existing `source` values or break any
  existing call signature. `load_ohlcv` keeps working exactly as it does now.
- No new required dependencies. Parquet support should degrade to a clear
  error if pyarrow is absent rather than making it a hard requirement.
- Match the existing code style: module docstrings explaining *why*, type
  hints everywhere, `__all__` maintained, no comments restating the code.
```

---

## Prompt 2 — Bring your own strategy, positions, or live track record

```
Context: algotrader/cli.py restricts --strategy to `choices=sorted(REGISTRY)`,
eleven built-in textbook rules. A practitioner's strategy is their own code,
and very often all they have is a positions series or a returns series they
already computed elsewhere (including a real live track record). There is no
entry point for any of that. Add three.

Goal: a practitioner can validate (a) their own strategy function, (b) a
positions/target-exposure series, or (c) a bare returns series, against as much
of the validation panel as each input can support.

Requirements:

1. Public strategy registration. algotrader/strategies.py has a private
   `_register`. Expose a supported API:
       register_strategy(key, name, fn, *, family="custom", description="",
                         params=None, grid=None) -> Strategy
   plus a `@strategy(...)` decorator. Export both from algotrader/__init__.py.
   In the CLI, replace the hard `choices=` on `--strategy` with validation
   against REGISTRY at parse time (so plugins registered via `--plugin
   my_module.py` still work), and add `--plugin PATH` that imports a user
   Python file before resolving the strategy name.

2. Positions mode. In algotrader/lab.py add:
       run_lab_on_positions(prices, target, cfg: LabConfig | None = None,
                            n_trials: int = 1) -> LabReport
   `prices` is anything load_user_ohlcv accepts; `target` is a Series of
   desired exposure (or a path/DataFrame column). Align target to the price
   index, and run the FULL panel: run_backtest, cost stress, permutation
   (the permutation callable just returns the fixed target realigned to the
   shuffled frame — think carefully about whether that is the right null and
   document the answer in the docstring), DSR with the caller's n_trials,
   MinTRL, benchmark comparison. PBO and walk-forward require re-fitting, so
   they are unavailable unless the caller supplies a trial matrix — mark them
   unavailable rather than faking them.

3. Returns mode. In algotrader/lab.py add:
       run_lab_on_returns(returns, *, periods_per_year=None, n_trials=1,
                          benchmark=None) -> LabReport
   for a bare P&L or live track record. Available: PSR, DSR, MinTRL,
   benchmark comparison, drawdown/vol metrics, and a significance test via a
   STATIONARY BOOTSTRAP of the returns (add
   algotrader/validation/bootstrap.py implementing Politis-Romano with an
   automatic block length) — bar permutation does not apply because there is
   no price path to shuffle. Unavailable: cost stress, PBO, walk-forward.

4. CRITICAL — the score must not silently fake missing tests.
   algotrader/verdict.py currently defaults every missing component to 50.0,
   which would quietly invent a passing grade for a returns-only run. Change
   `reality_score` to take an explicit set of available components,
   renormalise WEIGHTS over only those, and return the available/unavailable
   split in its result dict. The verdict text must state plainly which tests
   did not run and what that means, e.g. "Walk-forward and overfitting tests
   did not run on a returns-only input, so this grade is measured on 55% of
   the panel." Keep the existing full-panel behaviour byte-identical when all
   five components are present — add a regression test that pins this.

5. CLI: `--positions-file PATH [--positions-column NAME]` and
   `--returns-file PATH [--returns-column NAME]` on the `lab` subcommand,
   plus `--n-trials INT`.

6. Gradio: a fourth tab, "Bring your own", with a file upload and a radio for
   OHLCV / positions / returns, an n_trials number input, and the same result
   rendering the Lab tab uses. Show the unavailable-components notice
   prominently, not in fine print.

7. Tests in tests/test_v2_byo.py: registration + decorator, plugin loading,
   positions mode matches run_lab when given the same strategy's own output,
   returns mode on a known-Sharpe synthetic series, weight renormalisation
   arithmetic, and the pinned full-panel regression.

Constraints:
- run_lab, LabConfig, LabReport and reality_score keep their current public
  signatures. LabReport gains fields; it loses none.
- Reuse run_backtest and the existing validation modules. Do not fork the
  engine.
```

---

## Prompt 3 — Make the selection-bias charge honest

```
Context: algotrader/lab.py:171 sets `n_trials = max(len(trial_sharpes), 1)`,
where trial_sharpes comes from `strategy.grid(limit=cfg.grid_limit)` — the
grid THIS TOOL swept, capped at 40. It is not the number of variants the human
actually tried before showing up. Deflated Sharpe is 25% of the Reality Score,
so any pre-tuned strategy is currently undercharged for selection bias. That
is the exact failure the tool exists to catch, so fix it.

Requirements:

1. Add `n_trials: int | None = None` to LabConfig. When set, it overrides the
   grid-derived count and is passed to deflated_sharpe_ratio. When None,
   behaviour is as today but the report records that the count was inferred.

2. LabReport.trials gains: {"n", "source": "user"|"grid", "grid_size",
   "sharpes", "labels", "matrix_shape"}.

3. When the count was inferred, verdict.py adds a flag:
   "Deflated Sharpe charged for the {grid_size} variants this tool swept. If
   you tried more before arriving, set n_trials — the penalty grows with the
   square root of the search."
   When the user supplied it, state the number they gave.

4. Add `effective_n_trials(trial_sharpes) -> float` in
   algotrader/validation/deflated_sharpe.py: correlated variants are not
   independent trials, so estimate the effective count from the correlation
   structure of the trial return matrix (cluster or eigenvalue-based; cite the
   approach in the docstring). Report it alongside the raw count as an upper
   bound. Do not change the default scoring path yet — surface it as
   information.

5. CLI `--n-trials`, Gradio number input with a tooltip explaining that this
   is the one input the tool cannot observe.

6. Tests: override propagates, DSR falls as n_trials rises, effective count
   is <= raw count and equals it for uncorrelated trials.
```

---

## Prompt 4 — Cost model a practitioner will believe

```
Context: algotrader/types.py CostModel is flat commission_bps + slippage_bps
plus a single global short_borrow_bps=50 for every name. Robustness is 10% of
the Reality Score and rests entirely on those constants, and "survives 3x
costs" is an arbitrary threshold.

Requirements:

1. Replace the fixed 3x stress with a BREAKEVEN COST MULTIPLE: solve for the
   friction multiplier at which net Sharpe crosses zero (bisection on the
   existing run_backtest / fast_sharpe path; cap the search and report
   ">10x" beyond it). Report it in cost_stress as "breakeven_multiple". Keep
   sharpe_1x/sharpe_3x/ratio so the score weights are unchanged, but make the
   breakeven number the headline in the UI and verdict text — it is directly
   comparable to a desk's real execution costs.

2. Spread estimation from OHLC. Add algotrader/costs.py with
   `corwin_schultz_spread(df) -> pd.Series` (Corwin & Schultz 2012, cite it)
   and a helper that turns it into an estimated per-side slippage in bps.
   Add `estimate_costs(market) -> CostModel` that builds a data-driven default,
   and surface it in the UI as "estimated from your data: X bps" next to the
   manual override.

3. Size-aware impact. Add optional `adv_participation` to the cost path:
   slippage scales with sqrt(order_notional / ADV) using the volume column
   already loaded. Add `capital` sensitivity — the same strategy at $1m and
   $100m should not have the same cost line. Expose an optional
   `costs_fn: Callable[[pd.Series, float], float] | None` on LabConfig so a
   practitioner can supply their own model wholesale.

4. Per-name borrow instead of one constant: accept a dict or Series of borrow
   rates keyed by symbol in the portfolio path; keep the scalar as the
   fallback default.

5. Tests in tests/test_v2_costs.py: breakeven bisection converges and is
   monotonic in Sharpe, Corwin-Schultz on a known synthetic spread, impact
   scales with size, custom costs_fn is actually called, per-name borrow
   applies the right rate to the right name.

Constraints: CostModel stays backward compatible — existing constructor calls
must work unchanged.
```

---

## Prompt 5 — A report artifact that survives a committee

```
Context: the CLI only prints JSON to stdout. A practitioner needs to attach a
result to a memo and reproduce it in six months. There is no run manifest and
no schema version.

Requirements:

1. Add algotrader/report.py with `build_report(report) -> dict` producing a
   versioned payload: {"schema_version": "1.0", "manifest": {...},
   "config": {...}, "metrics": {...}, "validation": {...}, "verdict": {...}}.
   The manifest records: algotrader __version__, python version, UTC
   timestamp, seed, data source + provenance note, symbol, date range, row
   count, a sha256 of the price series bytes, the resolved n_trials and its
   source, and the git sha when the working tree is a repo (plus a dirty
   flag).

2. `--out PATH` on lab/portfolio/arena writing that JSON. `--format
   json|html` with HTML producing a self-contained single-file tearsheet:
   score and grade, the flag list, the permutation null histogram with the
   observed statistic marked, equity vs benchmark, walk-forward fold table,
   the manifest, and the limitations section from Prompt 7. Inline the CSS
   and embed charts as base64 PNGs or inline SVG — no external requests.
   Reuse algotrader/charts.py.

3. `python -m algotrader.cli verify report.json` re-runs from the manifest and
   diffs the headline numbers, reporting any drift. This is the single most
   convincing feature for this audience.

4. Gradio: a download button for both formats.

5. Tests: schema round-trip, manifest determinism under a fixed seed, hash
   changes when data changes, HTML contains no http(s) references, verify
   passes on a fresh run and detects a deliberately altered metric.
```

---

## Prompt 6 — Verdict copy: harsh on the evidence, not the user

```
Context: the validation panel is calibrated to be harsh and that should not
change. But some framing copy attacks the reader instead of the evidence,
which costs credibility with practitioners and retention with everyone else.

Requirements:

1. algotrader/verdict.py: every verdict scoring below 55 must name the single
   BINDING CONSTRAINT — the one change that would most move the score —
   derived from which component scored lowest and which flags fired. Format:
   "<verdict>. The binding constraint is <X>: <what would change it>."
   Examples: too few trades -> the trade count and the number needed; failed
   permutation -> that no amount of tuning fixes a non-significant edge, more
   data or a different rule will; high PBO -> reduce the search space.

2. Rewrite the GRADES headlines to describe the evidence rather than judge the
   work: "Mostly luck" and "Indistinguishable from randomness" become
   statements about what the tests could and could not separate. Keep them
   short and keep the A-F scale and all thresholds exactly as they are.

3. Delete the "the score exists to be screenshotted" line from the
   verdict.py module docstring.

4. Replace "Most strategies people post online score below 40" in README.md
   (~line 112), SPACE_README.md (~line 43) and app.py (~line 596) with the
   checkable calibration claim: buy-and-hold and a coin flip run as controls
   in every arena, and on most symbols and windows buy-and-hold wins.

5. Do not change any threshold, weight, cap or score arithmetic in this
   prompt. Copy only. Add tests asserting every sub-55 verdict string contains
   a binding-constraint clause, and that the score for a fixed input is
   unchanged from before this prompt.
```

---

## Prompt 7 — Limitations, intervals, and packaging

```
Requirements:

1. Confidence intervals, not point estimates. Add
   `sharpe_confidence_interval(returns, periods_per_year, alpha=0.05)` to
   algotrader/metrics.py (Lo 2002 standard error, adjusted for skew/kurtosis).
   Display Sharpe as "1.24 [0.31, 2.17]" everywhere it is shown. A wide band
   is itself the finding.

2. Add docs/LIMITATIONS.md and link it from README, the HTML report and the
   "How it works" tab. It must state at minimum: no point-in-time
   fundamentals; no borrow availability; no market impact unless configured;
   Yahoo adjustment quirks and the ~7-day intraday history limit; survivorship
   is measured only for the portfolio path; and — most important — that bar
   permutation destroys autocorrelation and volatility clustering, so it is
   the WRONG null for any strategy that trades those. Say explicitly that
   `--null block` exists for that case and when to prefer it. Naming your own
   method's failure modes is the point.

3. Surface the cross-sectional within-date weight permutation prominently.
   It is the most methodologically interesting thing in the repo and it is
   currently one README paragraph.

4. Packaging: add pyproject.toml (setuptools or hatchling), console entry
   point `algotrader = algotrader.cli:main`, extras {space, full, dev},
   requires-python >=3.11. Keep requirements*.txt working. Add a GitHub
   Actions job that builds the wheel and runs tests/test_v2_*.py against the
   installed package rather than the source tree.
```
