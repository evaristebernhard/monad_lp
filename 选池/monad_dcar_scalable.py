#!/usr/bin/env python3
"""DCAR: exploratory dual-bounded routing prototype for 1k-2k independent V2 pools.

Implements: O(n) dual evaluation, dual-guided candidate set, exact subset
waterfilling for the small shortlist, valid upper/lower optimization bounds,
and revalidation after a few pool updates. No RPC, no wallet or transactions.

NOVELTY NOTE: dual decomposition and fixed-cost routing are prior art; the
combination below is not established as novel. Certificates apply *only* to
frozen independent V2 pools with known fixed incremental gas and concave
risk-weighted quotes; NOT arbitrary Monad DEX routes or fee-on-transfer tokens.
"""
from __future__ import annotations
import argparse
import heapq
import json
import math
import random
import time
from pathlib import Path
from typing import Any
from monad_pool_selector import (Pool, optimize, optimally_split_over_subset,
    validate_problem, amount_tolerance, score_previous_route, checked_upper_bound)


def argmax_net_at_dual(pool: Pool, mu: float, amount_in: float, aversion: float) -> tuple[float, float]:
    """Return (h_i(mu)-g_i, x_i*) with h=max_{0<=x<=cap} w*q(x)-mu*x."""
    a = pool.fee_multiplier
    w = pool.risk_weight(aversion)
    cap = pool.cap(amount_in)
    if pool.weighted_marginal(0, aversion) <= mu:
        x = 0.0
    elif pool.weighted_marginal(cap, aversion) >= mu:
        x = cap
    else:
        x = (math.sqrt(w * pool.reserve_out * a * pool.reserve_in / mu)-pool.reserve_in) / a
        x = max(0.0, min(cap, x))
    gain = pool.weighted_quote(x, aversion)-mu*x
    return gain-pool.incremental_gas_in_out, x


def dual_bound(pools: list[Pool], amount: float, max_splits: int,
               mu: float, aversion: float, base_gas: float = 0.0
               ) -> tuple[float, list[tuple[float,int,float]]]:
    ranked = []
    for i,p in enumerate(pools):
        val,x = argmax_net_at_dual(p,mu,amount,aversion)
        ranked.append((val,i,x))
    top = heapq.nlargest(max_splits, ranked)
    ub = mu*amount - base_gas + sum(max(0.0,t[0]) for t in top)
    return ub,top


def dual_minimize(pools: list[Pool], amount: float, k: int,
                  aversion: float, gas: float, iterations: int = 36
                  ) -> tuple[float,float,int]:
    # Log-space golden section of the convex-in-mu Lagrangian dual objective.
    mmax = max(p.weighted_marginal(0, aversion) for p in pools)
    # mmax guarantees x(mu)=0 for all pools; low must be > 0 for inverse marginal.
    left, right = math.log(max(mmax*1e-9,1e-18)), math.log(mmax*1.01)
    gr = (math.sqrt(5)-1)/2
    c = right-gr*(right-left)
    d = left+gr*(right-left)
    uc = dual_bound(pools,amount,k,math.exp(c),aversion,gas)[0]
    ud = dual_bound(pools,amount,k,math.exp(d),aversion,gas)[0]
    evals=2
    for _ in range(iterations):
        if uc <= ud:
            right,d,ud=d,c,uc
            c=right-gr*(right-left)
            uc=dual_bound(pools,amount,k,math.exp(c),aversion,gas)[0]
        else:
            left,c,uc=c,d,ud
            d=left+gr*(right-left)
            ud=dual_bound(pools,amount,k,math.exp(d),aversion,gas)[0]
        evals+=1
    if uc<ud: return uc, math.exp(c),evals
    return ud, math.exp(d),evals


