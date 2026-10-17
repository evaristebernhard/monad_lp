// Opt-in running API/RPC/browser check: fixed 20 pools must keep advancing.
import { chromium } from 'playwright'
import assert from 'node:assert/strict'
import { writeFile } from 'node:fs/promises'
const origin=process.env.MONAD_WEB_ORIGIN||'http://127.0.0.1:5173'
const browser=await chromium.launch({headless:true}),page=await browser.newPage({viewport:{width:1600,height:1100}})
const errors=[],calls=[],samples=[]
page.on('pageerror',e=>errors.push(e.message))
page.on('request',r=>calls.push(new URL(r.url()).pathname))
try {
 await page.goto(origin,{waitUntil:'domcontentloaded',timeout:30000})
 await page.locator('h1').filter({hasText:/^20 pool$/}).waitFor()
 await page.locator('.solana-table tbody tr').nth(19).waitFor({timeout:20000})
 for(let i=0;i<5;i++){
  const started=performance.now()
  const payload=await page.evaluate(async()=>{
   const response=await fetch('/api/monad/demo',{signal:AbortSignal.timeout(15000)})
   if(!response.ok)throw new Error('Demo HTTP '+response.status)
   return response.json()
  }),now=Math.floor(Date.now()/1000)
  assert.equal(payload.pools.length,20)
  assert.equal(new Set(payload.pools.map(p=>p.pair.toLowerCase().split('/').sort().join('/'))).size,20)
  const pools=payload.pools.map(p=>({name:p.pair,state_block:p.price.state_block,value:p.price.value,as_of:p.price.as_of,age:now-p.price.as_of,statuses:['price','vol','fee_apr','volume'].map(k=>p[k].status)}))
  assert(payload.pools.every(p=>Number.isFinite(p.price.value)))
  samples.push({at:now,requestMs:Math.round(performance.now()-started),maxAge:Math.max(...pools.map(p=>p.age)),pools})
  if(i<4)await page.waitForTimeout(5000)
 }
 assert(samples.at(-1).pools.every(p=>p.as_of>samples[0].pools.find(x=>x.name===p.name).as_of),'RPC source times must advance for all 20 pools')
 assert(samples.at(-1).pools.every(p=>p.age<=30),'Prices must remain fresh')
 assert(calls.includes('/api/monad/prices-stream'),'Demo must subscribe to live price frames')
 assert(samples.at(-1).pools.every(p=>p.state_block>samples[0].pools.find(x=>x.name===p.name).state_block),'All 20 source blocks must advance')
 assert(!calls.includes('/api/pools'),'Demo must not load the full market')
 assert.deepEqual(errors,[])
 await writeFile(new URL('../../output/monad-live-demo-check.json',import.meta.url),JSON.stringify(samples,null,2)+'\n')
 await page.screenshot({path:new URL('../../output/monad-live-demo-public.png',import.meta.url).pathname,fullPage:true})
 console.log(JSON.stringify({pools:20,allClocksAdvanced:true,maxAges:samples.map(s=>s.maxAge),requestMs:samples.map(s=>s.requestMs),errors}))
}finally{await browser.close()}
