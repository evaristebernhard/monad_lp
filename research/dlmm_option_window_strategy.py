#!/usr/bin/env python3
"""Read-only DLMM pool-proxy vs Deribit whole-contract static option hedge.

Only a SYNTHETIC pro-rata pool inventory is modeled; no user PositionV2.
No actual fill, fee-income, pathwise bin replay or backtested P&L claim.
"""
import argparse,datetime as dt,json,math,sqlite3,sys
from collections import defaultdict
from pathlib import Path
import numpy as np
from scipy.optimize import Bounds,LinearConstraint,milp

SOL="So11111111111111111111111111111111111111112"
DEFAULT_POOL="5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6"

def iso(s): return dt.datetime.fromisoformat(str(s).replace("Z","+00:00"))

def pool_from_sql(db,pool,opt_time):
    row=db.execute("""SELECT ps.* FROM pool_snapshots ps WHERE pool_address=? AND status='ok'
                       ORDER BY abs(julianday(observed_at_utc)-julianday(?)) LIMIT 1""",
                   (pool,opt_time)).fetchone()
    if row is None:raise ValueError("no complete pool-level bin snapshot")
    if SOL not in (row["token_x_mint"],row["token_y_mint"]):raise ValueError("not a SOL bin pair")
    flip=row["token_y_mint"]==SOL
    dx=int(row["token_x_decimals"]);dy=int(row["token_y_decimals"])
    raw=db.execute("""SELECT bin_id,price_y_per_x,amount_x_raw,amount_y_raw,liquidity_supply
                      FROM bin_snapshots WHERE snapshot_id=?""",(row["snapshot_id"],)).fetchall()
    active_stored=next((float(z["price_y_per_x"]) for z in raw if int(z["bin_id"])==int(row["active_id"])),None)
    if active_stored is None:raise ValueError("active bin absent")
    expected_human=float(row["price_y_per_x"])
    scaling=10**(dx-dy)
    # Existing 2026-10-09 DB has incorrectly double-scaled Bin prices;
    # use the independently stored pool price as a consistency anchor.
    if abs(active_stored/expected_human-1)<.03:fix=1.
    elif abs(active_stored/scaling/expected_human-1)<.03:fix=scaling
    else:raise ValueError("saved Bin price not consistent with pool account or known decimal-scaling bug")
    bins=[]
    for b in raw:
        k=float(b["price_y_per_x"])/fix
        if not (math.isfinite(k) and k>0):continue
        k=1/k if flip else k
        x=(float(b["amount_y_raw"])/10**dy if flip else float(b["amount_x_raw"])/10**dx)
        y=(float(b["amount_x_raw"])/10**dx if flip else float(b["amount_y_raw"])/10**dy)
        bins.append({"id":int(b["bin_id"]),"strike":k,"sol":x,"stable":y,
                     "liquidity_supply":b["liquidity_supply"]})
    active=int(row["active_id"])
    active_bin=next((z for z in bins if z["id"]==active),None)
    if not active_bin:raise ValueError("active Bin absent: cannot anchor an internally consistent synthetic payoff")
    spot=active_bin["strike"]
    total=sum(z["sol"]*spot+z["stable"] for z in bins)
    if total<=0:raise ValueError("empty pool reserves")
    offside=0.
    for b in bins:
        if b["id"]<active:offside+=b["sol"]*spot
        elif b["id"]>active:offside+=b["stable"]
    weights=[]
    for b in bins:
        k=b["strike"]
        if b["id"]<=active and b["stable"]>0:weights.append(("put",k,b["stable"]/k, b["id"]))
        if b["id"]>=active and b["sol"]>0:weights.append(("call",k,b["sol"], b["id"]))
    age=abs((iso(opt_time)-iso(row["observed_at_utc"])).total_seconds())
    return dict(price_decimal_correction_factor=fix,snapshot_id=row["snapshot_id"],observed_at_utc=row["observed_at_utc"],
                active_id=active,bin_step_bps=row["bin_step_bps"],price=spot,
                aggregate_reserves_mark_usdc=total,offside_reserves_usdc=offside,
                offside_fraction=offside/total,bin_count=len(bins),weights=weights,
                pool_option_time_delta_seconds=age)

