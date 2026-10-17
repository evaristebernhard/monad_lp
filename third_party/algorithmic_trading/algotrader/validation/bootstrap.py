"""Stationary bootstrap for return-series significance.

Politis & Romano (1994): the stationary bootstrap resamples blocks of random
length (geometrically distributed) so the null preserves short-range
dependence in the return series while destroying long-horizon structure. That
is the right null when you only have a P&L track record and no price path to
shuffle bar-by-bar.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

from ..metrics import sharpe_ratio

__all__ = ["BootstrapResult", "automatic_block_length", "stationary_bootstrap_sharpe"]


@dataclass
class BootstrapResult:
    observed: float
    null: np.ndarray
    p_value: float
    method: str
    n_bootstrap: int
    block_length: int

    @property
    def null_mean(self) -> float:
        return float(np.mean(self.null)) if self.null.size else 0.0


def automatic_block_length(returns: np.ndarray) -> int:
    """Data-driven block length (Politis & White 2004, simplified).

    Uses the first-order autocorrelation of the series to pick a block length
    that preserves local dependence without smearing the entire sample into one
    block.
    """
    arr = np.asarray(returns, dtype=float)
    arr = arr[np.isfinite(arr)]
    n = arr.size
    if n < 20:
        return max(1, n // 5)
    rho = float(np.corrcoef(arr[:-1], arr[1:])[0, 1]) if n > 2 else 0.0
    rho = float(np.clip(rho, -0.95, 0.95))
    if abs(rho) < 1e-6:
        return max(1, int(round(n ** (1.0 / 3.0))))
    # Andrews-style scaling adapted for stationary bootstrap mean block length.
    length = int(np.ceil(1.5 * np.sqrt(n) * ((1.0 + rho) / (1.0 - rho)) ** 0.34))
    return max(1, min(length, n // 3))


def stationary_bootstrap_sharpe(
    returns: np.ndarray,
    *,
    n_bootstrap: int = 500,
    block_length: int | None = None,
    periods_per_year: int = 252,
    seed: int = 0,
    observed: float | None = None,
) -> BootstrapResult:
    """Build a Sharpe-ratio null via Politis-Romano stationary bootstrap."""
    arr = np.asarray(returns, dtype=float)
    arr = arr[np.isfinite(arr)]
    if arr.size < 10:
        raise ValueError("Need at least 10 finite returns for a bootstrap test.")

    block = block_length or automatic_block_length(arr)
    p_restart = 1.0 / block
    rng = np.random.default_rng(seed)

    import pandas as pd

    series = pd.Series(arr)
    obs = float(observed) if observed is not None else sharpe_ratio(series, periods_per_year)
    null = np.empty(n_bootstrap, dtype=float)
    n = arr.size

    for i in range(n_bootstrap):
        sample = np.empty(n, dtype=float)
        pos = 0
        while pos < n:
            start = int(rng.integers(0, n))
            length = 1
            while pos + length < n and rng.random() > p_restart:
                length += 1
            chunk = arr[start : min(start + length, n)]
            end = min(pos + chunk.size, n)
            sample[pos:end] = chunk[: end - pos]
            pos = end
        null[i] = sharpe_ratio(pd.Series(sample), periods_per_year)

    p_value = float((null >= obs).mean()) if null.size else 0.5
    return BootstrapResult(
        observed=obs,
        null=null,
        p_value=p_value,
        method="stationary_bootstrap",
        n_bootstrap=n_bootstrap,
        block_length=block,
    )
