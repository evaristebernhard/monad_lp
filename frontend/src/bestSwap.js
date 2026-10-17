import {encodeAbiParameters,encodeFunctionData,encodePacked,keccak256,parseAbi,parseAbiParameters,parseUnits} from 'viem'
export const CHAIN_ID=143
export const ZERO='0x0000000000000000000000000000000000000000'
export const USDC='0x754704bc059f8c67012fed69bc8a327a5aafb603'
export const WMON='0x3bd359c1119da7da1d913d1c4d2b7c461115433a'
export const UNI_ROUTER='0x0d97dc33264bfc1c226207428a79b26757fb9dc3'
export const PANCAKE_ROUTER='0x23682a588cf2601aca977df200938634c9f7d552'
const SELF='0x0000000000000000000000000000000000000002'
const KEY='(address currency0,address currency1,uint24 fee,int24 tickSpacing,address hooks)'
export const EXECUTE_ABI=parseAbi(['function execute(bytes commands,bytes[] inputs,uint256 deadline) payable'])
const V3_QUOTE=parseAbi(['function quoteExactInputSingle((address tokenIn,address tokenOut,uint256 amountIn,uint24 fee,uint160 sqrtPriceLimitX96) params) returns(uint256 amountOut,uint160 sqrtPriceX96After,uint32 initializedTicksCrossed,uint256 gasEstimate)'])
const V4_QUOTE=parseAbi([`function quoteExactInputSingle((${KEY} poolKey,bool zeroForOne,uint128 exactAmount,bytes hookData) params) returns(uint256 amountOut,uint256 gasEstimate)`])
const FACTORY_ABI=parseAbi(['function factory() view returns(address)','function getPool(address,address,uint24) view returns(address)','function getPair(address,address) view returns(address)','function token0() view returns(address)','function token1() view returns(address)','function fee() view returns(uint24)','function getReserves() view returns(uint112,uint112,uint32)'])
export function selectBestFrame(frame,catalog,amount,now=Date.now(),{protocol}={}){
 if(!/^(?:0|[1-9]\d*)(?:\.\d{1,18})?$/.test(amount))throw Error('请输入最多 18 位小数的 MON 数量')
 const raw=parseUnits(amount,18),s=frame.scenarios?.[0]
 if(raw<=0n||raw>=2n**128n||frame.sourceChainId!==143||frame.fresh!==true||!frame.sourceBlockHash||!/^\d+$/.test(frame.sourceBlock)||now-Number(frame.sourceTimestamp)*1000>30000||Number(frame.sourceTimestamp)*1000-now>5000)throw Error('报价过期或来源网络错误')
 if(!s||s.direction!=='MON_TO_USDC'||s.inputToken.toLowerCase()!==ZERO||s.outputToken.toLowerCase()!==USDC||BigInt(s.amountInRaw)!==raw)throw Error('报价币对或输入不匹配；仅支持 MON → USDC')
 if(s.quotes.length!==catalog.length||new Set(s.quotes.map(q=>q.pool.toLowerCase())).size!==catalog.length)throw Error('报价候选集合不完整')
 const ids=new Map(catalog.map(q=>[q.pool.toLowerCase(),q]))
 for(const q of s.quotes)if(!ids.has(q.pool.toLowerCase())||!/^\d+$/.test(q.amountOutRaw))throw Error('存在未知池或无效输出')
 const ranked=s.quotes.filter(q=>q.status==='quoted'&&BigInt(q.amountOutRaw)>0n).sort((a,b)=>BigInt(a.amountOutRaw)===BigInt(b.amountOutRaw)?(BigInt(a.pool)<BigInt(b.pool)?-1:1):BigInt(a.amountOutRaw)>BigInt(b.amountOutRaw)?-1:1)
 const best=ranked[0]
 if(!best||frame.selected?.pool?.toLowerCase()!==best.pool.toLowerCase()||String(frame.selected.amountOutRaw)!==best.amountOutRaw)throw Error('接口选择与整数排序不一致')
 const scoped=protocol?ranked.filter(q=>ids.get(q.pool.toLowerCase()).protocol===protocol):ranked
 if(!scoped.length)throw Error('指定协议没有有效报价')
 const chosen=scoped[0]
 return {route:ids.get(chosen.pool.toLowerCase()),amountIn:raw,amountOut:BigInt(chosen.amountOutRaw),frame,available:ranked.length,scope:protocol||'all_candidates'}
}
export function buildBestBuy({route,amountIn,amountOut,wallet,deadline,slippageBps=50}){
 if(!Number.isInteger(slippageBps)||slippageBps<1||slippageBps>100)throw Error('滑点必须为 1–100 bps')
 if(amountIn<=0n||amountIn>=2n**128n||amountOut<=0n||amountOut>=2n**128n)throw Error('无效兑换数量')
 const minimum=amountOut*BigInt(10000-slippageBps)/10000n
 if(minimum<=0n)throw Error('最低输出不足一个 USDC 最小单位')
 const encode=(types,values)=>encodeAbiParameters(parseAbiParameters(types),values)
 const refund=encode('address,address,uint256',[ZERO,wallet,0n])
 let to,commands,inputs
 if(route.kind==='v4'){
  const key=route.poolKey
  if(key.currency0.toLowerCase()!==ZERO||key.currency1.toLowerCase()!==USDC||key.hooks.toLowerCase()!==ZERO||keccak256(encode(KEY,[key])).toLowerCase()!==route.pool.toLowerCase())throw Error('V4 PoolKey 身份不匹配')
  const actions=encode('bytes,bytes[]',['0x060c0f',[
   encode(`(${KEY} poolKey,bool zeroForOne,uint128 amountIn,uint128 amountOutMinimum,bytes hookData)`,[{poolKey:key,zeroForOne:true,amountIn,amountOutMinimum:minimum,hookData:'0x'}]),
   encode('address,uint256',[ZERO,amountIn]),encode('address,uint256',[USDC,minimum])]])
  to=UNI_ROUTER;commands='0x1004';inputs=[actions,refund]
 }else{
  if(![route.token0.toLowerCase(),route.token1.toLowerCase()].sort().every((t,i)=>t===[WMON,USDC].sort()[i]))throw Error('路由币对不匹配')
  if(route.kind==='v3'){
   if(!['Uniswap V3','PancakeSwap V3'].includes(route.protocol))throw Error('未知 V3 协议')
   to=route.protocol==='PancakeSwap V3'?PANCAKE_ROUTER:UNI_ROUTER
   commands='0x0b000c04';inputs=[encode('address,uint256',[SELF,amountIn]),encode('address,uint256,uint256,bytes,bool',[wallet,amountIn,minimum,encodePacked(['address','uint24','address'],[WMON,route.fee,USDC]),false])]
  }else if(route.kind==='v2'&&route.protocol==='Uniswap V2'&&route.fee===3000){
   to=UNI_ROUTER;commands='0x0b080c04';inputs=[encode('address,uint256',[SELF,amountIn]),encode('address,uint256,uint256,address[],bool',[wallet,amountIn,minimum,[WMON,USDC],false])]
  }else throw Error('不支持的路由，拒绝退回固定池')
  inputs.push(encode('address,uint256',[wallet,0n]),refund)
 }
 return {account:wallet,to,value:amountIn,data:encodeFunctionData({abi:EXECUTE_ABI,functionName:'execute',args:[commands,inputs,deadline]}),minimum}
}
export async function verifyBestQuote(client,quote){
 if(await client.getChainId()!==143)throw Error('RPC 必须连接 Monad 主网')
 const {route,amountIn,amountOut,frame}=quote,blockNumber=BigInt(frame.sourceBlock)
 const block=await client.getBlock({blockNumber})
 if(block.hash.toLowerCase()!==frame.sourceBlockHash.toLowerCase())throw Error('报价区块哈希不匹配')
 let output
 if(route.kind==='v4'){
  const {result}=await client.simulateContract({address:'0xa222dd357a9076d1091ed6aa2e16c9742dd26891',abi:V4_QUOTE,functionName:'quoteExactInputSingle',args:[{poolKey:route.poolKey,zeroForOne:true,exactAmount:amountIn,hookData:'0x'}],blockNumber});output=result[0]
 }else{
  const read=(address,functionName,args=[])=>client.readContract({address,abi:FACTORY_ABI,functionName,args,blockNumber})
  const [factory,token0,token1]=await Promise.all([read(route.address,'factory'),read(route.address,'token0'),read(route.address,'token1')])
  if(factory.toLowerCase()!==route.factory.toLowerCase()||token0.toLowerCase()!==route.token0.toLowerCase()||token1.toLowerCase()!==route.token1.toLowerCase())throw Error('池身份核验失败')
  const pool=await read(factory,route.kind==='v3'?'getPool':'getPair',route.kind==='v3'?[WMON,USDC,route.fee]:[WMON,USDC])
  if(pool.toLowerCase()!==route.address.toLowerCase())throw Error('工厂未确认候选池')
  if(route.kind==='v3'){
   const [fee,quoterFactory]=await Promise.all([read(route.address,'fee'),read(route.quoter,'factory')])
   if(Number(fee)!==route.fee||quoterFactory.toLowerCase()!==factory.toLowerCase())throw Error('费率或 Quoter 工厂不匹配')
   const {result}=await client.simulateContract({address:route.quoter,abi:V3_QUOTE,functionName:'quoteExactInputSingle',args:[{tokenIn:WMON,tokenOut:USDC,amountIn,fee:route.fee,sqrtPriceLimitX96:0n}],blockNumber});output=result[0]
  }else{
   const [r0,r1]=await read(route.address,'getReserves'),[ri,ro]=token0.toLowerCase()===WMON?[r0,r1]:[r1,r0],adjusted=amountIn*997000n
   output=adjusted*ro/(ri*1000000n+adjusted)
  }
 }
 if(output!==amountOut)throw Error('最优候选的链上报价与 API 输出不一致')
 return block
}