def extract_lots(db,opt_id,spot,lo,hi,cap=3):
    options=db.execute("""SELECT * FROM deribit_option_quotes WHERE snapshot_id=? AND book_status='ok'
        AND two_sided=1 AND strike BETWEEN ? AND ? ORDER BY option_type,strike""",(opt_id,lo,hi)).fetchall()
    out=[]
    for o in options:
        side=o["option_type"];k=float(o["strike"])
        if side not in ("put","call") or (side=="put" and k>spot) or (side=="call" and k<spot):continue
        size=float(o["contract_size_sol"] or 0)
        min_trade=float(o["min_trade_amount"] or size)
        if size<=0 or min_trade>size+1e-5:continue
        asks=db.execute("""SELECT price_usdc_per_sol,amount_sol
                           FROM deribit_ask_levels WHERE snapshot_id=? AND instrument_name=?
                           ORDER BY price_usdc_per_sol""",(opt_id,o["instrument_name"])).fetchall()
        levels=[[float(z[0]),float(z[1])] for z in asks if z[0]>0 and z[1]>0]
        for lot in range(cap):
            left=size;premium=0.
            for lev in levels:
                q=min(left,lev[1]);premium+=q*lev[0];lev[1]-=q;left-=q
                if left<1e-7:break
            if left>1e-7:break
            fee=min(0.0003*spot*size,.125*premium) # indicative public standard tier
            out.append(dict(instrument=o["instrument_name"],side=side,strike=k,lot=lot,
                            size=size,premium=premium,fee=fee,cost=premium+fee))
    return out

def pay(s,side,k):return max(0,k-s) if side=="put" else max(0,s-k)

def loss_at(s,weights):
    return sum(q*pay(s,side,k) for side,k,q,_ in weights)

def evaluate(weights,legs,lo,hi,spot):
    points=sorted(set([lo,hi,spot]+[k for _,k,_,_ in weights if lo<=k<=hi]+
                      [z["strike"] for z in legs if lo<=z["strike"]<=hi]))
    worst=(-1,None,None)
    for s in points:
        d=loss_at(s,weights)
        h=sum(z["quantity_sol"]*pay(s,z["side"],z["strike"]) for z in legs)
        r=d-h
        if abs(r)>worst[0]:worst=(abs(r),s,r)
    return {"max_abs_error_usdc":worst[0],"at_price":worst[1],
            "signed_residual_usdc":worst[2],"checked_breakpoints":len(points)}

