import importlib.util
import unittest
from pathlib import Path
p=Path(__file__).with_name("meteora_dlmm_hedge.py")
spec=importlib.util.spec_from_file_location("dlmm",p)
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)

class TestDLMM(unittest.TestCase):
    def setUp(self):
        self.bins=[dict(strike=90,delta_sol=5),dict(strike=95,delta_sol=2),
                   dict(strike=105,delta_sol=4),dict(strike=110,delta_sol=3)]
    def test_exact_ideal(self):
        h=m.ideal_legs(100,self.bins)
        self.assertEqual(m.exact_residual(100,self.bins,h,80,120)["max_abs_residual_usdc"],0)
    def test_no_hedge(self):
        self.assertEqual(m.exact_residual(100,self.bins,[],80,120)["max_abs_residual_usdc"],90)
    def test_different_strikes(self):
        h=[dict(side="put",strike=92,quantity_sol=7),dict(side="call",strike=108,quantity_sol=7)]
        v=m.exact_residual(100,self.bins,h,80,120)
        sample=max(abs(m.loss(100,80+i*.01,self.bins)-m.payout(80+i*.01,h)) for i in range(4001))
        self.assertAlmostEqual(v["max_abs_residual_usdc"],sample,places=6)
    def test_no_negative_bin_weights(self):
        with self.assertRaises(ValueError):m.ideal_legs(100,[dict(strike=99,delta_sol=-1)])
if __name__=="__main__":unittest.main()
