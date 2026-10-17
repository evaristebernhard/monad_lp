"""Regressions for numerical and feasibility findings in the DCAR audit."""
import json
import math
import random
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from monad_pool_selector import Pool, optimize, checked_upper_bound, score_previous_route
from monad_dcar_scalable import shortlist_optimize, IncrementalDualCertificate, dual_bound


class AuditRegressions(unittest.TestCase):
    def test_rejects_nonfinite_problem_fields(self):
        for value in [math.nan, math.inf, -math.inf]:
            for field in ['reserve_in', 'reserve_out', 'incremental_gas_in_out']:
                data = dict(name='p', reserve_in=1000, reserve_out=1000, fee_bps=30, incremental_gas_in_out=0)
                data[field] = value
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    shortlist_optimize([Pool(**data)], 100, 1, 0, 0)
            with self.assertRaises(ValueError):
                shortlist_optimize([Pool('p',1000,1000,30,0)], value, 1, 0, 0)

    def test_capacity_shortlist_expands_to_feasible_k(self):
        pools = [Pool(str(i), 1000, 1000, 30, 0, cap_in=40) for i in range(3)]
        result = shortlist_optimize(pools, 100, 3, 0, 0, 1)
        self.assertAlmostEqual(sum(x['amount_in'] for x in result['allocation']), 100)
        self.assertEqual(result['pruned_to'], 3)

    def test_zero_cap_is_not_unlimited_in_cli(self):
        config = dict(amount_in=10, max_splits=1, pools=[dict(name='p',reserve_in=1000,reserve_out=1000,fee_bps=30,incremental_gas_in_out=0,cap_in=0)])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'zero.json'
            path.write_text(json.dumps(config))
            run = subprocess.run([sys.executable, str(Path(__file__).with_name('monad_dcar_scalable.py')), '--config', str(path)], capture_output=True, text=True)
            self.assertNotEqual(run.returncode, 0)
            self.assertIn('cap_in must be positive', run.stderr)

    def test_duplicate_route_cannot_receive_reuse_certificate(self):
        pool = Pool('p',1000,1000,0,0)
        certificate = IncrementalDualCertificate([pool],100,3,pool.weighted_marginal(100,0))
        route = {'allocation':[{'pool':'p','amount_in':100/3}]*3}
        result = certificate.recheck(route)
        self.assertFalse(result['certified_can_reuse'])
        self.assertIn('duplicate',result['reason'])

    def test_route_conserves_input_and_limits_number_of_pools(self):
        pools = [Pool(str(i),1000,1000,30,0) for i in range(3)]
        for allocation in [[{'pool':'0','amount_in':50}], [{'pool':str(i),'amount_in':100/3} for i in range(3)], [{'pool':'x','amount_in':100}], [{'pool':'0','amount_in':-100}]]:
            with self.assertRaises(ValueError):
                score_previous_route(pools,allocation,100,2,0,0)

    def test_bound_violation_is_not_silently_clamped(self):
        with self.assertRaises(ArithmeticError):
            checked_upper_bound(90.9,96.7)

    def test_invalid_updates_do_not_mutate_index(self):
        pools = [Pool('p',1000,1000,30,0)]
        index = IncrementalDualCertificate(pools,100,1,0.8)
        before = index.upper_bound()
        with self.assertRaises(ValueError):
            index.update([Pool('p',2000,2000,30,0),Pool('missing',1000,1000,30,0)])
        self.assertEqual(index.upper_bound(),before)

    def test_tiny_input_is_not_dropped(self):
        result = optimize([Pool('p',1000,1000,30,0)],1e-9,1,0,0)
        self.assertGreater(result['expected_output'],0)
        self.assertAlmostEqual(sum(x['amount_in'] for x in result['allocation']) / 1e-9,1)

    def test_incremental_matches_full_bound_and_exhaustive(self):
        rng = random.Random(143)
        for case in range(20):
            pools=[Pool(str(i),rng.uniform(500,3000),rng.uniform(500,3000),rng.choice([1,5,30]),rng.uniform(0,0.1),cap_in=rng.uniform(40,200)) for i in range(6)]
            result=shortlist_optimize(pools,100,3,0,0,4)
            exact=optimize(pools,100,3,0,0)
            self.assertLessEqual(result['optimization_score'],exact['optimization_score']+1e-7)
            self.assertGreaterEqual(result['dual_upper_bound']+1e-7,exact['optimization_score'])
            index=IncrementalDualCertificate(pools,100,3,result['dual_mu'])
            changed=Pool('0',pools[0].reserve_in,pools[0].reserve_out*0.99,pools[0].fee_bps,pools[0].incremental_gas_in_out,cap_in=pools[0].cap_in)
            index.update([changed]); pools[0]=changed
            self.assertAlmostEqual(index.upper_bound(),dual_bound(pools,100,3,result['dual_mu'],0,0)[0],places=7)


if __name__=='__main__':
    unittest.main()
