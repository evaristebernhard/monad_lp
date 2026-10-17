#!/usr/bin/env python3
"""Backfill observed Monad pool ratios; audit four metrics without a TVL cutoff.

No RPC, token USD proxy, interpolation or empty-candle filling. Re-run hourly
with --fetch; metric HTTP requests remain local-only. Reports retain timestamps.
"""
import argparse
import csv
import json
import math
from pathlib import Path
import sqlite3
import statistics
import time
import urllib.parse
import urllib.request
import urllib.error

ROOT = Path(__file__).resolve().parents[2]
PREFERRED = [
    ('MON/USDC', 'Uniswap V4'), ('MON/WETH', 'Uniswap V4'),
    ('MON/WBTC', 'Uniswap V4'), ('USDC/GHO', 'Uniswap V4'),
    ('AUSD/USDC', 'Uniswap V4'), ('USDC/syrupUSDC', 'Uniswap V4'),
    ('WMON/LVMON', 'PancakeSwap V3'), ('LV/LVMON', 'PancakeSwap V3'),
    ('shMON/WMON', 'Uniswap V3'), ('APR/WMON', 'PancakeSwap V3'),
    ('WMON/CAKE', 'PancakeSwap V3'), ('USDC/DUST', 'Uniswap V2'),
    ('SAVVA.M/WMON', 'Uniswap V2'), ('MON/CHOG', 'Uniswap V4'),
    ('WMON/MONIKA', 'PancakeSwap V3'), ('MON/GMONAD', 'Uniswap V4'),
    ('MON/SHRAMP', 'Uniswap V4'), ('XAUt0/USDT0', 'PancakeSwap V3'),
    ('aHOOD/USDC', 'Uniswap V4'), ('AUSD/USDT0', 'Uniswap V4'),
]
NATIVE = '0x' + '0' * 40
WMON = '0x3bd359c1119da7da1d913d1c4d2b7c461115433a'
SOURCE = 'geckoterminal_pool_ratio_1h'


def pair_key(pool):
    # Reversed ordering, letter case and different protocols/fees do not make
    # a new display name. Native MON and WMON remain distinct.
    return tuple(sorted(pool['pair'].casefold().split('/')))


def connection(path):
    c = sqlite3.connect(path, timeout=30, uri=True)
    c.row_factory = sqlite3.Row
    return c


def candidates(c):
    metadata = {r['address'].lower(): r['symbol'] for r in c.execute(
        "SELECT address,symbol FROM token_metadata WHERE chain='monad' AND status='resolved'")}
    rows = []
    for raw in c.execute("SELECT p.*,s.fee_bps AS rpc_fee FROM pools p LEFT JOIN pool_rpc_state s ON s.chain=p.chain AND s.pool_address=p.pool_address WHERE p.chain='monad'"):
        p = dict(raw)
        for side in ('a', 'b'):
            a = (p['token_'+side+'_address'] or '').lower()
            p['token_'+side+'_symbol'] = 'MON' if a == NATIVE else 'WMON' if a == WMON else metadata.get(a) or p['token_'+side+'_symbol']
        p['pair'] = p['token_a_symbol'] + '/' + p['token_b_symbol']
        fee = p['rpc_fee']
        if fee is None and p['pool_type'] != 'uniswap_v4':
            fee = p['fee_bps']
            if fee is not None and fee > 1000:
                fee /= 100
        volume, tvl = p['volume_24h_usd'], p['liquidity_usd']
        p['effective_fee_bps'] = fee
        p['fee_apr'] = volume * fee / 10000 * 365 / tvl * (p['lp_fee_share_assumption'] if p['lp_fee_share_assumption'] is not None else 1) if volume and fee and tvl and tvl > 0 else None
        if volume and volume > 0 and p['fee_apr'] is not None and p['fee_apr'] > 0:
            rows.append(p)
    rows.sort(key=lambda p: (-p['volume_24h_usd'], p['pool_address']))
    preferred, seen = [], set()
    for pair, protocol in PREFERRED:
        symbols = sorted(pair.lower().split('/'))
        matches = [p for p in rows if p['protocol'] == protocol and sorted(p['pair'].lower().split('/')) == symbols]
        if matches:
            preferred.append(matches[0]); seen.add(matches[0]['pool_address'])
    return preferred + [p for p in rows if p['pool_address'] not in seen]


