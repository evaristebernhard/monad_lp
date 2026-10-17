import unittest
from integer_hedge import marginal_lots,optimize

class IntegerTests(unittest.TestCase):
    @staticmethod
    def snapshot():
        opts=[]
        for side in ("put","call"):
            for k in (85,90,95,105,110,115):
                opts.append(dict(instrument=f"{side}-{k}",side=side,strike=k,expiry_ms=12345,
                    contract_size_sol=10,min_trade_amount=10,book_status="ok",
                    book_asks=[dict(price_usdc_per_sol=.5,amount_sol=6),
                               dict(price_usdc_per_sol=1.5,amount_sol=24)]))
        return dict(spot_usdc_per_sol=100,expiry_ms=12345,options=opts)
    def test_ask_depth(self):
        self.assertEqual(marginal_lots([dict(price_usdc_per_sol=1,amount_sol=6),
          dict(price_usdc_per_sol=2,amount_sol=17)],10,5),[14,20])
    def test_zero_position_option(self):
        x=optimize(self.snapshot(),"cpmm",1000,.1)
        self.assertEqual(x["status"],"risk_certified")
        self.assertEqual(x["contracts"],0)
    def test_integral_certified(self):
        x=optimize(self.snapshot(),"clmm",10000,50,max_lots=2)
        self.assertEqual(x["status"],"risk_certified")
        self.assertGreater(x["contracts"],0)
        self.assertLess(x["exact_worst_residual_usdc"],x["no_hedge_worst_usdc"])
        self.assertLessEqual(x["exact_worst_residual_usdc"],x["rounds"][-1]["restricted_E"]+1e-5)
    def test_no_depth_no_position(self):
        s=self.snapshot()
        for o in s["options"]:o["book_asks"]=[]
        with self.assertRaisesRegex(ValueError,"no depth-backed"):
            optimize(s,"cpmm",10000)
if __name__=="__main__":unittest.main()
