#!/usr/bin/env python3
"""BAMR-Direct: deterministic exact-input routing over independent V2-type pools.

Research / educational prototype; read-only, no RPC calls or transaction signing.
The continuous allocation for any chosen subset is optimal under the constant-
product model with proportional per-pool risk haircuts and fixed per-pool gas.
Enumeration over ALL pool subsets up to max_splits yields a global optimizer
for THIS RESTRICTED, frozen-state model. It does not optimize V3/V4, order books,
shared-pool multi-hop paths, or real blockchain execution.

Usage:
    python monad_pool_selector.py --config monad_pools_example.json
    python monad_pool_selector.py --config monad_pools_example.json --amount 10000 --max-splits 2
"""
from __future__ import annotations

import argparse
import itertools
import json
import math
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class Pool:
    name: str
    reserve_in: float
    reserve_out: float
    fee_bps: float
    incremental_gas_in_out: float
    risk_bps: float = 0.0
    cap_in: float | None = None

    def validate(self) -> None:
        if not self.name:
            raise ValueError('Pool name cannot be empty')
        values = [self.reserve_in, self.reserve_out, self.fee_bps,
                  self.incremental_gas_in_out, self.risk_bps]
        if self.cap_in is not None:
            values.append(self.cap_in)
        if not all(math.isfinite(value) for value in values):
            raise ValueError(f'{self.name}: all numeric fields must be finite')
        if self.reserve_in <= 0 or self.reserve_out <= 0:
            raise ValueError(f'{self.name}: reserves must be positive')
        if not 0 <= self.fee_bps < 10000:
            raise ValueError(f'{self.name}: fee_bps outside [0,10000)')
        if self.incremental_gas_in_out < 0:
            raise ValueError(f'{self.name}: gas cannot be negative')
        if not 0 <= self.risk_bps < 10000:
            raise ValueError(f'{self.name}: risk_bps outside [0,10000)')
        if self.cap_in is not None and self.cap_in <= 0:
            raise ValueError(f'{self.name}: cap_in must be positive')

    @property
    def fee_multiplier(self) -> float:
        return 1.0 - self.fee_bps / 10000.0

    def cap(self, total_input: float) -> float:
        return min(total_input, self.cap_in) if self.cap_in is not None else total_input

    def raw_quote(self, amount_in: float) -> float:
        if amount_in <= 0:
            return 0.0
        a = self.fee_multiplier
        return self.reserve_out * a * amount_in / (self.reserve_in + a * amount_in)

    def risk_weight(self, aversion: float) -> float:
        return 1.0 - aversion * self.risk_bps / 10000.0

    def weighted_quote(self, amount_in: float, aversion: float) -> float:
        return self.risk_weight(aversion) * self.raw_quote(amount_in)

    def weighted_marginal(self, amount_in: float, aversion: float) -> float:
        a = self.fee_multiplier
        denom = self.reserve_in + a * amount_in
        return self.risk_weight(aversion) * self.reserve_out * a * self.reserve_in / (denom * denom)


def validate_problem(pools: list[Pool], amount: float, k: int,
                     aversion: float, base_gas: float = 0.0) -> None:
    if not math.isfinite(amount) or amount <= 0:
        raise ValueError('amount_in must be finite and positive')
    if isinstance(k, bool) or not isinstance(k, int) or k <= 0:
        raise ValueError('max_splits must be a positive integer')
    if not all(math.isfinite(x) and x >= 0 for x in (aversion, base_gas)):
        raise ValueError('aversion and base_gas must be finite and nonnegative')
    if not pools:
        raise ValueError('No pools provided')
    for pool in pools:
        pool.validate()
        if pool.risk_weight(aversion) <= 0:
            raise ValueError(f'{pool.name}: risk weight must be positive')
    if len({pool.name for pool in pools}) != len(pools):
        raise ValueError('Pool names must be unique')


def amount_tolerance(amount: float) -> float:
    return max(amount * 1e-12, math.ulp(amount) * 32)


