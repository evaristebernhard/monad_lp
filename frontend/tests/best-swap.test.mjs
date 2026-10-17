import assert from 'node:assert/strict'
import test from 'node:test'
import fs from 'node:fs'
import {decodeFunctionData,decodeAbiParameters,parseAbiParameters} from 'viem'
import {selectBestFrame,buildBestBuy,EXECUTE_ABI,PANCAKE_ROUTER,UNI_ROUTER,USDC,ZERO,WMON} from '../src/bestSwap.js'
const catalog=JSON.parse(fs.readFileSync(new URL('../../选池/results/route-catalog.json',import.meta.url))).candidates
const wallet='0x0000000000000000000000000000000000000123',now=1792214408000
function frame(){const quotes=catalog.map((r,i)=>({...r,status:'quoted',amountOutRaw:String(2000+i)}));return {sourceChainId:143,sourceBlock:'123',sourceBlockHash:'0x'+'01'.repeat(32),sourceTimestamp:String(now/1000),fresh:true,selected:{pool:quotes.at(-1).pool,amountOutRaw:quotes.at(-1).amountOutRaw},scenarios:[{direction:'MON_TO_USDC',inputToken:ZERO,outputToken:USDC,amountInRaw:'100000000000000000',quotes}]}}
test('API output is integer ranked against a local candidate whitelist',()=>{
 const f=frame(),r=selectBestFrame(f,catalog,'0.1',now)
 assert.equal(r.route.pool,catalog.at(-1).pool)
 f.scenarios[0].quotes.at(-1).quoter=wallet
 assert.equal(selectBestFrame(f,catalog,'0.1',now).route.quoter,catalog.at(-1).quoter)
 f.selected.pool=catalog[0].pool
 assert.throws(()=>selectBestFrame(f,catalog,'0.1',now),/排序/)
})
test('reject stale, wrong network, duplicate candidates and precision rounding',()=>{
 const f=frame();assert.throws(()=>selectBestFrame(f,catalog,'0.1',now+31000),/过期/)
 f.sourceChainId=10143;assert.throws(()=>selectBestFrame(f,catalog,'0.1',now),/网络/)
 const duplicate=frame();duplicate.scenarios[0].quotes[0]=duplicate.scenarios[0].quotes[1]
 assert.throws(()=>selectBestFrame(duplicate,catalog,'0.1',now),/集合/)
 assert.throws(()=>selectBestFrame(frame(),catalog,'0.1000000000000000001',now),/18/)
})
test('V3 and V2 wrap MON, enforce output minimum, refund and never pull user ERC20',()=>{
 for(const route of catalog.filter(r=>r.kind!=='v4')){
  const {data,to,value,minimum}=buildBestBuy({route,amountIn:10n**17n,amountOut:2500n,wallet,deadline:123n})
  const {args:[commands,inputs,deadline]}=decodeFunctionData({abi:EXECUTE_ABI,data})
  assert.equal(to,route.protocol==='PancakeSwap V3'?PANCAKE_ROUTER:UNI_ROUTER)
  assert.equal(value,10n**17n);assert.equal(minimum,2487n);assert.equal(deadline,123n)
  assert.equal(commands,route.kind==='v3'?'0x0b000c04':'0x0b080c04')
  const decoded=decodeAbiParameters(parseAbiParameters(`address,uint256,uint256,${route.kind==='v3'?'bytes':'address[]'},bool`),inputs[1])
  assert.equal(decoded[0].toLowerCase(),wallet);assert.equal(decoded[2],minimum);assert.equal(decoded[4],false)
  if(route.kind==='v2')assert.deepEqual(decoded[3].map(v=>v.toLowerCase()),[WMON,USDC])
 }
})
test('V4 identities are verified and no partial-revert flag is enabled',()=>{
 const route=catalog.find(r=>r.kind==='v4'),request={route,amountIn:10n**17n,amountOut:2500n,wallet,deadline:123n}
 const {data}=buildBestBuy(request)
 assert.equal(decodeFunctionData({abi:EXECUTE_ABI,data}).args[0],'0x1004')
 assert.throws(()=>buildBestBuy({...request,route:{...route,poolKey:{...route.poolKey,fee:123}}}),/身份/)
 assert.throws(()=>buildBestBuy({...request,slippageBps:500}),/滑点/)
})

test('protocol-scoped testing never changes normal cross-protocol selection',()=>{
 const f=frame(),r=selectBestFrame(f,catalog,'0.1',now,{protocol:'Uniswap V2'})
 assert.equal(r.route.protocol,'Uniswap V2');assert.equal(r.scope,'Uniswap V2')
 assert.equal(selectBestFrame(f,catalog,'0.1',now).route.pool,catalog.at(-1).pool)
 assert.throws(()=>selectBestFrame(f,catalog,'0.1',now,{protocol:'Unknown'}),/没有有效报价/)
 assert.equal(fs.readFileSync(new URL('../src/bestSwap.js',import.meta.url),'utf8'),fs.readFileSync(new URL('../../examples/monad-best-buy/bestSwap.mjs',import.meta.url),'utf8'))
})
