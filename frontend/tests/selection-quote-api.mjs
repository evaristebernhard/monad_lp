import fs from 'node:fs'
import assert from 'node:assert/strict'
import {createPublicClient,http} from 'viem'
import {backendSnapshot,selectV4,fetchSelectionQuotes} from '../src/selectionQuotes.js'
import {subscribeJson} from '../src/streamJson.js'
import {json} from '../src/poolSelection.js'
const origin=process.env.QUOTE_API_ORIGIN||'http://127.0.0.1:8100',data=JSON.parse(fs.readFileSync(new URL('../src/generated/selection.json',import.meta.url)))
const rpc=createPublicClient({cacheTime:0,transport:http('http://127.0.0.1:8100/api/monad/trade-rpc',{retryCount:0,timeout:15000})}),testnet=createPublicClient({transport:http('https://testnet-rpc.monad.xyz',{retryCount:1,timeout:20000})}),results=[]
const pools=await (await fetch(origin+'/api/selection/pools')).json();assert.equal(pools.candidates.length,14)
for(const token of ['MON','USDC']){
 const parameters={amount:'0.1',token_in:token,token_out:token==='MON'?'USDC':'MON'}
 const response=await fetch(origin+'/api/selection/quote'+(token==='MON'?'?'+new URLSearchParams(parameters):''),token==='MON'?undefined:{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(parameters)})
 assert.equal(response.status,200);assert.equal(response.headers.get('cache-control'),'no-store')
 const frame=await response.json(),snapshot=backendSnapshot(frame,data.routes.candidates,'0.1',token),decision=selectV4(snapshot)
 const pinned={...rpc,getBlock:()=>rpc.getBlock({blockNumber:BigInt(snapshot.sourceBlock)})},direct=await fetchSelectionQuotes(data.routes.candidates,'0.1',token,pinned,{multicall:false})
 assert.deepEqual(snapshot.scenarios[0].quotes.map(q=>q.amountOutRaw),direct.scenarios[0].quotes.map(q=>q.amountOutRaw))
 const chain=await testnet.readContract({address:data.v4.reference.contractAddress,abi:data.v4.abi,functionName:'selectBest',args:decision.args})
 assert.deepEqual(chain,decision.values)
 results.push({token,frame,localAndChainMatch:true,independentQuotesMatch:true})
 console.log(json({token,block:frame.sourceBlock,available:frame.available,selected:frame.selected,independentAndChainMatch:true}))
}
for(const query of ['amount=0&token_in=MON','amount=-1&token_in=MON','amount=0.1&token_in=WBTC','amount=0.1&token_in=USDC&token_out=USDC','amount=0.0000001&token_in=USDC'])assert.equal((await fetch(origin+'/api/selection/quote?'+query)).status,400)
const rejected=await fetch(origin+'/api/selection/quote',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({amount:0.1})});assert.equal(rejected.status,422)
const controller=new AbortController(),samples=[],timeout=setTimeout(()=>controller.abort(),20000)
await subscribeJson(origin+'/api/selection/quote-stream?amount=0.1&token_in=MON&token_out=USDC',{signal:controller.signal,onValue:frame=>{
 backendSnapshot(frame,data.routes.candidates,'0.1','MON');samples.push({at:Date.now(),block:frame.sourceBlock});if(samples.length===4)controller.abort()
},onError:e=>console.log(e.message)})
clearTimeout(timeout);assert.equal(samples.length,4);assert.ok(BigInt(samples.at(-1).block)>BigInt(samples[0].block))
fs.writeFileSync(new URL('../../选池/results/quote-api-verification.json',import.meta.url),json({origin,results,stream:samples,invalidParametersRejected:true}))
console.log(json({sse:true,samples,intervalMs:samples.slice(1).map((s,i)=>s.at-samples[i].at)}))
