import copy
import time
import unittest
from decimal import Decimal
from pathlib import Path
from buy_usdc import expected_calldata, validate, ROUTER

WALLET = '0x41d5d3b5f2e497d1f4bcf767ffac9ddab85d2b79'

class ValidationTests(unittest.TestCase):
    def test_matches_browser_encoding(self):
        # Source checkout only; examples can be copied independently to a client.
        fixture = Path(__file__).resolve().parents[2] / 'backend/lp-api/tests/mon-buy-calldata.txt'
        self.assertEqual('0x' + expected_calldata(10**17, 12345, 2000000000).hex(), fixture.read_text().strip())

    def test_rejects_tampered_transactions(self):
        now = int(time.time())
        body = {'chain_id':143, 'expires_at':now+30, 'deadline':now+120,
                'amount_out_raw':'10000', 'minimum_out_raw':'9950',
                'transaction':{'chainId':143, 'from':WALLET, 'to':ROUTER,
                    'value':str(10**17), 'data':'0x'+expected_calldata(10**17,9950,now+120).hex()}}
        validate(body,WALLET,10**17,50,Decimal(0))
        for field, value in [('to',WALLET),('from',ROUTER),('value','1'),('chainId',1),('data','0x')]:
            changed = copy.deepcopy(body)
            changed['transaction'][field] = value
            with self.assertRaises(ValueError):
                validate(changed,WALLET,10**17,50,Decimal(0))
        for field, value in [('chain_id',1),('expires_at',now-1),('deadline',now+1000),('minimum_out_raw','1')]:
            changed = copy.deepcopy(body)
            changed[field] = value
            with self.assertRaises(ValueError):
                validate(changed,WALLET,10**17,50,Decimal(0))
        with self.assertRaises(ValueError):
            validate(body,WALLET,10**17,50,Decimal('1'))

if __name__ == '__main__':
    unittest.main()
