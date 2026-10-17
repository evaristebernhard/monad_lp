"""Select real Gecko pool histories by usable time coverage, before any training."""
import datetime as dt
import hashlib
import json
import math
from pathlib import Path
import re
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parent
RAW = ROOT / 'raw'
RAW.mkdir(exist_ok=True)
BASE = 'https://api.geckoterminal.com/api/v2/networks/monad'
ASSETS = [('CHOG', '0x350035555e10d9afaf1566aaebfced5ba6c27777'),
          ('WMON', '0x3bd359c1119da7da1d913d1c4d2b7c461115433a'),
          ('WETH', '0xee8c0e9f1bffb4eb878d8f15f368a02a35481242'),
          ('WBTC', '0x0555e30da8f98308edb960aa94c0db47230d2b9c')]
SOURCES, SCORES = [], []
LAST = 0
PERIOD = 14400
CUTOFF = int(time.time()) // PERIOD * PERIOD


def fetch(path, filename, params=None):
    global LAST
    url = BASE + path + ('?' + urllib.parse.urlencode(params) if params else '')
    meta_path = RAW / (filename + '.meta.json')
    if meta_path.exists():
        source = json.loads(meta_path.read_text())
        if source['url'] == url:
            SOURCES.append(source)
            return json.loads((RAW / filename).read_text())
    for attempt in range(4):
        time.sleep(max(0, 7 - (time.monotonic() - LAST)))
        LAST = time.monotonic()
        try:
            req = urllib.request.Request(url, headers={'Accept': 'application/json', 'User-Agent': 'MonadCoverageStudy/2.0'})
            with urllib.request.urlopen(req, timeout=40) as response:
                body = response.read()
            payload = json.loads(body)
            if 'errors' in payload:
                raise ValueError(str(payload['errors']))
            (RAW / filename).write_bytes(body)
            source = {'url': url, 'file': f'raw/{filename}', 'sha256': hashlib.sha256(body).hexdigest(), 'fetched_at': dt.datetime.now(dt.timezone.utc).isoformat()}
            meta_path.write_text(json.dumps(source))
            SOURCES.append(source)
            return payload
        except urllib.error.HTTPError as error:
            if error.code not in (429, 500, 502, 503, 504) or attempt == 3:
                raise
            time.sleep(15 * (attempt + 1))


def clean(payload, token, period):
    meta = payload['meta']
    assert any(meta.get(side, {}).get('address', '').lower() == token for side in ('base', 'quote'))
    rows, conflicts = {}, set()
    for row in payload['data']['attributes']['ohlcv_list']:
        assert len(row) == 6 and all(isinstance(v, (int, float)) and math.isfinite(v) for v in row)
        t, o, h, l, c, volume = row
        assert t % period == 0 and min(o, h, l, c) > 0 and h >= max(o, c) and l <= min(o, c) and volume >= 0
        if t + period > CUTOFF or t in conflicts:
            continue
        if t in rows and rows[t] != row:
            del rows[t]
            conflicts.add(t)
        else:
            rows[t] = row
    return rows, conflicts


def coverage(rows, period=PERIOD):
    timestamps = sorted(rows)
    if not timestamps:
        return {'count': 0, 'recent_coverage': 0, 'recent_valid_samples': 0, 'latest_age_bars': 100000, 'longest_run': 0}
    recent_start = CUTOFF - 1000 * period
    recent = set(t for t in timestamps if t >= recent_start)
    valid = sum(all(t + step * period in rows for step in range(-24, 6)) for t in recent)
    longest = run = 0
    previous = None
    for t in timestamps:
        run = run + 1 if previous is not None and t - previous == period else 1
        longest = max(longest, run)
        previous = t
    return {'count': len(rows), 'recent_coverage': len(recent) / 1000, 'recent_valid_samples': valid,
            'latest_age_bars': (CUTOFF - period - timestamps[-1]) / period, 'longest_run': longest,
            'latest_timestamp': timestamps[-1], 'from_timestamp': timestamps[0]}


