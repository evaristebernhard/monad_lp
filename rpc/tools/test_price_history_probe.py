"""Offline checks; no network requests or database writes."""
import importlib.util
import json
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("probe", Path(__file__).with_name("price-history-probe.py"))
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


def encoded(*words):
    return "0x" + "".join(format(value, "064x") for value in words)


class FakeHeaders(probe.RPC):
    def __init__(self, divisor=1):
        super().__init__("arbitrum", "public-catalog-1")
        self.divisor = divisor

    def get_headers(self, numbers):
        for number in numbers:
            self.headers[number] = 1700000000 + number // self.divisor
            self.blocks[number] = {"number": hex(number), "timestamp": hex(self.headers[number]), "hash": hex(number)}


class ProbeTests(unittest.TestCase):
    def test_v3_price_direction(self):
        self.assertEqual(probe.price(encoded(2**96), "base", 10**12), 10**12)

    def test_v2_reserve_price(self):
        self.assertEqual(probe.price(encoded(1000, 2500), "bsc", 1), 2.5)

    def test_zero_price_rejected(self):
        with self.assertRaises(RuntimeError):
            probe.price(encoded(0), "base", 1)

    def test_short_state_rejected(self):
        with self.assertRaises(RuntimeError):
            probe.word("0x", 0)

    def test_timestamp_locator(self):
        rpc = FakeHeaders()
        head = {"number": hex(1000000), "timestamp": hex(1701000000), "hash": "0xf4240"}
        result = rpc.locate([1700991234, 1700999999], head)
        for target, number in result.items():
            self.assertLessEqual(rpc.headers[number], target)
            self.assertGreater(rpc.headers[number + 1], target)

    def test_repeated_block_timestamps(self):
        rpc = FakeHeaders(4)
        head = {"number": hex(1000000), "timestamp": hex(1700250000), "hash": "0xf4240"}
        result = rpc.locate([1700249500, 1700249999], head)
        for target, number in result.items():
            self.assertEqual(number % 4, 3)
            self.assertLessEqual(rpc.headers[number], target)
            self.assertGreater(rpc.headers[number + 1], target)

    def test_url_redaction(self):
        self.assertNotIn("secret", probe.safe("error https://example.test/secret?key=secret"))

    def test_unknown_or_null_response_rejected(self):
        class Response:
            def __init__(self, data):
                self.data = data

            def __enter__(self):
                return self

            def __exit__(self, *args):
                pass

            def read(self, *args):
                return json.dumps(self.data).encode()

        bad_responses = [{"id": 0, "result": None}, {"id": 5, "result": "0x1"},
                         {"id": None, "error": {"message": "maximum 10 calls"}}]
        for data in bad_responses:
            with patch.object(probe.urllib.request, "urlopen", return_value=Response(data)):
                with self.assertRaises(RuntimeError):
                    probe.RPC("base", "public-catalog-1").one("eth_call", [])

    def test_bad_topic_rejected(self):
        class Fake:
            def one(self, *args):
                return [{"blockNumber": "0x1", "blockHash": "x", "transactionHash": "y", "logIndex": "0x0",
                         "transactionIndex": "0x0", "address": "0x123", "topics": ["wrong"]}]

        with self.assertRaises(RuntimeError):
            probe.get_logs(Fake(), "0x123", "arbitrum", 0, 2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
