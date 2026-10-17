import {test} from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import {RpcMarket,poolPrice} from '../../research/ai/showcase/rpc-market.mjs'
import {marketSource} from '../../research/ai/showcase/live-input.mjs'
const model=JSON.parse(fs.readFileSync(new URL('../../research/ai/monad-small/model.json',import.meta.url)))
const sqrt=price=>BigInt(Math.floor(Math.sqrt(price/1e12)*2**96))
test('current RPC price, OHLC range and real swap volume feed the original model; unchanged blocks do not double-count',async()=>{
  const hour=1800000000
  let head=10n
  const events=[{blockNumber:5n,logIndex:0,args:{sqrtPriceX96:sqrt(2.01),amount0:-1n,amount1:3000000n}}]
  const client={
    getBlock:async({blockNumber=head})=>({number:blockNumber,timestamp:BigInt(hour)-2n+blockNumber,hash:'0x'+String(blockNumber)}),
    readContract:async({address,functionName,blockNumber})=>{
      if(functionName==='token0')return marketSource.token
      if(functionName==='token1')return 'usdc'
      if(functionName==='symbol')return 'USDC'
      if(functionName==='decimals')return address==='usdc'?6:18
      if(functionName==='slot0')return [sqrt(blockNumber===1n?2:blockNumber===10n?2.02:1.99)]
      throw Error(functionName)
    },
    getLogs:async({fromBlock,toBlock})=>events.filter(e=>e.blockNumber>=fromBlock&&e.blockNumber<=toBlock)
  }
  const history={source:{candles:Array.from({length:25},(_,i)=>[hour-(25-i)*3600,2,2,2,2,10])}}
  const feed=new RpcMarket(client,model,async()=>history)
  const first=await feed.update()
  assert(Math.abs(first.source.currentCandle[4]-2.02)<1e-12)
  assert.equal(first.source.currentCandle[5],3)
  assert.equal(first.source.provider,'Monad RPC')
  head=11n
  events.push({blockNumber:11n,logIndex:0,args:{sqrtPriceX96:sqrt(1.99),amount0:1n,amount1:-2000000n}})
  const next=await feed.update()
  assert.equal(next.source.currentCandle[5],5)
  assert(Math.abs(next.source.currentCandle[2]-2.02)<1e-12)
  assert(Math.abs(next.source.currentCandle[3]-1.99)<1e-12)
  assert.notEqual(next.inputHash,first.inputHash)
  assert.equal((await feed.update()).source.currentCandle[5],5)
  assert(Math.abs(poolPrice(sqrt(2),true,18,6)-2)<1e-12)
})
