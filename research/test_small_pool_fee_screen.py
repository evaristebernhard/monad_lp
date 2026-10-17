import unittest
from small_pool_fee_screen import standardized_fee_bps,classify_actual,lp_share

class FeeUnitTests(unittest.TestCase):
    def test_v4_source_pips_are_not_basis_points(self):
        self.assertEqual(standardized_fee_bps({"pool_type":"uniswap_v4","fee_bps":100}),1.)
        self.assertEqual(standardized_fee_bps({"pool_type":"uniswap_v4","fee_bps":3000}),30.)
        self.assertEqual(standardized_fee_bps({"pool_type":"uniswap_v4","fee_bps":10000}),100.)
        self.assertEqual(standardized_fee_bps({"pool_type":"solana_clmm","fee_bps":25}),25.)
    def test_canonical_eth_native_and_solana_usdt(self):
        eth=[{"address":"0x0000000000000000000000000000000000000000"},
             {"address":"0xdAC17F958D2ee523a2206206994597C13D831ec7"}]
        self.assertEqual(classify_actual("ethereum",eth),"canonical_NATIVE_ETH_USDT")
        sol=[{"address":"So11111111111111111111111111111111111111112"},
             {"address":"Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"}]
        self.assertEqual(classify_actual("solana",sol),"canonical_SOL_USDT")
    def test_uniswap_mainnet_v2_fee_share_not_100percent(self):
        share,note=lp_share("constant_product_v2","Uniswap V2","ethereum")
        self.assertAlmostEqual(share,5/6)
        self.assertIn("protocol_share",note)
if __name__=="__main__":unittest.main()