def solve(weights,lots,lo,hi,spot,lamb=1,seconds=35):
    baseline=evaluate(weights,[],lo,hi,spot)
    if not lots:return dict(status="no_eligible_depth",no_hedge=baseline,contract_count=0)
    N=len(lots)
    points=sorted(set([lo,hi,spot]+[k for _,k,_,_ in weights if lo<=k<=hi]+
                      [z["strike"] for z in lots]))
    # Exact finite linear constraints: DLMM loss and vanilla hedges are piecewise affine.
    paymat=np.array([[z["size"]*pay(s,z["side"],z["strike"]) for z in lots] for s in points])
    desired=np.array([loss_at(s,weights) for s in points])
    constraints=[]
    # H-D <= E; D-H <= E.
    constraints.append((np.c_[paymat,-np.ones(len(points))],desired))
    constraints.append((np.c_[-paymat,-np.ones(len(points))],-desired))
    A=np.vstack([c[0] for c in constraints]);ub=np.concatenate([c[1] for c in constraints])
    by_name=defaultdict(list)
    for i,z in enumerate(lots):by_name[z["instrument"]].append(i)
    for group in by_name.values():
        for i,j in zip(group,group[1:]):
            row=np.zeros(N+1);row[j]=1;row[i]=-1
            A=np.vstack([A,row]);ub=np.r_[ub,0.]
    objective=np.r_[[z["cost"] for z in lots],lamb]
    res=milp(objective,integrality=np.r_[np.ones(N),0],
             bounds=Bounds(np.zeros(N+1),np.r_[np.ones(N),np.inf]),
             constraints=LinearConstraint(A,np.full(len(ub),-np.inf),ub),
             options={"time_limit":seconds,"mip_rel_gap":.005})
    if res.x is None:return dict(status="no_solution_or_timeout",solver_message=str(res.message),
                                  no_hedge=baseline,eligible_lots=N)
    buys=np.rint(res.x[:N]).astype(int)
    legs=[]
    for group in by_name.values():
        n=sum(int(buys[j]) for j in group)
        if n:
            z=lots[group[0]]
            legs.append(dict(instrument=z["instrument"],side=z["side"],strike=z["strike"],
                             contracts=n,quantity_sol=n*z["size"]))
    real=evaluate(weights,legs,lo,hi,spot)
    cost=sum(z["cost"] for z,b in zip(lots,buys) if b)
    price=sum(z["premium"] for z,b in zip(lots,buys) if b)
    err=float(res.x[-1])
    certified=real["max_abs_error_usdc"]<=err+1e-4
    return dict(status="risk_certified" if certified else "certificate_failed",
                solver_status=int(res.status),solver_message=str(res.message),
                mip_gap=getattr(res,"mip_gap",None),
                contract_count=sum(x["contracts"] for x in legs),legs=legs,
                observed_premium_usdc=price,indicative_cost_incl_fees_usdc=cost,
                objective_usdc=cost+lamb*real["max_abs_error_usdc"],
                no_hedge=baseline,hedged=real,eligible_instruments=len(by_name),
                eligible_lots=N,lambda_usdc_per_usdc=lamb)

def historical_window(db,pool,option_observed,expiry_ms,price_anchor):
    rows=db.execute("""SELECT timestamp_utc,close_usd,volume_usd FROM pool_market_candles
                       WHERE pool_address=? ORDER BY timestamp_utc""",(pool,)).fetchall()
    trades=db.execute("SELECT COUNT(*) FROM deribit_historical_trades").fetchone()[0]
    if not rows:return {"hourly_candles":0,"option_trade_rows_all_instruments":trades}
    prices=[float(r["close_usd"]) for r in rows if r["close_usd"] and float(r["close_usd"])>0]
    start=int(rows[0]["timestamp_utc"]);end=int(rows[-1]["timestamp_utc"])
    days=(int(expiry_ms)/1000 - iso(option_observed).timestamp())/86400
    return dict(hourly_candles=len(rows),first_hour_utc=dt.datetime.fromtimestamp(start,dt.timezone.utc).isoformat(),
        last_hour_utc=dt.datetime.fromtimestamp(end,dt.timezone.utc).isoformat(),
        observed_duration_hours=(end-start)/3600+1,hourly_interval_gaps=sum(int(rows[i+1]["timestamp_utc"]-rows[i]["timestamp_utc"]!=3600) for i in range(len(rows)-1)),
        min_close_usdc=min(prices),max_close_usdc=max(prices),
        min_close_ratio=min(prices)/price_anchor,max_close_ratio=max(prices)/price_anchor,
        historical_trade_rows_all_instruments=trades,
        option_remaining_days=days,
        option_expires_after_history_window=(days>len(rows)/24),
        interpretation="72h pool price/volume is not historical Bin liquidity, realized LP fees, or historical option depth")

