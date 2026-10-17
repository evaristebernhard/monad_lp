import importlib.util
import math
import io
import json
from pathlib import Path
import sqlite3
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('coverage', Path(__file__).with_name('monad-pool-coverage.py'))
coverage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(coverage)


class HistoryTests(unittest.TestCase):
    def test_distinct_pair_names_ignore_case_direction_protocol_and_fee(self):
        self.assertEqual(coverage.pair_key({'pair':'AUSD/USDC'}),coverage.pair_key({'pair':'usdc/ausd'}))
        self.assertNotEqual(coverage.pair_key({'pair':'MON/USDC'}),coverage.pair_key({'pair':'WMON/USDC'}))

    def setUp(self):
        self.pool = dict(pool_address='pool', token_a_address='0xaaa', token_b_address='0xbbb',
                         pair='A/B',protocol='Uniswap V4',fee_apr=0.1,volume_24h_usd=5,updated_at='old-snapshot')
        self.end = 3600*100

    def payload(self, rows, reverse=False):
        a,b=('0xbbb','0xaaa') if reverse else ('0xaaa','0xbbb')
        return dict(meta=dict(base=dict(address=a),quote=dict(address=b)),data=dict(attributes=dict(ohlcv_list=rows)))

    def test_ratio_direction_and_identity(self):
        row=[self.end-3600,2,2,2,2,5]
        self.assertEqual(coverage.decode(self.payload([row]),self.pool,self.end),[(row[0],2)])
        self.assertEqual(coverage.decode(self.payload([row],True),self.pool,self.end),[(row[0],0.5)])
        bad=self.payload([row]);bad['meta']['quote']['address']='0xother'
        with self.assertRaisesRegex(ValueError,'metadata mismatch'):
            coverage.decode(bad,self.pool,self.end)

    def test_skip_partial_invalid_and_future_candles_without_filling(self):
        rows=[[self.end,2,2,2,2,0],[self.end-3600,2,2,2,float('nan'),0],
              [self.end-7200,2,2,2,2,10],[self.end-14400,2,2,2,2,10]]
        self.assertEqual(len(coverage.decode(self.payload(rows),self.pool,self.end)),2)

    def test_strict_window_and_reversed_local_prices(self):
        db=sqlite3.connect(':memory:');db.row_factory=sqlite3.Row
        db.execute('CREATE TABLE pool_price_history(chain,pool_address,timestamp,token0_address,token1_address,token1_per_token0,source,observed_at,price_status)')
        for i in range(24):
            t=self.end-86400+i*3600
            db.execute("INSERT INTO pool_price_history VALUES('monad','pool',?,'0xbbb','0xaaa',?,'geckoterminal_pool_ratio_1h',?,'priced')",(t,2 if i%2 else 1,t+3600))
        result=coverage.audit(db,self.pool,self.end)
        self.assertTrue(result['complete']);self.assertEqual(result['samples'],24)
        self.assertTrue(math.isfinite(result['vol']));self.assertEqual(result['price'],0.5)
        self.assertEqual(result['catalog_as_of'],'old-snapshot')
        self.assertEqual(result['price_as_of'],self.end)
        db.execute('DELETE FROM pool_price_history WHERE timestamp=?',(self.end-10*3600,))
        self.assertIsNone(coverage.audit(db,self.pool,self.end)['vol'])

    def test_write_canonical_orientation_preserve_rpc_and_source_time(self):
        db=sqlite3.connect(':memory:')
        db.execute('''CREATE TABLE pool_price_history(chain,pool_address,timestamp,token0_address,token1_address,
            token1_per_token0,token0_price_usd,token1_price_usd,source,price_status,status_reason,observed_at,
            PRIMARY KEY(chain,pool_address,timestamp))''')
        stamp=self.end-3600
        db.execute("INSERT INTO pool_price_history VALUES('monad','pool',?,'0xaaa','0xbbb',9,NULL,NULL,'monad_listener_rpc','priced',NULL,?)",(stamp,stamp+50))
        pool=dict(self.pool,token_a_address='0xbbb',token_b_address='0xaaa')
        payload=self.payload([[stamp,4,4,4,4,1],[stamp-3600,4,4,4,4,1]],True)
        with patch.object(coverage.urllib.request,'urlopen',return_value=io.BytesIO(json.dumps(payload).encode())):
            self.assertEqual(coverage.fetch_history(db,pool,self.end,'https://example.test'),2)
        self.assertEqual(db.execute('SELECT token1_per_token0,source,observed_at FROM pool_price_history WHERE timestamp=?',(stamp,)).fetchone(),(9,'monad_listener_rpc',stamp+50))
        self.assertEqual(db.execute('SELECT token0_address,token1_address,token1_per_token0,source,observed_at FROM pool_price_history WHERE timestamp=?',(stamp-3600,)).fetchone(),('0xaaa','0xbbb',0.25,coverage.SOURCE,stamp))


if __name__ == '__main__':
    unittest.main()
