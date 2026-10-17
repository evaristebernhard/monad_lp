import {formatUnits,parseUnits,parseAbi,zeroAddress} from 'viem'

export const sessionAbi=parseAbi([
  'function sessions(address) view returns(uint64 id,uint64 startedAt,uint64 lastExecution,uint8 nextRound,bool active)',
  'function start()', 'function cancel()',
  'function previewAction(address,int32[12]) view returns(uint8 predicted,uint8 action,uint8 reason)',
  'event ExecutionRule(address indexed wallet,uint64 indexed sessionId,uint8 indexed round,uint8 reason)',
  'event MarketInput(address indexed wallet,uint64 indexed sessionId,uint8 indexed round,uint64 candleEnd,bytes32 sourceHash,int32[12] features,uint8 predicted)',
  'event SessionStarted(address indexed wallet,uint64 indexed sessionId,uint64 startedAt,uint64 expiresAt)',
  'event Decision(address indexed wallet,uint64 indexed sessionId,uint8 indexed round,uint8 action,bytes32 inputHash,uint256 amountIn,uint256 amountOut,int256[3] scores)'
])
const marketAbi=parseAbi([
  'function factory() view returns(address)',
  'function getPair(address,address) view returns(address)',
  'function token0() view returns(address)',
  'function token1() view returns(address)',
  'function getReserves() view returns(uint112,uint112,uint32)',
  'function getAmountsOut(uint256,address[]) view returns(uint256[])'
])

// All values in a snapshot refer to the same RPC block, including wallet reads.
export async function readMarket(client,config){
  if(await client.getChainId()!==config.chainId)throw Error('RPC 网络与合约配置不一致')
  const block=await client.getBlock({blockTag:'latest'})
  const blockNumber=block.number
  if(blockNumber===null)throw Error('RPC 未返回已出块区块')
  const read=(address,functionName,args=[])=>client.readContract({address,abi:marketAbi,functionName,args,blockNumber})
  const [code,factory]=await Promise.all([
    client.getBytecode({address:config.contract,blockNumber}),read(config.router,'factory')
  ])
  if(!code||code==='0x')throw Error('RPC 未找到会话合约')
  const pair=await read(factory,'getPair',[config.usdc,config.wmon])
  if(pair===zeroAddress)throw Error('RPC 未找到 USDC/WMON 池')
  const [token0,token1,reserves,buy,sell]=await Promise.all([
    read(pair,'token0'),read(pair,'token1'),read(pair,'getReserves'),
    read(config.router,'getAmountsOut',[parseUnits(config.buyUSDC||'0.4',6),[config.usdc,config.wmon]]),
    read(config.router,'getAmountsOut',[parseUnits(config.sellWMON||'0.1',18),[config.wmon,config.usdc]])
  ])
  const stableFirst=token0.toLowerCase()===config.usdc.toLowerCase()
  if((stableFirst?token1:token0).toLowerCase()!==config.wmon.toLowerCase()||
      (stableFirst?token0:token1).toLowerCase()!==config.usdc.toLowerCase())throw Error('池子资产与配置不一致')
  if(!reserves[0]||!reserves[1]||buy.length!==2||sell.length!==2||!buy[1]||!sell[1])throw Error('池子储备或兑换报价为空')
  return {block:String(blockNumber),timestamp:Number(block.timestamp),pair,
    reserveUSDC:formatUnits(reserves[stableFirst?0:1],6),
    reserveWMON:formatUnits(reserves[stableFirst?1:0],18),
    buyOut:formatUnits(buy[1],18),sellOut:formatUnits(sell[1],6),checkedAt:Date.now()}
}

async function blockAtTime(client,time,latest){
  let low=0n,high=latest.number
  while(low<high){
    const mid=(low+high)/2n
    const block=await client.getBlock({blockNumber:mid})
    if(Number(block.timestamp)<time)low=mid+1n
    else high=mid
  }
  return low
}

// Cache only complete scans. Failed RPC reads must never advance the cursor.
export function createDecisionReader(client){
  const cache=new Map()
  return async function activity(config,address,session){
    const key=[config.chainId,config.contract,address,session.id].join(':').toLowerCase()
    const latest=await client.getBlock({blockTag:'latest'})
    let entry=cache.get(key)
    if(!entry){
      const from=await blockAtTime(client,session.startedAt,latest)
      const end=Number(latest.timestamp)>session.startedAt+config.sessionTTL
        ?await blockAtTime(client,session.startedAt+config.sessionTTL+1,latest):null
      entry={from,end,logs:[]}
    }
    if(entry.end===null&&Number(latest.timestamp)>session.startedAt+config.sessionTTL){
      entry={...entry,end:await blockAtTime(client,session.startedAt+config.sessionTTL+1,latest)}
    }
    const to=entry.end!==null&&entry.end<latest.number?entry.end:latest.number
    const logs=[]
    for(let fromBlock=entry.from;fromBlock<=to;fromBlock+=70n){
      const toBlock=fromBlock+69n>to?to:fromBlock+69n
      logs.push(...await client.getContractEvents({address:config.contract,abi:sessionAbi,eventName:'Decision',
        args:{wallet:address,sessionId:BigInt(session.id)},fromBlock,toBlock,strict:true}))
      if(config.mode==='live-market-session')logs.push(...await client.getContractEvents({
        address:config.contract,abi:sessionAbi,eventName:'MarketInput',
        args:{wallet:address,sessionId:BigInt(session.id)},fromBlock,toBlock,strict:true}))
      if(config.executionRules?.enabled)logs.push(...await client.getContractEvents({
        address:config.contract,abi:sessionAbi,eventName:'ExecutionRule',
        args:{wallet:address,sessionId:BigInt(session.id)},fromBlock,toBlock,strict:true}))
    }
    // Re-read a small overlap so the most recent blocks can be reorganized.
    const merged=[...entry.logs.filter(l=>l.blockNumber<entry.from),...logs].filter(l=>!l.removed)
    cache.set(key,{from:to>=2n?to-2n:0n,end:entry.end,logs:merged})
    return merged.filter(log=>!['MarketInput','ExecutionRule'].includes(log.eventName)&&Number(log.args.sessionId)===session.id).map(log=>{
      const input=merged.find(l=>l.eventName==='MarketInput'&&l.args.round===log.args.round&&l.args.sessionId===log.args.sessionId)
      const rule=merged.find(l=>l.eventName==='ExecutionRule'&&l.args.round===log.args.round&&l.args.sessionId===log.args.sessionId)
      return {
      round:Number(log.args.round),action:Number(log.args.action),hash:log.transactionHash,
      predicted:input?Number(input.args.predicted):Number(log.args.action),
      reason:rule?Number(rule.args.reason):0,
      candleEnd:input?Number(input.args.candleEnd):null,sourceHash:input?.args.sourceHash,
      block:String(log.blockNumber),inputHash:log.args.inputHash,scores:log.args.scores.map(String),
      amountIn:String(log.args.amountIn),amountOut:String(log.args.amountOut)
    }}).sort((a,b)=>a.round-b.round)
  }
}
