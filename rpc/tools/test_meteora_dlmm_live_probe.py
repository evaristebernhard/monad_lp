import importlib.util
import struct
import unittest
from pathlib import Path

path = Path(__file__).with_name("meteora-dlmm-live-probe.py")
spec = importlib.util.spec_from_file_location("probe", path)
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class TestMeteoraProbe(unittest.TestCase):
    def test_position_v2_layout_and_nonzero_shares(self):
        data = bytearray(probe.POSITION_SIZE)
        data[:8] = probe.POSITION_DISCRIMINATOR
        data[8:40] = bytes(32)
        data[40:72] = bytes([7]) * 32
        struct.pack_into("<Q", data, 72, 10)
        struct.pack_into("<i", data, 7912, 100)
        struct.pack_into("<i", data, 7916, 101)
        parsed = probe.parse_position(bytes(data), "position")
        self.assertEqual(parsed["lb_pair"], "11111111111111111111111111111111")
        self.assertEqual(parsed["lower_bin_id"], 100)
        self.assertEqual(parsed["liquidity_shares"], [{"bin_id": 100, "liquidity_share": "10"}])

    def test_bin_array_decimal_normalization_and_price(self):
        data = bytearray(probe.BIN_ARRAY_SIZE)
        data[:8] = probe.BIN_ARRAY_DISCRIMINATOR
        struct.pack_into("<q", data, 8, 1)
        data[24:56] = bytes(32)
        offset = probe.BIN_ARRAY_HEADER_SIZE + 30 * probe.BIN_SIZE
        struct.pack_into("<Q", data, offset, 1_000_000_000)
        struct.pack_into("<Q", data, offset + 8, 200_000_000)
        data[offset + 16 : offset + 32] = ((1 << 64) // 5).to_bytes(16, "little")
        data[offset + 32 : offset + 48] = (1000).to_bytes(16, "little")
        parsed = probe.parse_bin_array(
            bytes(data), "11111111111111111111111111111111", 9, 6, 4
        )
        self.assertEqual(parsed["bins"][0]["bin_id"], 100)
        self.assertAlmostEqual(parsed["bins"][0]["price_y_per_x"], 200.0, places=8)

    def test_reject_wrong_discriminator(self):
        with self.assertRaises(ValueError):
            probe.parse_position(bytes(probe.POSITION_SIZE))


if __name__ == "__main__":
    unittest.main()