def decode(payload, pool, end):
    """Validate identities and completed hourly candles; currency=token/base."""
    a, b = (pool['token_'+side+'_address'].lower() for side in ('a', 'b'))
    meta = payload['meta']
    base, quote = meta['base']['address'].lower(), meta['quote']['address'].lower()
    if {base, quote} != {a, b}:
        raise ValueError('pool token metadata mismatch')
    points = {}
    for row in payload['data']['attributes']['ohlcv_list']:
        if len(row) < 6:
            continue
        stamp, close = row[0], float(row[4])
        if not isinstance(stamp, int) or stamp % 3600 or stamp >= end or close <= 0 or not math.isfinite(close):
            continue
        ratio = close if base == a else 1 / close
        if math.isfinite(ratio) and ratio > 0:
            points[stamp] = ratio
    return sorted(points.items())


def fetch_history(db, p, end, origin):
    params = urllib.parse.urlencode(dict(aggregate=1, limit=168, before_timestamp=end,
                                        currency='token', token='base', include_empty_intervals='false'))
    url = origin.rstrip('/') + '/pools/' + p['pool_address'] + '/ohlcv/hour?' + params
    request = urllib.request.Request(url, headers={'Accept': 'application/json;version=20230203', 'User-Agent': 'monad-pool-history/1'})
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=20) as response:
                payload = json.load(response)
            break
        except urllib.error.HTTPError as error:
            if error.code != 429 or attempt == 2:
                raise
            delay = min(max(float(error.headers.get('Retry-After', 30)), 30), 60)
            print('Provider rate limit; retry in', delay, 'seconds', flush=True)
            time.sleep(delay)
        except (urllib.error.URLError,TimeoutError):
            if attempt==2:
                raise
            time.sleep(5*(attempt+1))
    points = decode(payload, p, end)
    if not points:
        raise ValueError('no completed hourly candles')
    token0,token1=sorted((p['token_a_address'].lower(),p['token_b_address'].lower()))
    with db:
        for stamp, ratio in points:
            canonical_ratio=ratio if token0==p['token_a_address'].lower() else 1/ratio
            # Preserve actual RPC observations already recorded for that hour.
            db.execute("""INSERT INTO pool_price_history(chain,pool_address,timestamp,token0_address,token1_address,
                token1_per_token0,token0_price_usd,token1_price_usd,source,price_status,status_reason,observed_at)
                VALUES('monad',?,?,?,?,?,NULL,NULL,?,'priced',NULL,?)
                ON CONFLICT(chain,pool_address,timestamp) DO UPDATE SET
                token0_address=excluded.token0_address,token1_address=excluded.token1_address,
                token1_per_token0=excluded.token1_per_token0,source=excluded.source,
                price_status=excluded.price_status,status_reason=NULL,observed_at=excluded.observed_at
                WHERE pool_price_history.source=? OR pool_price_history.price_status!='priced'""",
                (p['pool_address'], stamp, token0, token1, canonical_ratio, SOURCE, stamp+3600, SOURCE))
    return len(points)


