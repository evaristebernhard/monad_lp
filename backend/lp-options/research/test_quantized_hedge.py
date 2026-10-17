import unittest
from quantized_hedge import model,construct,quantize,certificate,exact_error,payoff

class ProofTests(unittest.TestCase):
    def test_models(self):
        a,s,b=89.96,112.45,134.94
        for kind in ("cpmm","clmm"):
            L,x0,D,_=model(kind,s,10000,a,b)
            for np,nc in ((1,1),(2,3),(4,4)):
                ideal,E=construct(L,a,s,b,np,nc)
                exact=exact_error(kind,a,s,b,L,x0,D,ideal)
                self.assertAlmostEqual(exact["max_abs_residual_usd"],E,places=7)
                actual=quantize(ideal,10.)
                bd=certificate(ideal,actual,E,a,b)
                worst=exact_error(kind,a,s,b,L,x0,D,actual)
                self.assertLessEqual(worst["max_abs_residual_usd"],bd["total_bound_usd"]+1e-8)
                for i in range(101):
                    spot=a+(b-a)*i/100
                    self.assertLessEqual(abs(D(spot)-payoff(spot,actual)),bd["total_bound_usd"]+1e-8)

    def test_can_skip_small_positions(self):
        L,_,_,_=model("cpmm",112.45,10000,89.96,134.94)
        legs,_=construct(L,89.96,112.45,134.94,2,2)
        self.assertEqual([z["contracts"] for z in quantize(legs,10.)],[0,0,0,0])

    def test_listed_strike_error(self):
        a,s,b=89.96,112.45,134.94
        L,x0,D,_=model("clmm",s,10000,a,b)
        ideal,E=construct(L,a,s,b,4,4)
        strikes=[dict(side=side,strike=k) for side in ("put","call") for k in range(80,151,4)]
        actual=quantize(ideal,10.,strikes)
        cert=certificate(ideal,actual,E,a,b)
        result=exact_error("clmm",a,s,b,L,x0,D,actual)
        self.assertLessEqual(result["max_abs_residual_usd"],cert["total_bound_usd"]+1e-8)

if __name__=="__main__":unittest.main()
