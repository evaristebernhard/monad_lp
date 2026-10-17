#!/usr/bin/env python3
"""Reproducible restricted-model tests for DCAR. No RPC or transactions."""
import json
import random
import statistics
import time
from monad_pool_selector import Pool, optimize
from monad_dcar_scalable import (IncrementalDualCertificate, dual_bound,
                                  shortlist_optimize, synthetic)

def run():
    rng=random.Random(93)
    cap_count=0
    worst_cap_gap=0
    max_score_error=0
    for _ in range(120):
        n=rng.randrange(4,11)
        pools=[]
        for i in range(n):
            x=rng.uniform(400,50000)
            pools.append(Pool(f'c{i}',x,x*rng.uniform(.95,1.05),
                rng.choice([5,30,100]),rng.uniform(.01,4),
                cap_in=rng.uniform(1000,10000)))
        amount=9000;k=3
        if sum(sorted([p.cap(amount) for p in pools],reverse=True)[:k])<amount:
            continue
        result=shortlist_optimize(pools,amount,k,0,0,min(n,5))
        exact=optimize(pools,amount,k,0,0)
        error=exact['optimization_score']-result['optimization_score']
        assert error>=-1e-6
        assert exact['optimization_score']<=result['dual_upper_bound']+1e-5
        cap_count+=1
        max_score_error=max(max_score_error,error)
        worst_cap_gap=max(worst_cap_gap,result['certified_relative_gap_upper_bound'])
    perf={}
    for n in (1000,2000):
        timings=[];gaps=[]
        for i in range(12):
            pools=synthetic(n,100+i)
            q=[1000,10000,50000][i%3]
            result=shortlist_optimize(pools,q,3,0,0,20)
            timings.append(result['timing_ms']['total'])
            gaps.append(result['certified_relative_gap_upper_bound'])
        perf[str(n)]={'samples':12,
                      'median_offline_ms':round(statistics.median(timings),3),
                      'max_offline_ms':round(max(timings),3),
                      'max_certified_relative_gap_percent':round(100*max(gaps),8)}
    pools=synthetic(2000,2026)
    initial=shortlist_optimize(pools,10000,3,0,0,20)
    idx=IncrementalDualCertificate(pools,10000,3,initial['dual_mu'])
    used={row['pool'] for row in initial['allocation']}
    idx.update([Pool(p.name,p.reserve_in,p.reserve_out*.98,p.fee_bps,
                 p.incremental_gas_in_out) for p in pools if p.name in used])
    stress=idx.recheck(initial,.001)
    assert not stress['certified_can_reuse']
    assert abs(stress['upper_bound']-dual_bound(idx.pools,10000,3,initial['dual_mu'],0,0)[0])<1e-6
    return {'benchmark_model':'independent synthetic V2-like pools; max 3 splits',
            'capped_pool_tests':{'cases':cap_count,
                'max_exhaustive_score_minus_heuristic':round(max_score_error,8),
                'max_bound_gap_percent':round(worst_cap_gap*100,5)},
            'large_pool_tests':perf,
            'adverse_update_test':{
                'downward_change_of_selected_pools_y_reserve':'-2% (synthetic stress)',
                'certified_reuse':stress['certified_can_reuse'],
                'old_route_relative_gap_upper_bound_percent':round(100*stress['gap_upper_bound'],4)},
            'limitations':'CPU-only, no Monad RPC/swap simulation, no V3/V4/order books/multi-hop'}

if __name__=='__main__':
    print(json.dumps(run(),ensure_ascii=False,indent=2))