def score_previous_route(pools: list[Pool], allocation: list[dict], amount: float,
                         k: int, aversion: float, base_gas: float,
                         by_name: dict[str, Pool] | None = None) -> float:
    # The incremental index validates its universe at construction/update.
    # Its caller can supply only the referenced pools to keep rechecks O(K).
    if by_name is None:
        validate_problem(pools, amount, k, aversion, base_gas)
        by_name = {pool.name: pool for pool in pools}
    if not allocation or len(allocation) > k:
        raise ValueError('route must contain 1..K distinct active pools')
    used = set()
    total = 0.0
    score = -base_gas
    for row in allocation:
        name = row.get('pool')
        x = row.get('amount_in')
        if name not in by_name or name in used:
            raise ValueError('route contains an unknown or duplicate pool')
        if not isinstance(x, (int, float)) or not math.isfinite(x) or x <= 0:
            raise ValueError('route amount must be finite and positive')
        pool = by_name[name]
        if x > pool.cap(amount) + amount_tolerance(amount):
            raise ValueError('route cap violation')
        used.add(name)
        total += x
        score += pool.weighted_quote(x, aversion) - pool.incremental_gas_in_out
    if abs(total - amount) > amount_tolerance(amount):
        raise ValueError('route allocation must conserve total input')
    return score


def checked_upper_bound(upper: float, lower: float) -> float:
    if not math.isfinite(upper) or not math.isfinite(lower):
        raise ArithmeticError('non-finite optimization bound')
    tolerance = 1e-10 * max(abs(upper), abs(lower), 1e-12)
    if upper + tolerance < lower:
        raise ArithmeticError('lower bound exceeds upper bound; certificate invalid')
    return max(upper, lower)


def optimally_split_over_subset(subset: tuple[Pool, ...], amount_in: float,
                                aversion: float) -> list[float] | None:
    """Water-fill a fixed set of pools using marginal quote equalization.

    F_i(x) = w_i * Y_i * a_i * x/(X_i+a_i*x)
    F'_i(x)= w_i * Y_i * a_i * X_i/(X_i+a_i*x)^2
    Inverse marginal F'_i(x)=lambda gives
      x_i(lambda)=clip((sqrt(w_i*Y_i*a_i*X_i/lambda)-X_i)/a_i, 0, cap_i).
    """
    caps = [p.cap(amount_in) for p in subset]
    if sum(caps) + amount_tolerance(amount_in) < amount_in:
        return None
    if sum(caps) == amount_in:
        return caps

    lo = 0.0
    hi = max(p.weighted_marginal(0, aversion) for p in subset)

    def amounts_for_lambda(lamb: float) -> list[float]:
        if lamb <= 0:
            return caps[:]
        xs = []
        for p, cap in zip(subset, caps):
            a = p.fee_multiplier
            w = p.risk_weight(aversion)
            t = (w * p.reserve_out * a * p.reserve_in / lamb)
            raw_x = (math.sqrt(t) - p.reserve_in) / a if t > 0 else 0.0
            xs.append(min(cap, max(0.0, raw_x)))
        return xs

    for _ in range(90):
        mid = (lo + hi) * 0.5
        if sum(amounts_for_lambda(mid)) > amount_in:
            lo = mid
        else:
            hi = mid
    xs = amounts_for_lambda((lo + hi) / 2)

    # Remove tiny floating-point discrepancies. Exact integer math is required
    # for production transactions; this tolerance is for the simulation only.
    residual = amount_in - sum(xs)
    if abs(residual) > 1e-9 * amount_in:
        raise ArithmeticError('Water filling did not converge')
    if residual > 0:
        for idx in sorted(range(len(subset)),
                          key=lambda i: -subset[i].weighted_marginal(xs[i], aversion)):
            added = min(residual, caps[idx] - xs[idx])
            xs[idx] += added
            residual -= added
            if residual <= amount_tolerance(amount_in):
                break
    elif residual < 0:
        for idx in sorted(range(len(subset)),
                          key=lambda i: subset[i].weighted_marginal(xs[i], aversion)):
            removed = min(-residual, xs[idx])
            xs[idx] -= removed
            residual += removed
            if residual >= -amount_tolerance(amount_in):
                break
    return xs


