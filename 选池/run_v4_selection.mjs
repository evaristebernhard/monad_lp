// Read-only selection among verified, hook-free MON/USDC V4 PoolKeys.
// This is a K=1 exact-quote baseline, NOT the V2 DCAR certificate algorithm.
import fs from 'node:fs'
import { createPublicClient, http, parseAbi, encodeAbiParameters, parseAbiParameters, keccak256, parseUnits, formatUnits } from '../frontend/node_modules/viem/_esm/index.js'
const ZERO='0x0000000000000000000000000000000000000000'
const USDC='0x754704bc059f8c67012fed69bc8a327a5aafb603'
const QUOTER='0xa222dd357a9076d1091ed6aa2e16c9742dd26891'
const abi=parseAbi(['function quoteExactInputSingle(((address currency0,address currency1,uint24 fee,int24 tickSpacing,address hooks) poolKey,bool zeroForOne,uint128 exactAmount,bytes hookData) params) returns (uint256 amountOut,uint256 gasEstimate)'])
const client=createPublicClient({transport:http(process.env.SELECTION_RPC_URL||'http://127.0.0.1:8545/rpc/monad',{timeout:18000,retryCount:0}),batch:{multicall:false}})
if(await client.getChainId()!==143)throw new Error('This collector requires Monad mainnet read-only RPC')
const catalog=JSON.parse(fs.readFileSync(new URL('./results/mon-usdc-v4-catalog.json',import.meta.url),'utf8'))
const accepted=[],rejected=[]
for(const pool of catalog.pools){
 const fee=Number(pool.rpc_decoded_state.lp_fee_raw),tickSpacing=Number(pool.tick_spacing)
 if(!Number.isInteger(fee)||fee<0||fee>=8388608||!Number.isInteger(tickSpacing)||tickSpacing<=0){rejected.push({pool:pool.pool_address,reason:'unverified static PoolKey fee or tick spacing'});continue}
 const key={currency0:ZERO,currency1:USDC,fee,tickSpacing,hooks:ZERO}
 const id=keccak256(encodeAbiParameters(parseAbiParameters('address,address,uint24,int24,address'),[ZERO,USDC,fee,tickSpacing,ZERO]))
 if(id.toLowerCase()!==pool.pool_address.toLowerCase()){rejected.push({pool:pool.pool_address,reason:'zero-hook PoolKey hash mismatch; key/hook needs independent discovery'});continue}
 accepted.push({id,key})
}
const block=await client.getBlock()
const scenarios=[]
for(const [direction,amounts] of [['MON_TO_USDC',['1','100','1000']],['USDC_TO_MON',['1','10','100']]]){
 const zeroForOne=direction==='MON_TO_USDC',inDecimals=zeroForOne?18:6,outDecimals=zeroForOne?6:18
 for(const amount of amounts){
  const amountIn=parseUnits(amount,inDecimals),quotes=[]
  // Small concurrency and explicit block pinning preserve provider capacity and comparability.
  for(let at=0;at<accepted.length;at+=2){
   const batch=await Promise.all(accepted.slice(at,at+2).map(async pool=>{
    try{
     const {result}=await client.simulateContract({address:QUOTER,abi,functionName:'quoteExactInputSingle',args:[{poolKey:pool.key,zeroForOne,exactAmount:amountIn,hookData:'0x'}],blockNumber:block.number})
     if(result[0]<=0n)throw new Error('zero output')
     return {pool:pool.id,poolKey:pool.key,status:'quoted',amountOutRaw:String(result[0]),amountOut:formatUnits(result[0],outDecimals),quoterGasEstimate:String(result[1])}
    }catch(error){return {pool:pool.id,poolKey:pool.key,status:'unavailable',reason:error.shortMessage||error.message||'quote failed'}}
   }));quotes.push(...batch)
  }
  const valid=quotes.filter(q=>q.status==='quoted')
  valid.sort((a,b)=>BigInt(a.amountOutRaw)>BigInt(b.amountOutRaw)?-1:BigInt(a.amountOutRaw)<BigInt(b.amountOutRaw)?1:a.pool.localeCompare(b.pool))
  scenarios.push({direction,inputToken:zeroForOne?ZERO:USDC,outputToken:zeroForOne?USDC:ZERO,amountIn:amount,amountInRaw:String(amountIn),outputDecimals:outDecimals,quotes,selected:valid[0]||null})
 }
}
const header=await client.getBlock({blockNumber:block.number})
if(header.hash!==block.hash)throw new Error('Pinned block changed; discard selection')
const report={algorithm:'K=1 maximum gross exact-input quote, deterministic pool-ID tie break',source:'Monad mainnet Quoter read-only eth_call',sourceChainId:143,sourceBlock:String(block.number),sourceBlockHash:block.hash,sourceTimestamp:String(block.timestamp),capturedAt:new Date().toISOString(),quoter:QUOTER,catalogCandidates:catalog.pools.length,verifiedPoolKeys:accepted.length,rejected,scenarios,limitations:['No split routing or DCAR V2 optimality certificate','Gas estimates belong to Quoter execution, not complete user transaction costs','Best gross output only among verified hook-free supplied candidates','No swaps, approvals, signing or asset movement','Mainnet snapshots replayed on testnet do not represent testnet liquidity']}
fs.writeFileSync(new URL('./results/live-v4-selection.json',import.meta.url),JSON.stringify(report,null,2)+'\n')
console.log(JSON.stringify({sourceBlock:report.sourceBlock,verifiedPoolKeys:accepted.length,rejected:rejected.length,results:scenarios.map(s=>({direction:s.direction,amountIn:s.amountIn,pool:s.selected?.pool,output:s.selected?.amountOut,available:s.quotes.filter(q=>q.status==='quoted').length}))},null,2))
if(scenarios.some(s=>!s.selected))process.exitCode=2
