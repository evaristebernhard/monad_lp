import {createPublicClient,http,formatUnits,keccak256,encodeAbiParameters,parseAbiParameters,parseAbi,encodeFunctionData,decodeFunctionResult} from 'viem'
import {subscribeJson} from './streamJson.js'
import {monad} from 'viem/chains'
import {QUOTER,QUOTER_ABI,parseAmount,ZERO,USDC} from './monadTrade.js'

// Every candidate uses the same block, direction and exact input. Failed pools
// remain in the universe with zero output, as required by the selector ABI.
const WMON='0x3bd359c1119da7da1d913d1c4d2b7c461115433a'
const V3_ABI=parseAbi(['function quoteExactInputSingle((address tokenIn,address tokenOut,uint256 amountIn,uint24 fee,uint160 sqrtPriceLimitX96) params) returns(uint256 amountOut,uint160 sqrtPriceX96After,uint32 initializedTicksCrossed,uint256 gasEstimate)'])
const V2_ABI=parseAbi(['function getReserves() view returns(uint112 reserve0,uint112 reserve1,uint32 blockTimestampLast)'])
const MULTI_ABI=parseAbi(['function aggregate3((address target,bool allowFailure,bytes callData)[] calls) payable returns((bool success,bytes returnData)[] results)'])
function quoteContracts(candidates,amountIn,zeroForOne){
 return candidates.map(q=>{
  if(q.kind==='v3')return {address:q.quoter,abi:V3_ABI,functionName:'quoteExactInputSingle',args:[{tokenIn:zeroForOne?WMON:USDC,tokenOut:zeroForOne?USDC:WMON,amountIn,fee:q.fee,sqrtPriceLimitX96:0n}]}
  if(q.kind==='v2')return {address:q.address,abi:V2_ABI,functionName:'getReserves'}
  const key=q.poolKey,id=keccak256(encodeAbiParameters(parseAbiParameters('address,address,uint24,int24,address'),[key.currency0,key.currency1,key.fee,key.tickSpacing,key.hooks]))
  if(id.toLowerCase()!==q.pool.toLowerCase()||key.currency0.toLowerCase()!==ZERO||key.currency1.toLowerCase()!==USDC)throw Error('候选池标识或币对不匹配')
  return {address:QUOTER,abi:QUOTER_ABI,functionName:'quoteExactInputSingle',args:[{poolKey:key,zeroForOne,exactAmount:amountIn,hookData:'0x'}]}
 })
}
export async function fetchSelectionQuotes(candidates,amount,payToken,rpc,options={}){
 const startedAt=Date.now(),amountIn=parseAmount(amount,payToken),zeroForOne=payToken==='MON',outputDecimals=zeroForOne?6:18
 rpc??=createPublicClient({cacheTime:0,transport:http('/api/monad/trade-rpc',{retryCount:0,timeout:6000,fetchOptions:{signal:options.signal}})})
 if(await rpc.getChainId()!==143)throw Error('报价 RPC 必须连接 Monad 主网')
 const block=await rpc.getBlock(),quotes=[]
 const calls=quoteContracts(candidates,amountIn,zeroForOne)
 options.signal?.throwIfAborted()
 let observations
 if(rpc.multicall&&options.multicall!==false){
  // One aggregate eth_call instead of one network request per pool. No funds move.
  observations=await rpc.multicall({contracts:calls,blockNumber:block.number,multicallAddress:monad.contracts.multicall3.address,batchSize:0,allowFailure:true})
 }else{
  observations=[]
  for(let at=0;at<calls.length;at+=6){
   options.signal?.throwIfAborted()
   observations.push(...await Promise.all(calls.slice(at,at+6).map(async c=>{
    try{return {status:'success',result:c.functionName==='getReserves'?await rpc.readContract({...c,blockNumber:block.number}):(await rpc.simulateContract({...c,blockNumber:block.number})).result}}
    catch{return {status:'failure'}}
   })))
  }
 }
 // Some pools exceed an aggregate provider's per-call budget. Retry only
 // failed subcalls separately at the identical block before excluding them.
 const failed=observations.map((r,i)=>r.status==='success'?-1:i).filter(i=>i>=0)
 if(rpc.multicall&&options.multicall!==false&&failed.length&&failed.length<candidates.length){
  for(let at=0;at<failed.length;at+=6){
   options.signal?.throwIfAborted()
   await Promise.all(failed.slice(at,at+6).map(async i=>{
    const c=calls[i]
    try{observations[i]={status:'success',result:c.functionName==='getReserves'?await rpc.readContract({...c,blockNumber:block.number}):(await rpc.simulateContract({...c,blockNumber:block.number})).result}}catch{options.signal?.throwIfAborted()}
   }))
  }
 }
 options.signal?.throwIfAborted()
 candidates.forEach((q,i)=>{
  const observation=observations[i]
  if(observation.status!=='success'){quotes.push({...q,status:'unavailable',amountOutRaw:'0',amountOut:'0',reason:'该池报价失败'});return}
  let out=observation.result[0]
  if(q.kind==='v2'){
   const reserves=observation.result,inIs0=q.token0.toLowerCase()===(zeroForOne?WMON:USDC)
   const reserveIn=reserves[inIs0?0:1],reserveOut=reserves[inIs0?1:0],adjusted=amountIn*(1000000n-BigInt(q.fee))
   out=reserveIn>0n&&reserveOut>0n?adjusted*reserveOut/(reserveIn*1000000n+adjusted):0n
  }
  quotes.push({...q,status:out>0n?'quoted':'zero',amountOutRaw:String(out),amountOut:formatUnits(out,outputDecimals),reason:out>0n?null:'输出不足一个最小单位'})
 })
 options.signal?.throwIfAborted()
 const header=await rpc.getBlock({blockNumber:block.number})
 if(header.hash!==block.hash)throw Error('报价区块发生变化，请重新获取')
 const available=quotes.filter(q=>q.status==='quoted').length
 if(!available)throw Error('所有候选池均无有效报价，请检查金额或稍后重试')
 return {sourceChainId:143,sourceBlock:String(block.number),sourceBlockHash:block.hash,sourceTimestamp:block.timestamp===undefined?null:String(block.timestamp),startedAt:new Date(startedAt).toISOString(),capturedAt:new Date().toISOString(),durationMs:Date.now()-startedAt,available,candidateCount:candidates.length,scenarios:[{direction:zeroForOne?'MON_TO_USDC':'USDC_TO_MON',inputToken:zeroForOne?ZERO:USDC,outputToken:zeroForOne?USDC:ZERO,amountIn:amount,amountInRaw:String(amountIn),outputDecimals,quotes}]}
}

