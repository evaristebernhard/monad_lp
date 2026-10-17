"""Offline mathematical regression checks for the multi-protocol SOL research only."""
import unittest
from sol_cross_protocol_option_screen import exact_oracle,optimize_hedge,pay
from quantized_hedge import model

class CrossProtocolResearchTests(unittest.TestCase):
    def test_oracle_finds_clmm_interior_negative_extremum(self):
        spot=100;wealth=10000
        L,x0,D,_=model("clmm",spot,wealth,97,103)
        legs=[{"side":"put","strike":98,"quantity_sol":30},
              {"side":"call","strike":102,"quantity_sol":20}]
        exact=exact_oracle("clmm",85,115,spot,97,103,L,x0,D,legs)
        sampled=max(abs(D(85+(115-85)*i/5000)-
                  sum(z["quantity_sol"]*pay(85+(115-85)*i/5000,z["side"],z["strike"]) for z in legs))
                  for i in range(5001))
        self.assertLessEqual(sampled,exact["max_abs_error_usdc"]+1e-7)
        self.assertLessEqual(exact["max_abs_error_usdc"]-sampled,0.2)
    def test_oracle_cpmm_and_empty_hedge(self):
        spot=100
        L,x0,D,_=model("cpmm",spot,10000,90,110)
        exact=exact_oracle("cpmm",90,110,spot,90,110,L,x0,D,[])
        expected=max(D(90),D(110))
        self.assertAlmostEqual(exact["max_abs_error_usdc"],expected,places=7)
    def test_integer_optimizer_with_synthetic_public_depth(self):
        spot=100
        snap={"status":"ok","spot_usdc_per_sol":spot,"expiry_ms":1800000000000,
              "options":[{"instrument":"put95","side":"put","strike":95,"expiry_ms":1800000000000,
                          "contract_size_sol":10,"min_trade_amount":10,"book_status":"ok",
                          "book_asks":[{"price_usdc_per_sol":1,"amount_sol":20}]},
                         {"instrument":"call105","side":"call","strike":105,"expiry_ms":1800000000000,
                          "contract_size_sol":10,"min_trade_amount":10,"book_status":"ok",
                          "book_asks":[{"price_usdc_per_sol":1,"amount_sol":20}]}]}
        result=optimize_hedge(snap,"clmm",5000,.025,seconds=10)
        self.assertEqual(result["status"],"risk_certified")
        self.assertGreaterEqual(result["contracts"],0)
        self.assertLessEqual(result["hedged"]["max_abs_error_usdc"],
                             result["final_cut"]["restricted_bound"]+1e-5)
        self.assertEqual(result["option_eligible_instruments"],2)
    def test_not_purchase_if_no_depth(self):
        snap={"status":"ok","spot_usdc_per_sol":100,"expiry_ms":1800000000000,"options":[]}
        result=optimize_hedge(snap,"cpmm",5000,.1)
        self.assertEqual(result["status"],"no_quoted_whole_contract_lots")

if __name__=="__main__":unittest.main()
