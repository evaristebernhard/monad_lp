#!/usr/bin/env python3
"""Collect public Monad quotes using Python's standard library only."""

import argparse
import json
from pathlib import Path
import re
import sys
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode, urlsplit
from urllib.request import Request, urlopen


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--api-url', default='https://api.chayu123.me')
    parser.add_argument('--amount', default='0.1', help='Input amount as a decimal string')
    parser.add_argument('--token-in', type=str.upper, choices=['MON', 'USDC'], default='MON')
    parser.add_argument('--token-out', type=str.upper, choices=['MON', 'USDC'])
    parser.add_argument('--count', type=int, default=1, help='Request attempts; 0 runs until Ctrl+C')
    parser.add_argument('--interval', type=float, default=2, help='Seconds between completed requests')
    parser.add_argument('--timeout', type=float, default=30)
    parser.add_argument('--output', type=Path, help='Append full responses as UTF-8 JSONL')
    args = parser.parse_args()
    origin = args.api_url.rstrip('/')
    parsed = urlsplit(origin)
    if parsed.scheme not in ('http', 'https') or not parsed.hostname or parsed.path or parsed.query or parsed.fragment or parsed.username:
        parser.error('--api-url must be an origin such as https://api.chayu123.me, without /api')
    if not re.fullmatch(r'[0-9]+(?:\.[0-9]+)?', args.amount) or not any(c in '123456789' for c in args.amount):
        parser.error('--amount must be a positive decimal string, without scientific notation')
    if args.count < 0 or not 1 <= args.interval < float('inf') or not 0 < args.timeout < float('inf'):
        parser.error('--count must be >= 0, --interval >= 1, and --timeout > 0')
    token_out = args.token_out or ('USDC' if args.token_in == 'MON' else 'MON')
    if args.token_in == token_out:
        parser.error('Input and output tokens must differ')
    url = origin + '/api/selection/quote?' + urlencode({
        'amount': args.amount, 'token_in': args.token_in, 'token_out': token_out,
    })
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
    attempt = failures = 0
    try:
        while args.count == 0 or attempt < args.count:
            attempt += 1
            try:
                request = Request(url, headers={'Accept': 'application/json', 'User-Agent': 'monad-quotes/1.0'})
                with urlopen(request, timeout=args.timeout) as response:
                    frame = json.load(response)
                rows = frame['scenarios'][0]['quotes']
                best = max(int(row['amountOutRaw']) for row in rows if row['status'] == 'quoted')
                if frame['sourceChainId'] != 143 or int(frame['selected']['amountOutRaw']) != best:
                    raise ValueError('Unexpected chain or selected output does not match the best quoted output')
                received = int(time.time() * 1000)
                frame['clientReceivedAtMs'] = received
                frame['clientAgeMs'] = max(0, received - int(frame['sourceTimestamp']) * 1000)
                frame['clientFresh'] = frame.get('fresh') is True and frame['clientAgeMs'] <= 5000
                line = json.dumps(frame, ensure_ascii=False, separators=(',', ':'))
                if args.output:
                    with args.output.open('a', encoding='utf-8') as handle:
                        handle.write(line + '\n')
                    print(json.dumps({
                        'sourceBlock': frame['sourceBlock'], 'clientFresh': frame['clientFresh'],
                        'available': frame['available'], 'candidateCount': frame['candidateCount'],
                        'selected': frame['selected'], 'output': str(args.output),
                    }, ensure_ascii=False), flush=True)
                else:
                    print(line, flush=True)
            except (HTTPError, URLError, TimeoutError, OSError, ValueError, KeyError, TypeError) as error:
                failures += 1
                if isinstance(error, HTTPError):
                    detail = f'HTTP {error.code}: {error.read(1000).decode("utf-8", errors="replace")}'
                else:
                    detail = str(error)
                print(f'Request {attempt} failed: {detail}', file=sys.stderr, flush=True)
            if args.count == 0 or attempt < args.count:
                time.sleep(args.interval)
    except KeyboardInterrupt:
        return 130
    return 1 if failures else 0


if __name__ == '__main__':
    sys.exit(main())
