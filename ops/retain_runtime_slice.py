"""Save a bounded price slice; optionally remove unreferenced SQLite LFS copies.

Run from the repository root. Never removes live databases or referenced objects.
"""
import argparse
import gzip
import json
from pathlib import Path
import sqlite3
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--clean-lfs', action='store_true')
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
out = root / 'output' / 'retained-price-slice'
out.mkdir(parents=True, exist_ok=True)
db = sqlite3.connect(f'file:{root}/data/lp_price_history.db?mode=ro', uri=True)
db.row_factory = sqlite3.Row
manifest = {'window': 'last hour relative to latest observation in each table', 'tables': {}}
for table in ('token_price_history', 'pool_price_history'):
    end = db.execute(f'SELECT max(timestamp) FROM {table}').fetchone()[0]
    count = 0
    with gzip.open(out / f'{table}.jsonl.gz', 'wt') as stream:
        for row in db.execute(f'SELECT * FROM {table} WHERE timestamp >= ?', ((end or 0) - 3600,)):
            stream.write(json.dumps(dict(row), ensure_ascii=False) + '\n')
            count += 1
    manifest['tables'][table] = {'latest_timestamp': end, 'rows': count}
db.close()
lines = subprocess.check_output(['git', 'lfs', 'ls-files', '--all', '--long'], cwd=root, text=True).splitlines()
refs = {line.split()[0] for line in lines if line}
candidates = []
for p in (root / '.git/lfs/objects').glob('*/*/*'):
    if not p.is_file() or p.name in refs or len(p.name) != 64:
        continue
    with p.open('rb') as stream:
        if stream.read(16) == b'SQLite format 3\x00':
            candidates.append((p, p.stat().st_size))
manifest['unreferenced_sqlite_objects'] = len(candidates)
manifest['unreferenced_bytes'] = sum(size for _, size in candidates)
manifest['cleaned'] = False
(out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
if args.clean_lfs:
    # Ignore active tracked DBs during status scans: otherwise each write creates
    # another full-size LFS object, even though nobody committed that version.
    tracked = subprocess.check_output(['git', 'ls-files', '-z'], cwd=root).split(b'\x00')
    paths = [p.decode() for p in tracked if p.endswith(b'.db')]
    if paths:
        subprocess.run(['git', 'update-index', '--skip-worktree', '--', *paths], cwd=root, check=True)
    for p, _ in candidates:
        p.unlink()
    manifest['cleaned'] = True
    manifest['skip_worktree'] = paths
    (out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps(manifest, indent=2))
