import fs from 'node:fs'
import {createPublicClient,http,parseAbi,parseAbiParameters,encodeAbiParameters,encodeFunctionData,encodePacked,keccak256,parseUnits,formatUnits,isAddress} from 'viem'
import {monad} from 'viem/chains'
export const pools=JSON.parse(fs.readFileSync(new URL('./pools.json',import.meta.url))).pools
export const ZERO='0x0000000000000000000000000000000000000000', WMON='0x3bd359c1119da7da1d913d1c4d2b7c461115433a'
export const PERMIT2='0x000000000022d473030f116ddee9f6b43ac78ba3'
export const UNI='0x0d97dc33264bfc1c226207428a79b26757fb9dc3', PCS='0x23682a588cf2601aca977df200938634c9f7d552'
const KEY='(address currency0,address currency1,uint24 fee,int24 tickSpacing,address hooks)'
export const erc=parseAbi(['function decimals() view returns(uint8)','function balanceOf(address) view returns(uint256)','function allowance(address,address) view returns(uint256)','function approve(address,uint256) returns(bool)','function deposit() payable'])
export const permit=parseAbi(['function allowance(address,address,address) view returns(uint160,uint48,uint48)','function approve(address,address,uint160,uint48)'])
const poolAbi=parseAbi(['function token0() view returns(address)','function token1() view returns(address)','function fee() view returns(uint24)','function factory() view returns(address)','function getPool(address,address,uint24) view returns(address)','function getPair(address,address) view returns(address)','function getReserves() view returns(uint112,uint112,uint32)'])
const v3=parseAbi(['function quoteExactInputSingle((address tokenIn,address tokenOut,uint256 amountIn,uint24 fee,uint160 sqrtPriceLimitX96) params) returns(uint256,uint160,uint32,uint256)'])
const v4=parseAbi([`function quoteExactInputSingle((${KEY} poolKey,bool zeroForOne,uint128 exactAmount,bytes hookData) params) returns(uint256,uint256)`])
const execute=parseAbi(['function execute(bytes,bytes[],uint256) payable'])
const enc=(t,v)=>encodeAbiParameters(parseAbiParameters(t),v)
export const tokens=Object.fromEntries(pools.flatMap(p=>['a','b'].map(s=>[p[`token_${s}_symbol`],p[`token_${s}_address`]])))
export const targets=Object.keys(tokens).filter(t=>t!=='MON')
export function token(name){const symbol=Object.keys(tokens).find(t=>t.toLowerCase()===String(name).toLowerCase()||tokens[t]===String(name).toLowerCase());if(!symbol)throw Error('Unknown token');return {symbol,address:tokens[symbol]}}
export function amountMon(text='0.1'){if(!/^(0|[1-9]\d*)(\.\d{1,18})?$/.test(text))throw Error('Invalid MON amount');const n=parseUnits(text,18);if(n<=0n||n>parseUnits('0.1',18))throw Error('Example limit: 0 < amount <= 0.1 MON per token');return n}
export function minimum(out,bps=50){if(!Number.isInteger(bps)||bps<1||bps>100)throw Error('slippageBps must be 1..100');const n=out*BigInt(10000-bps)/10000n;if(n<=0n)throw Error('Output below minimum token unit');return n}
export const json=(v)=>JSON.stringify(v,(_,x)=>typeof x==='bigint'?x.toString():x,2)
export function client(){return createPublicClient({chain:monad,transport:http(process.env.MONAD_RPC_URL||'https://rpc.monad.xyz',{retryCount:0,timeout:20000})})}
function edge(pair,input,output){const p=pools.find(p=>p.pair===pair);if(!p)throw Error('Missing pool');return {pool:p.pool_address,protocol:p.protocol,kind:p.pool_type,input:tokens[input],output:tokens[output],pair}}
// Fixed, reviewable routes through the supplied catalog, not a global best-price claim.
export function route(name){const {symbol}=token(name);switch(symbol){
 case 'MON':return []
 case 'WMON':return [{kind:'wrap',input:ZERO,output:WMON}]
 case 'USDC':case 'WETH':case 'WBTC':case 'AUSD':return [edge('MON/'+symbol,'MON',symbol)]
 case 'LVMON':return [edge('WMON/LVMON','MON','LVMON')]
 case 'shMON':return [edge('shMON/WMON','MON','shMON')]
 case 'APR':return [edge('APR/WMON','MON','APR')]
 case 'SAVVA.M':return [edge('SAVVA.M/WMON','MON','SAVVA.M')]
 case 'cbBTC':return [edge('WMON/cbBTC','MON','cbBTC')]
 case 'USDT0':return [edge('MON/USDC','MON','USDC'),edge('USDC/USDT0','USDC','USDT0')]
 case 'XAUt0':return [...route('USDT0'),edge('XAUt0/USDT0','USDT0','XAUt0')]
 default:throw Error('Missing route')
}}
export async function metadata(c,e,blockNumber){
 if(e.kind==='wrap')return e
 const p=pools.find(p=>p.pool_address===e.pool)
 if(e.kind==='uniswap_v4'){
  const tickSpacing=['MON/USDC','USDC/WETH','USDC/cbBTC'].includes(p.pair)?10:1
  const fee=p.pair==='WBTC/cbBTC'?100:p.pair==='USDC/cbBTC'?500:p.pair==='USDC/USDT0'?75:p.pair==='AUSD/USDC'?50:500
  const poolKey={currency0:p.token_a_address,currency1:p.token_b_address,fee,tickSpacing,hooks:ZERO}
  if(keccak256(enc(KEY,[poolKey]))!==e.pool)throw Error('PoolKey hash mismatch')
  return {...e,poolKey,router:UNI,quoter:'0xa222dd357a9076d1091ed6aa2e16c9742dd26891'}
 }
 const read=(address,functionName,args=[])=>c.readContract({address,abi:poolAbi,functionName,args,blockNumber})
 const [t0,t1,factory]=await Promise.all([read(e.pool,'token0'),read(e.pool,'token1'),read(e.pool,'factory')]);const input=e.input===ZERO?WMON:e.input
 if(![t0,t1].map(t=>t.toLowerCase()).sort().every((t,i)=>t===[input,e.output].sort()[i]))throw Error('Pool tokens mismatch')
 const pancake=e.protocol==='PancakeSwap V3',isV2=e.kind==='constant_product_v2'
 const expected=isV2?'0x182a927119d56008d921126764bf884221b10f59':pancake?'0x0bfbcf9fa4f9c56b0f40a671ad40e0805a091865':'0x204faca1764b154221e35c0d20abb3c525710498'
 if(factory.toLowerCase()!==expected)throw Error('Untrusted factory '+factory)
 const fee=isV2?3000:Number(await read(e.pool,'fee'))
 const canonical=await read(factory,isV2?'getPair':'getPool',isV2?[input,e.output]:[input,e.output,fee]);if(canonical.toLowerCase()!==e.pool)throw Error('Factory pool mismatch')
 const quoter=pancake?'0xb048bbc1ee6b733fffcfb9e9cef7375518e25997':'0x661e93cca42afacb172121ef892830ca3b70f08d'
 if(!isV2&&(await read(quoter,'factory')).toLowerCase()!==factory.toLowerCase())throw Error('Quoter factory mismatch')
 return {...e,token0:t0.toLowerCase(),fee,router:pancake?PCS:UNI,quoter}
}
export async function quoteLeg(c,e,amountIn,blockNumber){
 if(e.kind==='wrap')return amountIn
 if(e.kind==='uniswap_v4'){const {result}=await c.simulateContract({address:e.quoter,abi:v4,functionName:'quoteExactInputSingle',args:[{poolKey:e.poolKey,zeroForOne:e.input===e.poolKey.currency0,exactAmount:amountIn,hookData:'0x'}],blockNumber});return result[0]}
 const input=e.input===ZERO?WMON:e.input
 if(e.kind==='constant_product_v2'){const [r0,r1]=await c.readContract({address:e.pool,abi:poolAbi,functionName:'getReserves',blockNumber});const [ri,ro]=e.token0===input?[r0,r1]:[r1,r0];const a=amountIn*997n;return a*ro/(ri*1000n+a)}
 const {result}=await c.simulateContract({address:e.quoter,abi:v3,functionName:'quoteExactInputSingle',args:[{tokenIn:input,tokenOut:e.output,amountIn,fee:e.fee,sqrtPriceLimitX96:0n}],blockNumber});return result[0]
}
export function build(e,amountIn,out,wallet,deadline,bps=50){
 if(!isAddress(wallet)||wallet===ZERO)throw Error('Invalid wallet')
 const min=minimum(out,bps)
 if(e.kind==='wrap')return {account:wallet,to:WMON,data:encodeFunctionData({abi:erc,functionName:'deposit'}),value:amountIn,minimum:amountIn}
 let commands='',inputs=[]
 if(e.kind==='uniswap_v4'){
  commands='0x1004';inputs=[enc('bytes,bytes[]',['0x060c0f',[
   enc(`(${KEY} poolKey,bool zeroForOne,uint128 amountIn,uint128 amountOutMinimum,bytes hookData)`,[{poolKey:e.poolKey,zeroForOne:e.input===e.poolKey.currency0,amountIn,amountOutMinimum:min,hookData:'0x'}]),enc('address,uint256',[e.input,amountIn]),enc('address,uint256',[e.output,min])]]),enc('address,address,uint256',[e.input,wallet,0n])]
 }else{
  const native=e.input===ZERO,isV2=e.kind==='constant_product_v2',input=native?WMON:e.input
  commands='0x'+(native?'0b':'')+(isV2?'08':'00')+(native?'0c04':'')
  if(native)inputs.push(enc('address,uint256',['0x0000000000000000000000000000000000000002',amountIn]))
  inputs.push(isV2?enc('address,uint256,uint256,address[],bool',[wallet,amountIn,min,[input,e.output],!native]):enc('address,uint256,uint256,bytes,bool',[wallet,amountIn,min,encodePacked(['address','uint24','address'],[input,e.fee,e.output]),!native]))
  if(native)inputs.push(enc('address,uint256',[wallet,0n]),enc('address,address,uint256',[ZERO,wallet,0n]))
 }
 return {account:wallet,to:e.router,value:e.input===ZERO?amountIn:0n,data:encodeFunctionData({abi:execute,functionName:'execute',args:[commands,inputs,deadline]}),minimum:min}
}
export async function quote(c,name,amount='0.1',bps=50){
 const target=token(name);if(target.symbol==='MON')return {token:'MON',status:'native',message:'MON is the input asset'}
 const initial=amountMon(amount);minimum(initial,bps)
 if(await c.getChainId()!==143)throw Error('Expected Monad mainnet 143')
 const block=await c.getBlock();if(Math.abs(Date.now()/1000-Number(block.timestamp))>30)throw Error('Stale RPC block')
 const decimals=Number(await c.readContract({address:target.address,abi:erc,functionName:'decimals',blockNumber:block.number}));let current=initial;const legs=[]
 for(const r of route(name)){const e=await metadata(c,r,block.number),out=await quoteLeg(c,e,current,block.number);legs.push({...e,amountIn:current,amountOut:out,minimumOut:minimum(out,bps)});current=out}
 if((await c.getBlock({blockNumber:block.number})).hash!==block.hash)throw Error('Quote block changed')
 return {chainId:143,token:target.symbol,address:target.address,decimals,inputMON:amount,amountOutRaw:current,amountOut:formatUnits(current,decimals),effectiveMONPerToken:Number(amount)/Number(formatUnits(current,decimals)),slippageBps:bps,sourceBlock:block.number,sourceBlockHash:block.hash,sourceTimestamp:block.timestamp,expiresAt:Number(block.timestamp)+30,scope:'fixed catalog route; gas excluded; multi-transaction routes are not atomic',legs}
}
