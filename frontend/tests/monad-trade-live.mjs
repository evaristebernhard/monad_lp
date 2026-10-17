// Opt-in real read-only preflight; never requests signatures or sends a transaction.
// MONAD_API_ORIGIN=http://127.0.0.1:8100 MONAD_TEST_WALLET=0x... node frontend/tests/monad-trade-live.mjs
import { createPublicClient, http, formatUnits } from 'viem'
import { monad } from 'viem/chains'
import { quoteMon, buildSwap } from '../src/monadTrade.js'
import assert from 'node:assert/strict'
const account=process.env.MONAD_TEST_WALLET
if(!/^0x[0-9a-fA-F]{40}$/.test(account||''))throw Error('Set MONAD_TEST_WALLET to a funded Monad wallet address; this test is read-only.')
const client=createPublicClient({chain:monad,transport:http(`${process.env.MONAD_API_ORIGIN||'http://127.0.0.1:8100'}/api/monad/trade-rpc`,{retryCount:0,timeout:22000})})
const quote=await quoteMon(10n**17n,50,client),block=await client.getBlock()
const tx=buildSwap({account,amountIn:quote.amountIn,amountOutMinimum:quote.minimum,deadline:block.timestamp+120n})
const [gas,gasPrice,balance,blockNumber]=await Promise.all([client.estimateGas(tx),client.getGasPrice(),client.getBalance({address:account}),client.getBlockNumber()])
assert(gas>0n&&gasPrice>0n&&blockNumber>0n)
assert(balance>=tx.value+gas*120n/100n*gasPrice*2n,'Balance must cover 0.1 MON and gas reserve')
console.log(JSON.stringify({readOnly:true,broadcast:false,quoteUsdc:formatUnits(quote.amountOut,6),minimumUsdc:formatUnits(quote.minimum,6),gas:gas.toString(),gasPrice:gasPrice.toString(),block:blockNumber.toString()}))
