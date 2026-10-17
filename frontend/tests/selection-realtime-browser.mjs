// Deterministic RPC responses exercise timing and failures, not market prices.
import {chromium} from 'playwright'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import http from 'node:http'
import {parseAbi,encodeFunctionResult,decodeFunctionData} from 'viem'
import {quoteStreamRequest,decodeQuoteFrame,selectV4} from '../src/selectionQuotes.js'
const data=JSON.parse(fs.readFileSync(new URL('../src/generated/selection.json',import.meta.url)))
const multi=parseAbi(['function aggregate3((address target,bool allowFailure,bytes callData)[] calls) payable returns((bool success,bytes returnData)[] results)'])
const v4=parseAbi(['function quoteExactInputSingle(((address currency0,address currency1,uint24 fee,int24 tickSpacing,address hooks) poolKey,bool zeroForOne,uint128 exactAmount,bytes hookData) params) returns(uint256 amountOut,uint256 gasEstimate)'])
const v3=parseAbi(['function quoteExactInputSingle((address tokenIn,address tokenOut,uint256 amountIn,uint24 fee,uint160 sqrtPriceLimitX96) params) returns(uint256 amountOut,uint160 sqrtPriceX96After,uint32 initializedTicksCrossed,uint256 gasEstimate)'])
const v2=parseAbi(['function getReserves() view returns(uint112,uint112,uint32)'])
const browser=await chromium.launch({headless:true}),page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[]
page.on('pageerror',e=>errors.push(e.message))
let head=1000,fail=false,delayNext=0,active=0,maxActive=0,rounds=[],delayed=false
function block(n){return {number:'0x'+n.toString(16),hash:'0x'+n.toString(16).padStart(64,'0'),timestamp:'0x'+Math.floor(Date.now()/1000).toString(16),transactions:[]}}
const server=http.createServer(async(req,res)=>{
 res.setHeader('Access-Control-Allow-Origin','*');res.setHeader('Access-Control-Allow-Headers','Content-Type');res.setHeader('Access-Control-Allow-Methods','POST,OPTIONS')
 if(req.method==='OPTIONS'){res.end();return}
 const chunks=[];for await(const chunk of req)chunks.push(chunk)
 const body=JSON.parse(Buffer.concat(chunks).toString()),{args}=decodeFunctionData({abi:multi,data:quoteStreamRequest(data.routes.candidates,body.amount,body.token_in).data})
 res.writeHead(200,{'Content-Type':'text/event-stream','Cache-Control':'no-cache'});res.flushHeaders()
 let closed=false,pending=false;const stop=new AbortController()
 const tick=async()=>{
  if(closed||pending)return;pending=true;active++;maxActive=Math.max(maxActive,active)
  const n=++head;rounds.push({at:Date.now(),n});const delay=delayNext;delayNext=0;if(delay)delayed=true
  await new Promise(resolve=>{const timer=setTimeout(resolve,delay||30);stop.signal.addEventListener('abort',()=>{clearTimeout(timer);resolve()},{once:true})})
  if(!closed){
   if(fail)res.write('data: '+JSON.stringify({error:'offline'})+'\n\n')
   else{
    const response=args[0].map((c,i)=>{
     const q=data.routes.candidates[i]
     if(q.kind==='v2')return {success:true,returnData:encodeFunctionResult({abi:v2,functionName:'getReserves',result:[100000000000000000000n,1000000n,0]})}
     const abi=q.kind==='v3'?v3:v4,decoded=decodeFunctionData({abi,data:c.callData}),p=decoded.args[0],amount=q.kind==='v3'?p.amountIn:p.exactAmount
     const out=(i===(n%2)?900000n:100000n)+amount/1000000000000n+BigInt(n)
     return {success:true,returnData:encodeFunctionResult({abi,functionName:'quoteExactInputSingle',result:q.kind==='v3'?[out,0n,0,1n]:[out,1n]})}
    })
    const raw={chainId:143,block:block(n),result:encodeFunctionResult({abi:multi,functionName:'aggregate3',result:response}),startedAtMs:Date.now()-30,capturedAtMs:Date.now(),durationMs:30}
    const snapshot=decodeQuoteFrame(data.routes.candidates,body.amount,body.token_in,raw),decision=selectV4(snapshot)
    res.write('data: '+JSON.stringify({...snapshot,selected:{pool:decision.values[0],amountOutRaw:String(decision.values[1])},replayHash:decision.values[2]})+'\n\n')
   }
  }
  active--;pending=false
 }
 const timer=setInterval(tick,1000);res.on('close',()=>{closed=true;clearInterval(timer);stop.abort()});void tick()
})
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve))
await page.route('**/api/selection/quote-stream',route=>route.continue({url:`http://127.0.0.1:${server.address().port}/api/selection/quote-stream`}))

try{
await page.goto('http://127.0.0.1:5173/#selection')
await page.waitForFunction(()=>Number(document.querySelector('output')?.textContent)>0)
const initial=await page.getByTestId('selection-source-block').innerText(),out=await page.getByLabel('预计收到数量',{exact:true}).innerText()
await page.waitForFunction(v=>document.querySelector('[data-testid="selection-source-block"]')?.textContent!==v,initial)
assert.notEqual(await page.getByLabel('预计收到数量',{exact:true}).innerText(),out)
await page.waitForTimeout(2200)
assert.ok(rounds.length>=3);assert.equal(maxActive,1)
assert.ok(rounds.slice(1).every((r,i)=>r.at-rounds[i].at>=800))
await page.getByRole('button',{name:'暂停',exact:true}).click()
const frozen=await page.getByTestId('selection-source-block').innerText(),count=rounds.length
await page.waitForTimeout(2200)
assert.equal(await page.getByTestId('selection-source-block').innerText(),frozen);assert.equal(rounds.length,count)
await page.getByRole('button',{name:'继续',exact:true}).click()
await page.waitForFunction(v=>document.querySelector('[data-testid="selection-source-block"]')?.textContent!==v,frozen)
// Delay a round, change its input, then ensure the old response cannot replace it.
delayNext=1800
while(!delayed)await page.waitForTimeout(100)
await page.getByLabel('支付数量',{exact:true}).fill('0.2')
assert.equal(await page.getByLabel('预计收到数量',{exact:true}).innerText(),'—')
await page.waitForFunction(()=>Number(document.querySelector('output')?.textContent)>0)
await page.getByRole('button',{name:'暂停',exact:true}).click()
await page.getByText('计算记录',{exact:true}).click()
const download=page.waitForEvent('download');await page.getByRole('button',{name:'下载结果',exact:true}).click()
const artifact=JSON.parse(fs.readFileSync(await (await download).path(),'utf8'))
assert.equal(artifact.args[0].amountIn,'200000000000000000')
// Retain a previous quote visibly as stale on an upstream outage.
fail=true;await page.getByRole('button',{name:'继续',exact:true}).click()
await page.getByRole('alert').waitFor()
assert.ok((await page.getByRole('alert').innerText()).includes('已过期'))
assert.equal(await page.locator('.selection-token-output.selection-quote-old').count(),1)
assert.deepEqual(errors,[])
console.log('1s cadence, no overlapping rounds, live output/block changes, pause/resume, stale-input cancellation and outage state passed')
}finally{await browser.close();await new Promise(resolve=>server.close(resolve))}