def initial_candidates(pools:list[Pool], amount:float, k:int, aversion:float,
                       dual_mu:float, cap:int) -> list[int]:
    if cap<=0 or len(pools)<=cap: return list(range(len(pools)))
    cap = min(len(pools), max(cap, k))
    rankings=[]
    # Multi-scale dual contribution finds alternative gas-efficient and deep pools.
    for factor in (0.6,0.85,1.0,1.15,1.5,2.0):
        ranked=heapq.nlargest(cap, (
            (argmax_net_at_dual(p,max(dual_mu*factor,1e-15),amount,aversion)[0],i)
            for i,p in enumerate(pools)))
        rankings.append([i for val,i in ranked])
    # Also retain quote leaders for the actual trade and smaller portions.
    for fraction in (1.0,0.5,1.0/max(k,1)):
        ranked=heapq.nlargest(cap,(
            (p.weighted_quote(amount*fraction,aversion)-p.incremental_gas_in_out,i)
            for i,p in enumerate(pools)))
        rankings.append([i for val,i in ranked])
    # Always include a capacity-covering combination when one exists.
    # Otherwise top prices with tiny capacities could make the shortlist
    # infeasible even though a feasible full-universe solution exists.
    result=[i for _,i in heapq.nlargest(min(k,cap),
                         ((p.cap(amount),i) for i,p in enumerate(pools)))]
    if len(result) >= cap:
        return result
    # Round-robin priority avoids letting any one criterion monopolize shortlist.
    for rank_idx in range(cap):
        for group in rankings:
            i=group[rank_idx]
            if i not in result:
                result.append(i)
                if len(result)>=cap: return result
    return result


def shortlist_optimize(pools:list[Pool], amount:float, k:int, aversion:float,
                       base_gas:float, shortlist:int=20) -> dict[str,Any]:
    t0=time.perf_counter()
    validate_problem(pools, amount, k, aversion, base_gas)
    if not isinstance(shortlist, int) or shortlist < 0:
        raise ValueError('shortlist must be a nonnegative integer')
    if sum(heapq.nlargest(min(k,len(pools)),(p.cap(amount) for p in pools))) + amount_tolerance(amount) < amount:
        raise ValueError('Insufficient combined pool capacity for requested input and K')
    ub,mu,neval=dual_minimize(pools,amount,k,aversion,base_gas)
    t1=time.perf_counter()
    indices=initial_candidates(pools,amount,k,aversion,mu,shortlist)
    # Exhaustive within shortlist only; never claim globally optimal by default.
    best=optimize([pools[i] for i in indices],amount,k,aversion,base_gas)
    t2=time.perf_counter()
    # Compare with globally best full-sized single pool (feasible LB) so
    # shortlist pruning cannot make us worse than best isolated pool.
    best_single=None
    for p in pools:
        if p.cap(amount)+amount_tolerance(amount) < amount: continue
        utility=p.weighted_quote(amount,aversion)-p.incremental_gas_in_out-base_gas
        if best_single is None or utility>best_single[0]:best_single=(utility,p)
    if best_single is not None and best_single[0]>best['optimization_score']:
        p=best_single[1]
        best.update(optimization_score=best_single[0],
                    expected_output=p.raw_quote(amount),
                    risk_adjusted_output=p.weighted_quote(amount,aversion),
                    gas_cost_in_output_token=base_gas+p.incremental_gas_in_out,
                    allocation=[{'pool':p.name,'amount_in':amount,'expected_out':p.raw_quote(amount)}])
    # The golden-section numeric approximate minimizer may produce a valid
    # but loose upper bound; rounding tolerance prevents false negative gaps.
    lb=best['optimization_score']
    ub=checked_upper_bound(ub+1e-10*max(abs(ub),1e-12),lb)
    best['dual_upper_bound']=ub
    best['feasible_lower_bound']=lb
    best['certified_relative_gap_upper_bound']=max(0,ub-lb)/max(1e-12,abs(lb))
    best['certificate_scope']='frozen, independent V2 pools; <=K, exact-in; fixed known gas'
    best['certificate_arithmetic']='floating-point numerical bound, not an interval or on-chain proof'
    best['global_optimality_for_frozen_v2_model']=bool(ub-lb <= 1e-8*max(1,abs(lb)))
    best['dual_mu']=mu
    best['dual_evaluations']=neval
    best['pruned_to']=len(indices)
    best['total_pools_available']=len(pools)
    best['timing_ms']={'dual':(t1-t0)*1000,'shortlist':(t2-t1)*1000,
                       'total':(t2-t0)*1000}
    return best


