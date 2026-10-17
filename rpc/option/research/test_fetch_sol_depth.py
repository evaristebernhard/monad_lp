import unittest
from fetch_sol_depth import parse_asks,enrich

class OrderBookTests(unittest.TestCase):
    def test_filter_level_prices(self):
        self.assertEqual(parse_asks({"asks":[[1.2,4],[.8,3],[-1,100],[2,0]]}),
            [dict(price_usdc_per_sol=.8,amount_sol=3),dict(price_usdc_per_sol=1.2,amount_sol=4)])
    def test_addressed_same_expiry_contracts(self):
        s=dict(status="ok",spot_usdc_per_sol=100,expiry_ms=7,options=[
           dict(instrument="put95",side="put",strike=95,two_sided=True),
           dict(instrument="call105",side="call",strike=105,two_sided=True),
           dict(instrument="call150",side="call",strike=150,two_sided=True)])
        calls=[]
        def fetch(method,**kwargs):
            calls.append(kwargs["instrument_name"])
            return dict(timestamp=1234,asks=[[1,10],[2,20]])
        r=enrich(s,pause=0,request=fetch)
        self.assertEqual(r["depth_attempted"],2)
        self.assertEqual(r["depth_with_asks"],2)
        self.assertEqual(calls,["put95","call105"])
        self.assertEqual(r["options"][2]["book_status"],"not_requested")
if __name__=="__main__":unittest.main()
