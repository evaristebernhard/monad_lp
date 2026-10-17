"""Read the local catalog without changing it. Match token addresses, not symbols."""
import json,sqlite3,datetime
from pathlib import Path
root=Path(__file__).resolve().parent
usdc='0x754704bc059f8c67012fed69bc8a327a5aafb603'
mon={'0x0000000000000000000000000000000000000000','0x3bd359c1119da7da1d913d1c4d2b7c461115433a'}
with sqlite3.connect(f'file:{root.parent}/data/lp_screener.db?mode=ro',uri=True) as db:
 db.row_factory=sqlite3.Row
 rows=[dict(r) for r in db.execute("select pool_address,pair,protocol,token_a_address,token_b_address,fee_bps,tick_spacing,liquidity_usd,updated_at from pools where chain='monad'")]
 candidates=[r for r in rows if (str(r['token_a_address']).lower()==usdc and str(r['token_b_address']).lower() in mon) or (str(r['token_b_address']).lower()==usdc and str(r['token_a_address']).lower() in mon)]
report={'asOf':datetime.datetime.now(datetime.timezone.utc).isoformat(),'source':'Local Monad pool catalog; not an exhaustive on-chain index','totalMonadPools':len(rows),'samePairPools':len(candidates),'pools':candidates}
(root/'results/pool-coverage.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({k:v for k,v in report.items() if k!='pools'}))