def optimize(pools: list[Pool], amount_in: float, max_splits: int,
             aversion: float, base_gas_in_out: float = 0.0,
             candidate_limit: int = 0) -> dict[str, Any]:
    validate_problem(pools, amount_in, max_splits, aversion, base_gas_in_out)

    all_pool_count = len(pools)
    if candidate_limit > 0 and candidate_limit < len(pools):
        # Deliberate approximate candidate reduction (not globally exact).
        # Both small-trade attractiveness and available depth matter.
        # Favors reserves (depth) and competitive exchange rates.
        def rank(p: Pool) -> float:
            half = min(amount_in / 2, p.cap(amount_in))
            if half <= 0:
                return float('-inf')
            return p.weighted_quote(half, aversion) / half
        pools = sorted(pools, key=rank, reverse=True)[:candidate_limit]
    best = None
    feasible_subsets = 0
    for size in range(1, min(max_splits, len(pools)) + 1):
        for subset in itertools.combinations(pools, size):
            xs = optimally_split_over_subset(subset, amount_in, aversion)
            if xs is None:
                continue
            feasible_subsets += 1
            allocation = [(p, x) for p, x in zip(subset, xs) if x > 0]
            raw_output = sum(p.raw_quote(x) for p, x in allocation)
            robust_utility_output = sum(p.weighted_quote(x, aversion)
                                        for p, x in allocation)
            gas_out = base_gas_in_out + sum(p.incremental_gas_in_out
                                             for p, _ in allocation)
            score = robust_utility_output - gas_out
            candidate = {
                'total_input': amount_in,
                'expected_output': raw_output,
                'risk_adjusted_output': robust_utility_output,
                'gas_cost_in_output_token': gas_out,
                'optimization_score': score,
                'allocation': [
                    {'pool': p.name, 'amount_in': x,
                     'expected_out': p.raw_quote(x),
                     'risk_adjusted_out': p.weighted_quote(x, aversion),
                     'fee_bps': p.fee_bps,
                     'risk_bps_assumption': p.risk_bps,
                     'incremental_gas_in_output_token': p.incremental_gas_in_out}
                    for p, x in allocation],
            }
            if best is None or score > best['optimization_score']:
                best = candidate
    if best is None:
        raise ValueError('No feasible set of pools can fill the requested input')
    best['candidate_pools_scanned'] = len(pools)
    best['total_pools_available'] = all_pool_count
    best['feasible_subsets_examined'] = feasible_subsets
    best['global_optimality_for_frozen_v2_model'] = (len(pools) == all_pool_count)
    return best


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--config', required=True, help='JSON with pools and input amount')
    ap.add_argument('--amount', type=float, default=None, help='Override amount_in')
    ap.add_argument('--max-splits', type=int, default=None, help='Override max_splits')
    ap.add_argument('--risk-aversion', type=float, default=None,
                    help='0 = ignore risk haircuts, 1 = apply full supplied risk_bps')
    ap.add_argument('--candidate-limit', type=int, default=0,
                    help='Optional heuristic top N; 0 = exhaustive')
    ap.add_argument('--json-out', type=str, default=None)
    args = ap.parse_args()
    config = json.loads(Path(args.config).read_text(encoding='utf8'))
    pools = [Pool(
        name=str(p['name']),
        reserve_in=float(p['reserve_in']),
        reserve_out=float(p['reserve_out']),
        fee_bps=float(p['fee_bps']),
        incremental_gas_in_out=float(p['incremental_gas_in_out']),
        risk_bps=float(p.get('risk_bps', 0)),
        cap_in=float(p['cap_in']) if p.get('cap_in') is not None else None,
    ) for p in config['pools']]
    result = optimize(
        pools=pools,
        amount_in=float(config['amount_in'] if args.amount is None else args.amount),
        max_splits=int(config.get('max_splits', 3) if args.max_splits is None else args.max_splits),
        aversion=float(config.get('risk_aversion', 1.0)
                       if args.risk_aversion is None else args.risk_aversion),
        base_gas_in_out=float(config.get('base_gas_in_output_token', 0)),
        candidate_limit=args.candidate_limit,
    )
    report = json.dumps(result, ensure_ascii=False, indent=2)
    print(report)
    if args.json_out:
        Path(args.json_out).write_text(report + '\n', encoding='utf8')


if __name__ == '__main__':
    main()
