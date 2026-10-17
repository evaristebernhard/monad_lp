import unittest,time
from unittest.mock import patch
from fetch_deribit_sol import fetch,positive

class DeribitCollectorTests(unittest.TestCase):
    def test_positive(self):
        self.assertIsNone(positive("nan"))
        self.assertIsNone(positive(0))
        self.assertEqual(positive("2.5"),2.5)
    def test_offline_sample(self):
        expiry=int((time.time()+8*86400)*1000)
        def mock(method,**params):
            if method=="get_index_price":return {"index_price":112.45}
            if method=="get_instruments":return [
                dict(instrument_name=f"SOL_USDC-X-110-{s}",option_type=t,strike=110,
                     contract_size=10,min_trade_amount=1,is_active=True,expiration_timestamp=expiry)
                for s,t in (("C","call"),("P","put"))]
            if method=="get_book_summary_by_currency":return [
                dict(instrument_name=f"SOL_USDC-X-110-{s}",bid_price=1.0,ask_price=1.2)
                for s in ("C","P")]
            raise ValueError(method)
        with patch("fetch_deribit_sol.get",side_effect=mock):
            x=fetch(7,10)
        self.assertEqual(x["selected_count"],2)
        self.assertEqual(x["two_sided_count"],2)
        self.assertEqual({v["side"] for v in x["options"]},{"call","put"})

if __name__=="__main__":unittest.main()