def audit(db, p, end):
    rows = db.execute("""SELECT timestamp,token0_address,token1_address,token1_per_token0,source,observed_at
        FROM pool_price_history WHERE chain='monad' AND lower(pool_address)=lower(?) AND price_status='priced'
        AND timestamp>=? AND timestamp<? ORDER BY timestamp""", (p['pool_address'], end-86400, end)).fetchall()
    points, sources = {}, set()
    for r in rows:
        a,b = r['token0_address'].lower(),r['token1_address'].lower()
        pa,pb = p['token_a_address'].lower(),p['token_b_address'].lower()
        value = r['token1_per_token0']
        if {a,b}!={pa,pb} or value is None or not math.isfinite(value) or value<=0:
            continue
        points[r['timestamp']] = value if a==pa else 1/value
        sources.add(r['source'])
    complete = all(t in points for t in range(end-86400,end,3600))
    ordered = [points[t] for t in sorted(points)]
    vol = statistics.stdev([math.log(b/a) for a,b in zip(ordered,ordered[1:])])*math.sqrt(24) if complete else None
    return dict(pair=p['pair'],protocol=p['protocol'],pool=p['pool_address'],base_token=p['token_a_address'],
                quote_token=p['token_b_address'],vol=vol,price=ordered[-1] if ordered else None,
                fee_apr=p['fee_apr'],volume=p['volume_24h_usd'],samples=len(points),
                fee_bps=p.get('effective_fee_bps'),
                price_as_of=max(points)+3600 if points else None,window_end=end,
                catalog_as_of=p['updated_at'],sources=sorted(sources),complete=complete,
                stale=not points or max(points)+3600 < end-3600)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--catalog',type=Path,default=ROOT/'data/lp_screener.db')
    parser.add_argument('--prices',type=Path,default=ROOT/'data/lp_price_history.db')
    parser.add_argument('--fetch',action='store_true')
    parser.add_argument('--fetch-pool',action='append',default=[],help='Limit external fetching to these pool IDs (repeatable); audit still checks all candidates')
    parser.add_argument('--max-fetch',type=int,default=50)
    parser.add_argument('--interval',type=float,default=15)
    parser.add_argument('--base-url',default='https://api.geckoterminal.com/api/v2/networks/monad')
    parser.add_argument('--api',help='Verify all four actual metric endpoints against this running API origin')
    parser.add_argument('--output',type=Path,default=ROOT/'output/monad-four-metrics.json')
    args=parser.parse_args()
    end=int(time.time())//3600*3600
    catalog=connection('file:'+str(args.catalog)+'?mode=ro')
    db=connection(args.prices if args.fetch else 'file:'+str(args.prices)+'?mode=ro')
    if args.fetch:
        db.execute("CREATE TABLE IF NOT EXISTS monad_history_fetch(pool TEXT PRIMARY KEY,checked_at INTEGER,status TEXT,error TEXT)")
    selected,checked,attempts,selected_names=[],[],0,set()
    for p in candidates(catalog):
        if pair_key(p) in selected_names:
            continue
        result=audit(db,p,end)
        previous=db.execute('SELECT * FROM monad_history_fetch WHERE pool=?',(p['pool_address'],)).fetchone() if args.fetch else None
        fetch_allowed=not args.fetch_pool or p['pool_address'].lower() in {x.lower() for x in args.fetch_pool}
        if args.fetch and fetch_allowed and not result['complete'] and attempts<args.max_fetch and (not previous or previous['status']!='ok' or time.time()-previous['checked_at']>900):
            attempts+=1
            try:
                count=fetch_history(db,p,end,args.base_url)
                status,error='ok',None
                print(p['pair'],p['pool_address'],count,'hourly prices',flush=True)
            except Exception as e:
                status,error='error',str(e)
                print(p['pair'],p['pool_address'],error,flush=True)
            with db:
                db.execute('INSERT OR REPLACE INTO monad_history_fetch VALUES(?,?,?,?)',(p['pool_address'],int(time.time()),status,error))
            time.sleep(max(0,args.interval))
            result=audit(db,p,end)
        checked.append(result)
        if result['complete'] and result['vol'] is not None and result['price'] is not None:
            selected.append(result)
            selected_names.add(pair_key(p))
        if len(selected)==20:
            break
    report=dict(checked_at=int(time.time()),window_end=end,units=dict(vol='daily_fraction',fee_apr='annual_fraction',price='quote_per_base',volume='USD'),
                tvl_filter=False,unique_pairs=True,selected_count=len(selected),selected=selected,checked=checked)
    if args.api:
        for pool in selected:
            pool['api_metrics']={}
            for metric in ('vol','fee_apr','price','volume'):
                suffix='?'+urllib.parse.urlencode({'base':pool['base_token']}) if metric=='price' else ''
                url=args.api.rstrip('/')+'/monad/'+pool['pool']+'/'+metric+suffix
                with urllib.request.urlopen(url,timeout=10) as response:
                    payload=json.load(response)
                value=payload.get('value')
                if not isinstance(value,(int,float)) or not math.isfinite(value) or value<0 or (metric!='vol' and value==0):
                    raise ValueError('Metric unavailable: '+pool['pool']+'/'+metric)
                pool['api_metrics'][metric]=payload
        report['api_verified_at']=int(time.time())
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(report,indent=2)+'\n')
    with args.output.with_suffix('.csv').open('w',newline='') as file:
        fields=['pool','pair','protocol','base_token','quote_token','fee_bps','vol','fee_apr','price','volume',
                'vol_as_of','fee_apr_as_of','price_as_of','volume_as_of','price_status','source']
        writer=csv.DictWriter(file,fieldnames=fields)
        writer.writeheader()
        for pool in selected:
            row={key:pool.get(key) for key in fields}
            row.update(vol_as_of=pool['price_as_of'],fee_apr_as_of=pool['catalog_as_of'],
                       volume_as_of=pool['catalog_as_of'],price_status='historical',source=','.join(pool['sources']))
            for metric,payload in pool.get('api_metrics',{}).items():
                row[metric]=payload['value'];row[metric+'_as_of']=payload['as_of']
                if metric=='price':row['price_status']=payload['status']
            writer.writerow(row)
    print('Selected',len(selected),'of 20; report:',args.output,flush=True)
    return 0 if len(selected)==20 else 2


if __name__=='__main__':
    raise SystemExit(main())
