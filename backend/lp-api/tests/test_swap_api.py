"""HTTP contract tests against the compiled API and a deterministic local RPC.
Run: python3 backend/lp-api/tests/test_swap_api.py
"""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError
from urllib.request import Request, build_opener, ProxyHandler

ROOT = Path(__file__).resolve().parents[3]
HTTP = build_opener(ProxyHandler({}))

class RPC(BaseHTTPRequestHandler):
    chain = '0x8f'
    age = 0
    rejected = False
    def log_message(self, *args): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        method = body['method']
        result = {'eth_chainId': self.chain,
                  'eth_getBlockByNumber': {'number':'0x123', 'timestamp':hex(int(time.time())-self.age)},
                  'eth_call':'0x'+f'{10000:064x}{170000:064x}'}[method]
        value = {'jsonrpc':'2.0','id':1,'result':result}
        if self.rejected:
            value = {'error': {'message':'private-provider-credential'}}
        data = json.dumps(value).encode()
        self.send_response(200)
        self.send_header('Content-Length',str(len(data)))
        self.end_headers()
        self.wfile.write(data)

class SwapHTTPTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory()
        cls.rpc = ThreadingHTTPServer(('127.0.0.1',0), RPC)
        threading.Thread(target=cls.rpc.serve_forever,daemon=True).start()
        with socket.socket() as sock:
            sock.bind(('127.0.0.1',0))
            port = sock.getsockname()[1]
        cls.url = f'http://127.0.0.1:{port}'
        env = dict(os.environ, LP_API_ADDR=f'127.0.0.1:{port}',
                   LP_RPC_PRICE_URL=f'http://127.0.0.1:{cls.rpc.server_port}',
                   LP_SQLITE_PATH=cls.temp.name+'/catalog.db',
                   LP_PRICE_DB=cls.temp.name+'/prices.db', SOLANA_MARKET_DB=cls.temp.name+'/market.db')
        cls.process = subprocess.Popen([str(ROOT/'backend/target/debug/lp-api')],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        for _ in range(50):
            try:
                HTTP.open(cls.url+'/api/health',timeout=1).close()
                return
            except OSError: time.sleep(0.1)
        raise RuntimeError('test API did not start')
    @classmethod
    def tearDownClass(cls):
        cls.process.terminate()
        cls.process.wait(timeout=5)
        cls.rpc.shutdown()
        cls.rpc.server_close()
        cls.temp.cleanup()
    def setUp(self):
        RPC.chain, RPC.age, RPC.rejected = '0x8f', 0, False
    def post(self,path,body):
        request = Request(self.url+'/api/monad/swap/'+path,data=json.dumps(body).encode(),headers={'Content-Type':'application/json'})
        try:
            with HTTP.open(request,timeout=5) as response: return response.status,json.load(response)
        except HTTPError as response:
            with response:
                return response.code,response.read().decode()
    def test_quote_and_build(self):
        status,body = self.post('quote',{'amount_mon':'0.1'})
        self.assertEqual(status,200)
        self.assertEqual(body['minimum_out_raw'],'9950')
        self.assertNotIn('transaction',body)
        status,body = self.post('build',{'amount_mon':'0.1','wallet':'0x41d5d3b5f2e497d1f4bcf767ffac9ddab85d2b79'})
        self.assertEqual(status,200)
        self.assertEqual(body['transaction']['value'],'100000000000000000')
        self.assertEqual(body['transaction']['chainId'],143)
    def test_invalid_requests(self):
        for body in [{'amount_mon':'0'},{'amount_mon':'1e3'},{'amount_mon':'1','slippage_bps':101},{'amount_mon':'1','private_key':'not-accepted'},{'amount_mon':0.1}]:
            self.assertIn(self.post('quote',body)[0],(400,422))
        self.assertEqual(self.post('build',{'amount_mon':'1'})[0],400)
    def test_wrong_chain(self):
        RPC.chain='0x1'
        self.assertEqual(self.post('quote',{'amount_mon':'1'})[0],502)
    def test_stale_chain(self):
        RPC.age=120
        self.assertEqual(self.post('quote',{'amount_mon':'1'})[0],502)
    def test_provider_errors_are_private(self):
        RPC.rejected=True
        status,body=self.post('quote',{'amount_mon':'1'})
        self.assertEqual(status,502)
        self.assertNotIn('private-provider-credential',body)

if __name__ == '__main__': unittest.main()
