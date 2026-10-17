# SOL LP static options: explicit construction, finite strikes and contract quantization

Research status (2026-10-08): **theorem + offline tests; no live fill, trade, or profit claim**. This note extends the existing Rust finite_replication construction without changing the production hedge_analysis outputs.

## 1. Payoff to hedge, not the entire LP

Let S>0 be the SOL price in USDC, S0 the entry price, and W0 the initial LP value. For a *fixed* non-rebalanced LP position write V(S) for its principal value (excluding LP fees, rewards and cash flows); let x0=V'(S0) be the initial SOL inventory. The nonlinear loss relative to HODL is

\[
D(S)=W_0+x_0(S-S_0)-V(S),\qquad D(S_0)=D'(S_0)=0.
\]

For Raydium-style constant-product CPMM,

\[
V_{CP}(S)=W_0\sqrt{S/S_0},\quad x_0=W_0/(2S_0),\quad
D_{CP}(S)=\frac{W_0}{2}(\sqrt{S/S_0}-1)^2.
\]

Define L_eff=W0/(2 sqrt(S0)); then D_CP''(S)=L_eff/(2 S^(3/2)).

For a single CLMM position with price bounds 0<A<S0<B and liquidity L,

\[
V_{CL}(S)=\begin{cases}
LS(A^{-1/2}-B^{-1/2}), & S\le A,\\
L(2\sqrt{S}-S/\sqrt B-\sqrt A), & A<S<B,\\
L(\sqrt B-\sqrt A),&S\ge B.
\end{cases}
\]

Here L=W0/(2 sqrt(S0)-S0/sqrt(B)-sqrt(A)). Its nonlinear loss has curvature D_CL''(S)=L/(2 S^(3/2)) on (A,B), and zero outside. These formulas assume ordinary CPMM and CLMM without dynamic fees, jumps in the LP position or rebalancing. A constant-product formula must **not** be used for the StableSwap accounts that currently share the repository's solana_standard directory family.

## 2. Exact one-option per segment theorem

On a segment [a,b], 0<a<b, match curvature density w(K)=L/(2 K^(3/2)) by one vanilla long put if b<=S0, or one vanilla long call if a>=S0. Match its mass and first moment:

\[
q=\int_a^b w(K)dK=L(a^{-1/2}-b^{-1/2}),\qquad
k=\frac{\int_a^b K w(K)dK}{q}=\sqrt{ab}.
\]

The continuous curvature integral and the option q times its strike-k payoff agree when S lies outside [a,b]. Inside [a,b], Jensen gives a nonnegative residual. Direct integration gives

\[
e_{a,b}(S)=
\begin{cases}
L(\sqrt{S}-\sqrt{a})^2/\sqrt{a}, &a<S\le\sqrt{ab},\\
L(\sqrt{b}-\sqrt{S})^2/\sqrt{b}, &\sqrt{ab}\le S<b,\\
0,&S\notin(a,b).
\end{cases}
\]

Thus sup e = L(b^(1/4)-a^(1/4))^2, **exactly** (puts and calls share this residual identity).

Split [A,S0] into N_- and [S0,B] into N_+ intervals, equally spaced in u=S^(1/4). Segment errors have disjoint supports; hence on the entire positive price axis for CLMM,

\[
\boxed{E_{\rm ideal}=L\max\left\{
\frac{(S_0^{1/4}-A^{1/4})^2}{N_-^2},
\frac{(B^{1/4}-S_0^{1/4})^2}{N_+^2}\right\}.}
\]

It follows that each N_side can be chosen *explicitly* as the ceiling of sqrt(L/epsilon) times its fourth-root width. Equal-fourth-root partition is minimax within this **moment-matched, one-option-per-segment family**, not necessarily among all option portfolios.

For CPMM, replace L by L_eff, and take a *finite* risk interval [A,B]: the same exact error holds **on [A,B]**. The unhedged tail residual is L_eff (sqrt(A)-sqrt(S))^2/sqrt(A) for S<A, and L_eff (sqrt(S)-sqrt(B))^2/sqrt(B) for S>B. No finite vanilla portfolio has bounded uniform CPMM absolute error on all S>0, because the extreme-right payoff is eventually affine while D_CP contains -sqrt(S).

