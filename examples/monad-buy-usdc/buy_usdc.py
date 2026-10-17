#!/usr/bin/env python3
"""Build a MON -> USDC swap; --send opts in to local signing and broadcast."""
import argparse
import json
import os
import time
from decimal import Decimal
from pathlib import Path
from urllib.request import Request, urlopen

from dotenv import load_dotenv
from eth_abi import encode
from web3 import Web3

ROUTER = '0x0d97dc33264bfc1c226207428a79b26757fb9dc3'
USDC = '0x754704bc059f8c67012fed69bc8a327a5aafb603'
ZERO = '0x' + '00' * 20
KEY_TYPE = '(address,address,uint24,int24,address)'
KEY = (ZERO, USDC, 500, 10, ZERO)


def expected_calldata(amount, minimum, deadline):
    """Independently encode the allowed pool, exact spend, minimum and refund."""
    swap = encode([f'({KEY_TYPE},bool,uint128,uint128,bytes)'], [(KEY, True, amount, minimum, b'')])
    actions = encode(['bytes', 'bytes[]'], [bytes.fromhex('060c0f'), [
        swap, encode(['address', 'uint256'], [ZERO, amount]),
        encode(['address', 'uint256'], [USDC, minimum]),
    ]])
    refund = encode(['address', 'address', 'uint256'], [ZERO, '0x' + '00' * 19 + '01', 0])
    return Web3.keccak(text='execute(bytes,bytes[],uint256)')[:4] + encode(
        ['bytes', 'bytes[]', 'uint256'], [bytes.fromhex('1004'), [actions, refund], deadline])


def validate(body, wallet, amount, slippage, min_usdc):
    now = int(time.time())
    if body['chain_id'] != 143 or not now < body['expires_at'] <= now + 35:
        raise ValueError('Wrong chain or expired quote')
    if not now < body['deadline'] <= now + 155:
        raise ValueError('Invalid transaction deadline')
    output, minimum = int(body['amount_out_raw']), int(body['minimum_out_raw'])
    if not 0 < output < 2**128 or minimum != output * (10000 - slippage) // 10000 or minimum <= 0:
        raise ValueError('Invalid output or slippage')
    if Decimal(minimum) / Decimal(10**6) < min_usdc:
        raise ValueError('Minimum USDC output is below --min-usdc')
    tx = body['transaction']
    if (tx['chainId'] != 143 or tx['from'].lower() != wallet.lower()
            or tx['to'].lower() != ROUTER or int(tx['value']) != amount):
        raise ValueError('Transaction wallet, chain, router or spend mismatch')
    data = bytes.fromhex(tx['data'].removeprefix('0x'))
    if data != expected_calldata(amount, minimum, body['deadline']):
        raise ValueError('Transaction calldata differs from the allowed MON -> USDC swap')
    return {'chainId': 143, 'from': Web3.to_checksum_address(wallet),
            'to': Web3.to_checksum_address(ROUTER), 'value': amount, 'data': data}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('amount_mon', help='Exact MON amount, e.g. 0.1')
    parser.add_argument('--env', default=str(Path(__file__).with_name('.env')))
    parser.add_argument('--wallet', help='Wallet address for a preview without loading a key')
    parser.add_argument('--slippage-bps', type=int, default=50)
    parser.add_argument('--min-usdc', type=Decimal, default=Decimal('0'))
    parser.add_argument('--send', action='store_true', help='Sign locally and send a real transaction')
    parser.add_argument('--max-gas', type=int, default=1000000)
    parser.add_argument('--max-gas-price-gwei', type=Decimal, default=Decimal('1000'))
    args = parser.parse_args()
    load_dotenv(args.env)
    amount_decimal = Decimal(args.amount_mon)
    if not amount_decimal.is_finite() or amount_decimal <= 0:
        parser.error('amount_mon must be a positive decimal')
    # Avoid Decimal context rounding for 128-bit quantities.
    from decimal import localcontext
    with localcontext() as context:
        context.prec = 80
        raw = amount_decimal * Decimal(10**18)
        if raw != raw.to_integral_value() or not 0 < raw < 2**128:
            parser.error('amount_mon must fit uint128 wei with at most 18 decimals')
        amount = int(raw)
    if not 1 <= args.slippage_bps <= 100:
        parser.error('slippage-bps must be 1..100')
    account = None
    if args.send:
        try:
            account = Web3().eth.account.from_key(os.environ['PRIVATE_KEY'])
        except Exception:
            parser.error('Set a valid PRIVATE_KEY in your local .env')
        if args.wallet and args.wallet.lower() != account.address.lower():
            parser.error('--wallet differs from the local signing wallet')
    wallet = account.address if account else args.wallet
    if not wallet or not Web3.is_address(wallet):
        parser.error('Provide --wallet for preview, or --send with a local PRIVATE_KEY')
    origin = os.getenv('MONAD_API_URL', 'https://api.chayu123.me').rstrip('/')
    request = Request(origin + '/api/monad/swap/build', data=json.dumps({
        'wallet': wallet, 'amount_mon': args.amount_mon, 'slippage_bps': args.slippage_bps,
    }).encode(), headers={'Content-Type': 'application/json', 'User-Agent': 'monad-buy-usdc/1.0',
               'Accept': 'application/json'}, method='POST')
    with urlopen(request, timeout=30) as response:
        body = json.load(response)
    tx = validate(body, wallet, amount, args.slippage_bps, args.min_usdc)
    print(json.dumps({'wallet': wallet, 'spend_mon': args.amount_mon,
                      'expected_usdc': str(Decimal(body['amount_out_raw']) / 10**6),
                      'minimum_usdc': str(Decimal(body['minimum_out_raw']) / 10**6),
                      'deadline': body['deadline'], 'broadcast': False}, indent=2))
    if not args.send:
        return
    w3 = Web3(Web3.HTTPProvider(os.getenv('MONAD_RPC_URL', 'https://rpc.monad.xyz'), request_kwargs={'timeout': 20}))
    if w3.eth.chain_id != 143:
        raise ValueError('Signing RPC is not Monad mainnet')
    tx['nonce'] = w3.eth.get_transaction_count(wallet, 'pending')
    tx['gasPrice'] = w3.eth.gas_price
    tx['gas'] = w3.eth.estimate_gas(tx) * 120 // 100
    if tx['gas'] > args.max_gas or tx['gasPrice'] > int(args.max_gas_price_gwei * 10**9):
        raise ValueError('Gas exceeds your configured limits')
    if w3.eth.get_balance(wallet) < amount + tx['gas'] * tx['gasPrice']:
        raise ValueError('Insufficient MON for purchase plus gas')
    # Reject quotes that expired while estimating. Never automatically retry a broadcast.
    validate(body, wallet, amount, args.slippage_bps, args.min_usdc)
    signed = account.sign_transaction(tx)
    tx_hash = w3.eth.send_raw_transaction(signed.raw_transaction)
    print('transaction_hash=0x' + tx_hash.hex().removeprefix('0x'))
    receipt = w3.eth.wait_for_transaction_receipt(tx_hash, timeout=180)
    if receipt.status != 1:
        raise RuntimeError('Transaction reverted; inspect the transaction hash')
    print('confirmed_block=' + str(receipt.blockNumber))


if __name__ == '__main__':
    main()
