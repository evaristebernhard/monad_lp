import {test} from 'node:test'
import assert from 'node:assert/strict'
import {createDecisionReader,readMarket} from '../src/ai/chainData.js'

const config={chainId:10143,contract:'session',router:'router',usdc:'usdc',wmon:'wmon',sessionTTL:110}
test('an old completed session still retrieves its actual events, with bounded queries and retry after RPC failure',async()=>{
  const calls=[]
  let reject=true
  const event={args:{sessionId:9n,round:0,action:2,inputHash:'0xinput',scores:[1n,2n,3n],amountIn:100000000000000000n,amountOut:321000n},
    blockNumber:101n,transactionHash:'0xconfirmed'}
  const client={
    getBlock:async({blockNumber=10000n})=>({number:blockNumber,timestamp:1800000000n+blockNumber}),
    getContractEvents:async({fromBlock,toBlock,args})=>{
      calls.push([fromBlock,toBlock,args])
      if(reject)throw Error('RPC log request rejected')
      return fromBlock<=101n&&toBlock>=101n?[event]:[]
    }
  }
  const read=createDecisionReader(client)
  const session={id:9,startedAt:1800000100,nextRound:1}
  await assert.rejects(read(config,'wallet',session),/RPC log request rejected/)
  reject=false
  const result=await read(config,'wallet',session)
  assert.equal(result.length,1)
  assert.equal(result[0].hash,'0xconfirmed')
  assert.equal(result[0].action,2)
  assert.equal(result[0].amountOut,'321000')
  assert(calls.every(([from,to,args])=>to-from<100n&&args.sessionId===9n))
  assert.deepEqual(await read(config,'wallet',session),result)
})

test('market reads are block-pinned and respect the pool token order',async()=>{
  const calls=[]
  const client={
    getChainId:async()=>10143,
    getBlock:async()=>({number:77n,timestamp:BigInt(Math.floor(Date.now()/1000))}),
    getBytecode:async()=> '0x6000',
    readContract:async({functionName,blockNumber,args})=>{
      calls.push(blockNumber)
      return {factory:'factory',getPair:'pair',token0:'wmon',token1:'usdc',
        getReserves:[2000000000000000000n,3000000n,0],
        getAmountsOut:[args[0],args[0]===400000n?250000000000000000n:150000n]}[functionName]
    }
  }
  const result=await readMarket(client,config)
  assert(calls.every(n=>n===77n))
  assert.equal(result.reserveUSDC,'3')
  assert.equal(result.reserveWMON,'2')
  assert.equal(result.buyOut,'0.25')
  assert.equal(result.sellOut,'0.15')
  client.getChainId=async()=>143
  await assert.rejects(readMarket(client,config),/网络与合约配置不一致/)
})

test('a budget-limited settlement preserves the actual AI prediction and input provenance',async()=>{
  const common={sessionId:1n,round:0}
  const base={blockNumber:101n,transactionHash:'0xsettled'}
  const client={
    getBlock:async({blockNumber=150n})=>({number:blockNumber,timestamp:1800000000n+blockNumber}),
    getContractEvents:async({eventName,fromBlock,toBlock})=>fromBlock<=101n&&toBlock>=101n?[{
      ...base,eventName,args:eventName==='MarketInput'?{
        ...common,predicted:1,candleEnd:1800000000n,sourceHash:'0xsource'
      }:{...common,action:0,inputHash:'0xinput',scores:[0n,5n,1n],amountIn:0n,amountOut:0n}
    }]:[]
  }
  const [decision]=await createDecisionReader(client)({...config,mode:'live-market-session'},'wallet',{id:1,startedAt:1800000100})
  assert.equal(decision.predicted,1)
  assert.equal(decision.action,0)
  assert.equal(decision.amountIn,'0')
  assert.equal(decision.sourceHash,'0xsource')
})

test('rule-driven BUY is a trade even when the unchanged model predicts HOLD',async()=>{
  const common={sessionId:1n,round:0}
  const client={
    getBlock:async({blockNumber=150n})=>({number:blockNumber,timestamp:1800000000n+blockNumber}),
    getContractEvents:async({eventName,fromBlock,toBlock})=>fromBlock<=101n&&toBlock>=101n?[{
      blockNumber:101n,transactionHash:'0xbuy',eventName,
      args:eventName==='MarketInput'?{...common,predicted:0,candleEnd:1800000000n,sourceHash:'0xsource'}:
        eventName==='ExecutionRule'?{...common,reason:1}:
        {...common,action:1,inputHash:'0xinput',scores:[10n,0n,0n],amountIn:400000n,amountOut:100000000000000000n}
    }]:[]
  }
  const decisions=await createDecisionReader(client)({...config,mode:'live-market-session',executionRules:{enabled:true}},'wallet',{id:1,startedAt:1800000100})
  assert.equal(decisions.length,1,'provenance events must not appear as extra trades')
  assert.equal(decisions[0].predicted,0)
  assert.equal(decisions[0].action,1)
  assert.equal(decisions[0].reason,1)
  assert.equal(decisions[0].amountOut,'100000000000000000')
})