def analysis(path,pool,wealth,widths,lambdas):
    db=sqlite3.connect(f"file:{path.resolve()}?mode=ro",uri=True);db.row_factory=sqlite3.Row
    opt=db.execute("SELECT * FROM deribit_snapshots WHERE status='ok' ORDER BY snapshot_id DESC LIMIT 1").fetchone()
    if not opt:raise ValueError("no saved Deribit snapshot")
    state=pool_from_sql(db,pool,opt["fetched_at_utc"])
    if state["pool_option_time_delta_seconds"]>1800:raise ValueError("pool and option snapshots too far apart")
    spot=state["price"]
    deribit_spot=float(opt["spot_usdc_per_sol"])
    if abs(deribit_spot/spot-1)>.03:raise ValueError(f"pool/Deribit price basis exceeds 3%: pool={spot:.5f} deribit={deribit_spot:.5f} active_id={state['active_id']} snapshot={state['snapshot_id']} bin_count={state['bin_count']}")
    factor=wealth/state["aggregate_reserves_mark_usdc"]
    weights=[(s,k,q*factor,binid) for s,k,q,binid in state.pop("weights")]
    result={"status":"ok","created_utc":dt.datetime.now(dt.timezone.utc).isoformat(),
        "pool":pool,"position_model":"synthetic pro-rata pool inventory; NOT a real DLMM position",
        "hypothetical_capital_usdc":wealth,
        "pool_state":state,
        "deribit_snapshot":{"id":opt["snapshot_id"],"fetched_at_utc":opt["fetched_at_utc"],
          "expiry_utc":opt["expiry_utc"],"index_usdc_per_sol":deribit_spot,
          "pool_price_usdc_per_sol":spot,
          "basis_pct":100*(spot/deribit_spot-1)},
        "weights":{"strike_entries":len(weights),"sum_put_underlying_sol":sum(q for s,k,q,i in weights if s=="put"),
                   "sum_call_underlying_sol":sum(q for s,k,q,i in weights if s=="call")},
        "history_window":historical_window(db,pool,opt["fetched_at_utc"],opt["expiry_timestamp_ms"],spot),
        "scenarios":[]}
    for width in widths:
        lo,hi=spot*(1-width),spot*(1+width)
        lots=extract_lots(db,opt["snapshot_id"],spot,lo,hi)
        for lam in lambdas:
            sol=solve(weights,lots,lo,hi,spot,lam)
            result["scenarios"].append(dict(price_window_pm_pct=100*width,
                              low_usdc=lo,high_usdc=hi,**sol))
    db.close()
    return result

def selftest():
    weights=[("put",90.,2.0,1),("call",110.,2.0,2)]
    ideal=[dict(strike=90.,side="put",quantity_sol=2.),dict(strike=110.,side="call",quantity_sol=2.)]
    assert evaluate(weights,ideal,80,120,100)["max_abs_error_usdc"]<1e-9
    assert evaluate(weights,[],80,120,100)["max_abs_error_usdc"]==20
    # With whole 10 SOL and large cost, no hedge is favored.
    lot=[dict(instrument="P90",side="put",strike=90.,size=10.,premium=100.,fee=.3,cost=100.3,lot=0)]
    r=solve(weights,lot,80,120,100,1.)
    assert r["status"]=="risk_certified" and r["contract_count"]==0
    print("PASS: exact DLMM breakpoint certificate; zero-hedge feasible; integer MILP")

def main():
    p=argparse.ArgumentParser()
    p.add_argument("--db",type=Path,default=Path("data/sol_dlmm_deribit.db"))
    p.add_argument("--pool",default=DEFAULT_POOL)
    p.add_argument("--wealth",type=float,default=10000.)
    p.add_argument("--out",type=Path,default=Path("research-artifacts/dlmm_option_window_strategy.json"))
    p.add_argument("--self-test",action="store_true")
    a=p.parse_args()
    if a.self_test:return selftest()
    if a.db.open("rb").read(16)!=b"SQLite format 3\x00":raise ValueError("Git LFS object missing; database is pointer only")
    r=analysis(a.db,a.pool,a.wealth,[.10,.20],[.1,1.0])
    a.out.parent.mkdir(parents=True,exist_ok=True);a.out.write_text(json.dumps(r,indent=2)+"\n")
    print(json.dumps({"status":r["status"],"pool":r["pool"],"scenarios":[{
        "window_pct":q["price_window_pm_pct"],"lambda":q.get("lambda_usdc_per_usdc"),
        "status":q["status"],"contracts":q.get("contract_count"),
        "cost":q.get("indicative_cost_incl_fees_usdc"),
        "no_hedge_error":q.get("no_hedge",{}).get("max_abs_error_usdc"),
        "hedged_error":q.get("hedged",{}).get("max_abs_error_usdc")
    } for q in r["scenarios"]]}))
if __name__=="__main__":sys.exit(main() or 0)