## 3. Executable strikes and 10-SOL contract size

Let ideal leg i be (q_i,k_i,side_i), with q_i in SOL payoff units. Select a Deribit-listed strike k'_i of **the same option type and expiry**, and an integer n_i>=0 contracts. If h_i SOL are represented by one contract, actual exposure is q'_i=h_i n_i. For SOL_USDC, current official documentation specifies h_i=10 SOL and a minimum *positive* order of one contract. Zero contracts is an admissible choice.

On a specified study interval I=[A,B], define phi_i(S,k)=(S-k)^+ (call) or (k-S)^+ (put), and M_i=sup_(S in I) phi_i(S,k'_i), which equals (B-k'_i)^+ for a call and (k'_i-A)^+ for a put.

Then a **fully explicit, rigorously certified uniform error** is

\[
\boxed{
\sup_{S\in I}|D(S)-H_{\rm real}(S)|
\le E_{\rm ideal}
+\sum_i q_i |k_i-k'_i|
+\sum_i |q_i-q'_i|M_i.}
\tag{Q}
\]

Proof: subtract the ideal hedge and add it back; the option payoff is 1-Lipschitz in strike, and its absolute value is at most M_i on I. Take absolute values term by term. This still holds when n_i=0; it is a **conservative** certificate, not an equality. Option execution costs and LP fees are not included. Note that quantity rounding may alter the asymptotic call slope, so even CLMM loses its full S>0 bound unless a separate tail-slope condition is imposed.

If nearest-integer rounding is used, |q_i-q'_i|<=h_i/2. If each nearest *same-side listed* strike is within delta/2 and M_i<=R, with total ideal curvature Q=sum_i q_i, the simplified bound is

\[
E_{\rm real} \le L\Delta^2/N^2+\delta Q/2+hRN/2
\]

for proportional continuous left/right allocation N (replace the first term by the exact side-specific formula for integer counts), where Delta=B^(1/4)-A^(1/4), h=max h_i. This surrogate's minimizer is N=(4L Delta^2/(hR))^(1/3), showing the competition between O(N^-2) ideal error and an O(N) worst-case rounding certificate. **It is not an economic-optimal number of traded legs**. Exact actual error can be much lower; some legs should be skipped entirely.

## 4. Compute exact post-rounding error, not just the bound

Between consecutive actual option strikes the hedge payoff H_real is affine. Hence residual D-H_real is convex wherever D is smooth, and its signed maximum is achieved at segment endpoints. However, the maximum **absolute** residual can occur at an interior stationary minimum; endpoint-only checking is invalid.

For a segment with hedge slope c, stationarity D'(S)=c occurs at

\[
S_*=(L_{\rm eff}/(x_0-c))^2 \quad\text{(CPMM)}
\]

or

\[
S_*=(L/(x_0+L/\sqrt B-c))^2 \quad\text{(CLMM within [A,B])}.
\]

Check all risk-interval endpoints, option strikes, S0, and applicable S_* to obtain the *exact* sup of |D-H| in [A,B]. The Python research script implements this finite check; it does not approximate with a grid.

## 5. Concrete illustration — NOT a live market quote

Set S0=112.45 USDC/SOL, W0=10,000 USDC and [A,B]=[0.8S0,1.2S0].

- CPMM: 2 puts + 2 calls yields ideal max residual ~$3.68. Each ideal leg is only ~1.87–2.73 SOL. **Nearest-integer 10-SOL sizing selects zero contracts on all four legs**, so exact residual becomes about $55.73; certificate (Q) is about $104.96.
- CLMM: 4 puts + 4 calls yields ideal max residual ~$9.55 and desired sizes ~9.54–14.49 SOL. Quantizing them changes the risk profile significantly.

This distinction matters more than additional decimals in the ideal error formula.

## 6. Data and reproducibility

The read-only collector at rpc/option/research/fetch_deribit_sol.py requests three public Deribit endpoints: get_index_price, get_instruments, and get_book_summary_by_currency. It filters SOL_USDC options to a common expiry, preserving the listed strikes, bid/ask and contract metadata. The workflow .github/workflows/deribit-sol-research.yml runs unit tests, retrieves observations and uploads the JSON artifact. A failed network request writes a clearly marked fetch_failed JSON and causes a failing step. It **never** inserts stale prices into option_quotes or hedge_analysis, never signs transactions, and requires no secrets.

To run the proof checks and illustrative quantization locally:

    python3 -m unittest discover -s backend/lp-options/research -p 'test_*.py' -v
    python3 backend/lp-options/research/quantized_hedge.py --kind cpmm --n-put 2 --n-call 2

Once a genuinely fetched, two-sided same-expiry strike list is available, pass the downloaded deribit_sol_quoted_strikes.json file via --strikes-json. The research script does not yet model live depth or execution costs, and does not claim executable hedges. The existing production Rust Deribit request builder still needs independent query-serialization verification before inserting quotes into the canonical SQLite database.

Sources: https://support.deribit.com/hc/en-us/articles/31424932728093-Linear-USDC-Options ; https://docs.deribit.com/ ; existing backend/lp-options/src/main.rs.


## 7. First actual Deribit public quote snapshot — 2026-10-08

The GitHub Actions run [37800930733](https://github.com/evaristebernhard/arbitage/actions/runs/37800930733) **succeeded**, including the offline unit tests and a real read-only query to Deribit. Its [artifact 11560377826](https://github.com/evaristebernhard/arbitage/actions/runs/37800930733/artifacts/11560377826) contains the unmodified public quote observations and a separate list of strikes with nonempty two-sided quotes.

- Snapshot time: 2026-10-08 15:27:38 UTC.
- Deribit SOL_USDC index: **109.2471 USDC per SOL**.
- Selected expiry: **2026-10-16 08:00 UTC**; 118 active contracts (59 puts and 59 calls).
- 93 contracts have positive non-crossed bid/ask observations (51 puts, 42 calls).
- The contract metadata reports a **10-SOL multiplier**; for SOL_USDC the minimum order is one contract. API min_trade_amount appears as 10 underlying SOL in the snapshot and should not be mislabelled as 10 independent contracts.

**Reproducible diagnostic (not a feasible execution claim).** Fix W0=10,000 USDC, S0=109.2471, A=0.8S0, B=1.2S0. Apply the proof's fixed fourth-root construction. Each theoretical option leg is snapped to the nearest *two-sided quoted* strike of matching side and the same expiry; amount is rounded to the nearest nonnegative integer of 10-SOL contracts (zero allowed). Assume all desired size could be purchased at the observed single best ask (depth not yet checked).

| Static LP | Ideal legs | Ideal worst residual | Actual rounded contracts | Exact worst residual on [A,B] | Sum of observed top asks |
| --- | ---: | ---: | ---: | ---: | ---: |
| CPMM | 2 puts, 2 calls | $3.68 | 0 | **$55.73** | $0 |
| CLMM | 4 puts, 4 calls | $9.55 | 8 (one per leg) | **$114.29** | **$73.00** |

The quantity and strike **rounding** completely invalidate the theoretical $3.68/$9.55 error values as representations of the actual positions. The CLMM eight-leg ask sum does *not* imply fillability or profitability: level-1 price alone provides no cumulative book depth, immediate fill guarantee, borrowing/funding/clearing cost or subsequent position maintenance cost. OI and 24h volume are not executable best-ask size. Premiums are USDC per SOL times the 10-SOL contract multiplier.

Actual diagnostics can change even within minutes; do not present this table as a current quote screen. Raw artifacts in Actions expire per workflow retention policy. For a true executable cost study, query get_order_book for each chosen option and price the required integer contracts across actual ask levels, preserving one expiry and the full option contract metadata.

The first smaller collection run [37800647067](https://github.com/evaristebernhard/arbitage/actions/runs/37800647067) only retained the 20 closest strikes on each side; it is deliberately **not** used for the final diagnostic above.