def certificate_after_updates(pools: list[Pool], previous:dict[str,Any],
                              amount:float,k:int,aversion:float,base_gas:float,
                              epsilon:float=0.0005)->dict[str,Any]:
    """Reprice previous allocation, recompute valid global UB at prev mu.

    Returns certified_can_reuse if new UB/new feasible score gap <= tolerance;
    pool deltas are already applied to pools. O(nk) via bound evaluation,
    O(k) for held-route revaluation, no enumeration and no RPC network costs.
    """
    t0=time.perf_counter()
    if not math.isfinite(epsilon) or epsilon < 0:
        raise ValueError('epsilon must be finite and nonnegative')
    mu = previous['dual_mu']
    if not math.isfinite(mu) or mu <= 0:
        raise ValueError('dual_mu must be finite and positive')
    feasible_score = score_previous_route(pools, previous['allocation'], amount, k, aversion, base_gas)
    ub,_=dual_bound(pools,amount,k,mu,aversion,base_gas)
    ub=checked_upper_bound(ub,feasible_score)
    gap=max(0,ub-feasible_score)/max(1e-12,abs(feasible_score))
    return {'certified_can_reuse':gap<=epsilon,
            'upper_bound':ub,'lower_bound':feasible_score,
            'gap_upper_bound':gap,
            'time_ms':(time.perf_counter()-t0)*1000}



class IncrementalDualCertificate:
    """Reusable fixed-dual-price upper bound with lazy heap updates.

    After initialization O(N), changed-pool updates are O(d log N) amortized;
    certificate query is O(K log N) amortized, excluding RPC and re-pricing
    held pools. Valid for any fixed positive mu, even if mu is no longer dual
    optimal after state changes; the bound may simply become looser.
    """
    def __init__(self, pools:list[Pool], amount:float, k:int,mu:float,
                 aversion:float=0.0,base_gas:float=0.0):
        validate_problem(pools, amount, k, aversion, base_gas)
        if not math.isfinite(mu) or mu <= 0:
            raise ValueError('dual_mu must be finite and positive')
        self.pools=list(pools)
        self.amount=amount
        self.k=k
        self.mu=mu
        self.aversion=aversion
        self.base_gas=base_gas
        self.ids={p.name:i for i,p in enumerate(pools)}
        self.version=[0]*len(pools)
        self.heap=[]
        for i,p in enumerate(pools):
            score,_=argmax_net_at_dual(p,mu,amount,aversion)
            self.heap.append((-score,0,i))
        heapq.heapify(self.heap)
        self.update_count=0

    def update(self,new_pools:list[Pool])->None:
        if len({p.name for p in new_pools}) != len(new_pools):
            raise ValueError('duplicate pool update')
        for p in new_pools:
            p.validate()
            if p.name not in self.ids or p.risk_weight(self.aversion) <= 0:
                raise ValueError('unknown pool or nonpositive risk weight')
        for p in new_pools:
            i=self.ids[p.name]
            self.pools[i]=p
            self.version[i]+=1
            score,_=argmax_net_at_dual(p,self.mu,self.amount,self.aversion)
            heapq.heappush(self.heap,(-score,self.version[i],i))
            self.update_count+=1
        if len(self.heap)>4*len(self.pools):
            # Periodic rebuild prevents unbounded stale entries.
            self.heap=[]
            for i,p in enumerate(self.pools):
                score,_=argmax_net_at_dual(p,self.mu,self.amount,self.aversion)
                self.heap.append((-score,self.version[i],i))
            heapq.heapify(self.heap)

    def upper_bound(self)->float:
        kept=[]
        gain=0.0
        for _ in range(min(self.k,len(self.pools))):
            while self.heap and self.heap[0][1]!=self.version[self.heap[0][2]]:
                heapq.heappop(self.heap)
            if not self.heap:break
            negscore,ver,i=heapq.heappop(self.heap)
            kept.append((negscore,ver,i))
            gain+=max(0.0,-negscore)
        for item in kept:heapq.heappush(self.heap,item)
        return self.mu*self.amount+gain-self.base_gas

    def recheck(self,previous:dict[str,Any],relative_epsilon:float=0.001
                )->dict[str,Any]:
        t0=time.perf_counter()
        if not math.isfinite(relative_epsilon) or relative_epsilon < 0:
            raise ValueError('epsilon must be finite and nonnegative')
        try:
            referenced = {row['pool']: self.pools[self.ids[row['pool']]]
                          for row in previous['allocation'] if row.get('pool') in self.ids}
            lb = score_previous_route(self.pools, previous['allocation'], self.amount,
                                      self.k, self.aversion, self.base_gas, by_name=referenced)
        except ValueError as exc:
            return {'certified_can_reuse':False, 'reason':str(exc)}
        ub=checked_upper_bound(self.upper_bound(),lb)
        gap=max(0.0,ub-lb)/max(abs(lb),1e-12)
        return {'certified_can_reuse':gap<=relative_epsilon,
                'lower_bound':lb,'upper_bound':ub,
                'gap_upper_bound':gap,
                'time_ms':(time.perf_counter()-t0)*1000}