def main():
    chosen = []
    for symbol, token in ASSETS:
        listing = fetch(f'/tokens/{token}/pools', f'{symbol}-pools.json', {'page': 1})
        pools = []
        for pool in listing['data']:
            sides = [pool['relationships'][f'{side}_token']['data']['id'].split('_', 1)[1].lower() for side in ('base', 'quote')]
            a = pool['attributes']
            if token in sides and float(a.get('reserve_in_usd') or 0) >= 1000 and float(a.get('volume_usd', {}).get('h24') or 0) > 0:
                pools.append(pool)
        pools.sort(key=lambda p: float(p['attributes']['volume_usd']['h24']), reverse=True)
        inspected = []
        for index, pool in enumerate(pools[:2]):
            address = pool['attributes']['address']
            payload = fetch(f'/pools/{address}/ohlcv/hour', f'{symbol}-4h-candidate-{index}-ohlcv-0.json',
                {'aggregate': 4, 'limit': 1000, 'currency': 'usd', 'token': token, 'before_timestamp': CUTOFF, 'include_empty_intervals': 'false'})
            rows, conflicts = clean(payload, token, PERIOD)
            score = {'symbol': symbol, 'pool': address, 'pool_name': pool['attributes']['name'], 'period': PERIOD, **coverage(rows)}
            score['eligible'] = score['recent_coverage'] >= .99 and score['recent_valid_samples'] >= 850 and score['latest_age_bars'] <= 1
            SCORES.append(score)
            inspected.append((score, pool, rows, conflicts))
            print(json.dumps(score), flush=True)
            if score['eligible'] and score['recent_valid_samples'] >= 970:
                break
        candidates = [candidate for candidate in inspected if candidate[0]['eligible']]
        if not candidates:
            continue
        score, pool, rows, conflicts = max(candidates, key=lambda c: (c[0]['recent_valid_samples'], c[0]['recent_coverage'], c[0]['longest_run']))
        index = inspected.index((score, pool, rows, conflicts))
        address = pool['attributes']['address']
        # Continue back to the requested 180-day boundary, instead of stopping after two pages.
        history_start = CUTOFF - 180 * 86400
        exhausted = False
        for page in range(1, 8):
            before = min(rows) - 1
            if before < history_start:
                exhausted = True
                break
            payload = fetch(f'/pools/{address}/ohlcv/hour', f'{symbol}-4h-candidate-{index}-ohlcv-{page}.json',
                {'aggregate': 4, 'limit': 1000, 'currency': 'usd', 'token': token, 'before_timestamp': before, 'include_empty_intervals': 'false'})
            older, more_conflicts = clean(payload, token, PERIOD)
            conflicts |= more_conflicts
            for t, row in older.items():
                if t in rows and rows[t] != row:
                    conflicts.add(t)
                else:
                    rows[t] = row
            for t in conflicts:
                rows.pop(t, None)
            print(json.dumps({'symbol': symbol, 'page': page, 'candles': len(rows)}), flush=True)
            if not older or len(payload['data']['attributes']['ohlcv_list']) < 1000:
                exhausted = True
                break
        ordered = sorted(rows.values())
        p = pool['attributes']
        fee = re.search(r'(\d+(?:\.\d+)?)%', p['name'])
        chosen.append({'symbol': symbol, 'address': token, 'pool': address, 'pool_name': p['name'], 'dex': pool['relationships']['dex']['data']['id'],
            'liquidity_usd_at_fetch': float(p['reserve_in_usd']), 'fee_bps_per_side': float(fee.group(1)) * 100 if fee else 100,
            'fee_source': 'pool_name' if fee else 'conservative_config_assumption', 'candles': ordered,
            'conflicting_timestamps_excluded': sorted(conflicts), 'gap_count': sum(b[0] - a[0] != PERIOD for a, b in zip(ordered, ordered[1:])),
            'coverage': coverage({row[0]: row for row in ordered}), 'history_boundary_reached': exhausted})
    selection = {'cutoff': CUTOFF, 'criteria': {'period': PERIOD, 'recent_bars': 1000, 'minimum_coverage': .99, 'minimum_valid_windows': 850, 'maximum_latest_age_bars': 1},
                 'candidates': SCORES, 'selected': [a['symbol'] for a in chosen]}
    (ROOT / 'selection.json').write_text(json.dumps(selection, indent=2) + '\n')
    if len(chosen) < 2:
        raise ValueError('Fewer than two assets meet data coverage criteria')
    dataset = {'source': 'GeckoTerminal index of Monad mainnet DEX trades', 'chain_id': 143, 'currency': 'USD', 'timeframe_seconds': PERIOD,
        'empty_intervals_filled': False, 'assets': chosen, 'sources': list({s['file']: s for s in SOURCES}.values()),
        'fetched_at': dt.datetime.now(dt.timezone.utc).isoformat(), 'selection': selection}
    (ROOT / 'dataset.json').write_text(json.dumps(dataset, indent=2) + '\n')
    config = json.loads((ROOT.parent / 'monad-small/config.json').read_text())
    config['assets'] = [{k: a[k] for k in ('symbol', 'address')} for a in chosen]
    config['pages_per_asset'] = 8
    config['timeframe_seconds'] = PERIOD
    config['lookback_bars'] = 24
    config['horizon_bars'] = 4
    config['lookback_hours'] = 96
    config['horizon_hours'] = 16
    config['features'] = ['return_4h','return_12h','return_24h','return_48h','return_96h','range_to_close','body_to_open','close_vs_sma_24h','close_vs_sma_96h','volatility_24h','volatility_96h','log_volume_vs_mean_96h']
    (ROOT / 'config.json').write_text(json.dumps(config, indent=2) + '\n')
    print(json.dumps({'selected': selection['selected'], 'candles': {a['symbol']: len(a['candles']) for a in chosen}}), flush=True)


if __name__ == '__main__':
    main()