export function selectV4(source){
 if(!source||source.sourceChainId!==143||!source.sourceBlockHash||BigInt(source.sourceBlock)<=0n)throw Error('无有效报价快照')
 const s=source.scenarios[0],seen=new Set()
 if(!s||!s.quotes.length||BigInt(s.amountInRaw)<=0n||!((s.inputToken.toLowerCase()===ZERO&&s.outputToken.toLowerCase()===USDC)||(s.inputToken.toLowerCase()===USDC&&s.outputToken.toLowerCase()===ZERO)))throw Error('无效交易输入或币对')
 const quotes=s.quotes.map(q=>{
  const id=q.pool.toLowerCase();if(seen.has(id))throw Error('重复候选池');seen.add(id)
  const out=q.status==='quoted'?BigInt(q.amountOutRaw):0n
  if(out<0n)throw Error('无效输出')
  return {poolId:q.pool,amountOut:out}
 })
 const ranked=[...quotes].sort((a,b)=>a.amountOut===b.amountOut?(BigInt(a.poolId)<BigInt(b.poolId)?-1:1):a.amountOut>b.amountOut?-1:1)
 if(ranked[0].amountOut===0n)throw Error('没有有效报价')
 const context={sourceChainId:143n,sourceBlock:BigInt(source.sourceBlock),sourceBlockHash:source.sourceBlockHash,tokenIn:s.inputToken,tokenOut:s.outputToken,amountIn:BigInt(s.amountInRaw)}
 const universe=keccak256(encodeAbiParameters(parseAbiParameters('bytes32[]'),[quotes.map(q=>q.poolId)]))
 const hash=keccak256(encodeAbiParameters(parseAbiParameters('bytes32,(uint256 sourceChainId,uint256 sourceBlock,bytes32 sourceBlockHash,address tokenIn,address tokenOut,uint256 amountIn),(bytes32 poolId,uint256 amountOut)[]'),[universe,context,quotes]))
 return {args:[context,quotes],values:[ranked[0].poolId,ranked[0].amountOut,hash],tieCount:ranked.filter(q=>q.amountOut===ranked[0].amountOut).length,runnerUp:ranked.find(q=>q.amountOut>0n&&q.poolId!==ranked[0].poolId)||null}
}