def synthetic(n:int,seed:int=7)->list[Pool]:
    rng=random.Random(seed)
    pools=[]
    for i in range(n):
        x=math.exp(rng.uniform(math.log(300), math.log(300000)))
        y=x*rng.uniform(0.99,1.015)
        gas=rng.uniform(0.2,1.0)
        pools.append(Pool(name=f'synthetic-{i}',reserve_in=x,reserve_out=y,
            fee_bps=rng.choice([1,5,10,30,100]),incremental_gas_in_out=gas))
    return pools


def benchmark()->None:
    for n in (8,12,1000,2000):
        pools=synthetic(n,seed=42+n)
        q=10000;k=3
        res=shortlist_optimize(pools,q,k,0.0,0.0,shortlist=min(n,20))
        row={'pool_count':n,'shortlist':res['pruned_to'],
            'score':round(res['optimization_score'],4),
            'dual_upper_bound':round(res['dual_upper_bound'],4),
            'gap_upper_bound_pct':round(100*res['certified_relative_gap_upper_bound'],5),
            'runtime_ms':round(res['timing_ms']['total'],2),
            'searched_subsets':res['feasible_subsets_examined']}
        if n<=12:
            exact=optimize(pools,q,k,0,0)
            row['exact_score']=round(exact['optimization_score'],4)
            row['heuristic_minus_exact']=round(res['optimization_score']-exact['optimization_score'],9)
            assert res['optimization_score'] <= exact['optimization_score']+1e-5
            assert exact['optimization_score'] <= res['dual_upper_bound']+1e-5
        if n>=1000:
            idx=IncrementalDualCertificate(pools,q,k,res['dual_mu'])
            rng=random.Random(n)
            affected=rng.sample(range(n),5)
            for i in affected:
                p=pools[i]
                # Update 5 pools with small reserve movement, carry certificate.
                new_x=p.reserve_in*rng.uniform(0.998,1.002)
                new_y=p.reserve_out*rng.uniform(0.998,1.002)
                pools[i]=Pool(p.name,new_x,new_y,p.fee_bps,
                              p.incremental_gas_in_out)
            idx.update([pools[i] for i in affected])
            reuse=idx.recheck(res,relative_epsilon=0.001)
            # Verify incremental heap equals global fresh dual calculation.
            fresh_ub,_=dual_bound(pools,q,k,res['dual_mu'],0,0)
            assert abs(fresh_ub-reuse['upper_bound'])<=1e-6
            row['update_reuse_gap_pct']=round(100*reuse['gap_upper_bound'],5)
            row['update_check_ms']=round(reuse['time_ms'],4)
            row['update_certified_0p1pct']=reuse['certified_can_reuse']
        print(json.dumps(row,ensure_ascii=False))


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--benchmark',action='store_true')
    p.add_argument('--config')
    p.add_argument('--shortlist',type=int,default=20)
    p.add_argument('--json-out')
    args=p.parse_args()
    if args.benchmark or not args.config:
        benchmark()
    else:
        c=json.loads(Path(args.config).read_text(encoding='utf-8'))
        pools=[Pool(name=p['name'],reserve_in=float(p['reserve_in']),
                    reserve_out=float(p['reserve_out']),fee_bps=float(p['fee_bps']),
                    incremental_gas_in_out=float(p['incremental_gas_in_out']),
                    risk_bps=float(p.get('risk_bps',0)),
                    cap_in=float(p['cap_in']) if p.get('cap_in') is not None else None)
               for p in c['pools']]
        res=shortlist_optimize(pools,float(c['amount_in']),
            int(c.get('max_splits',3)),float(c.get('risk_aversion',0)),
            float(c.get('base_gas_in_output_token',0)),args.shortlist)
        output=json.dumps(res,ensure_ascii=False,indent=2)
        print(output)
        if args.json_out:Path(args.json_out).write_text(output+'\n',encoding='utf-8')
if __name__=='__main__':main()