export function quoteStreamRequest(candidates,amount,payToken){
 const amountIn=parseAmount(amount,payToken),contracts=quoteContracts(candidates,amountIn,payToken==='MON')
 return {data:encodeFunctionData({abi:MULTI_ABI,functionName:'aggregate3',args:[contracts.map(c=>({target:c.address,allowFailure:true,callData:encodeFunctionData(c)}))]})}
}
export function decodeQuoteFrame(candidates,amount,payToken,frame){
 if(frame.error)throw Error(frame.error)
 if(frame.chainId!==143)throw Error('报价网络错误')
 const amountIn=parseAmount(amount,payToken),zeroForOne=payToken==='MON',outputDecimals=zeroForOne?6:18
 const contracts=quoteContracts(candidates,amountIn,zeroForOne),results=decodeFunctionResult({abi:MULTI_ABI,functionName:'aggregate3',data:frame.result})
 if(results.length!==candidates.length)throw Error('报价数量不匹配')
 const quotes=candidates.map((q,i)=>{
  try{
   if(!results[i].success)throw Error('unavailable')
   const c=contracts[i],values=decodeFunctionResult({abi:c.abi,functionName:c.functionName,data:results[i].returnData})
   let out=values[0]
   if(q.kind==='v2'){
    const inIs0=q.token0.toLowerCase()===(zeroForOne?WMON:USDC),reserveIn=values[inIs0?0:1],reserveOut=values[inIs0?1:0],adjusted=amountIn*(1000000n-BigInt(q.fee))
    out=reserveIn>0n&&reserveOut>0n?adjusted*reserveOut/(reserveIn*1000000n+adjusted):0n
   }
   return {...q,status:out>0n?'quoted':'zero',amountOutRaw:String(out),amountOut:formatUnits(out,outputDecimals)}
  }catch{return {...q,status:'unavailable',amountOutRaw:'0',amountOut:'0'}}
 })
 const available=quotes.filter(q=>q.status==='quoted').length
 if(!available)throw Error('所有候选池均无有效报价')
 const block=frame.block
 return {sourceChainId:143,sourceBlock:String(BigInt(block.number)),sourceBlockHash:block.hash,sourceTimestamp:String(BigInt(block.timestamp)),startedAt:new Date(frame.startedAtMs).toISOString(),capturedAt:new Date(frame.capturedAtMs).toISOString(),durationMs:frame.durationMs,available,candidateCount:candidates.length,scenarios:[{direction:zeroForOne?'MON_TO_USDC':'USDC_TO_MON',inputToken:zeroForOne?ZERO:USDC,outputToken:zeroForOne?USDC:ZERO,amountIn:amount,amountInRaw:String(amountIn),outputDecimals,quotes}]}
}
export function backendSnapshot(frame,candidates,amount,payToken){
 if(frame.error)throw Error(frame.error)
 const snapshot={...frame,startedAt:frame.startedAt||new Date(frame.startedAtMs).toISOString(),capturedAt:frame.capturedAt||new Date(frame.capturedAtMs).toISOString()}
 const scenario=snapshot.scenarios?.[0],expected=parseAmount(amount,payToken)
 if(!scenario||scenario.amountInRaw!==String(expected)||scenario.direction!==(payToken==='MON'?'MON_TO_USDC':'USDC_TO_MON'))throw Error('后端报价输入不匹配')
 const ids=candidates.map(q=>q.pool.toLowerCase()),actual=scenario.quotes.map(q=>q.pool.toLowerCase())
 if(actual.length!==ids.length||actual.some((id,i)=>id!==ids[i]))throw Error('后端候选集合不匹配')
 const decision=selectV4(snapshot)
 if(frame.selected?.pool?.toLowerCase()!==decision.values[0].toLowerCase()||frame.selected.amountOutRaw!==String(decision.values[1])||frame.replayHash!==decision.values[2])throw Error('后端决策与本地核对不一致')
 return snapshot
}
export async function subscribeSelectionQuotes(candidates,amount,payToken,signal,onSnapshot,onError){
 const body={amount,token_in:payToken,token_out:payToken==='MON'?'USDC':'MON'}
 return subscribeJson('/api/selection/quote-stream',{body,signal,onValue:frame=>onSnapshot(backendSnapshot(frame,candidates,amount,payToken)),onError})
}
