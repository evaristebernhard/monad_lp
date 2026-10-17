// Opt-in live smoke test: running Vite/API/RPC plus real chart provider required.
import { chromium } from 'playwright'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { homedir } from 'node:os'
const api=process.env.MONAD_API_ORIGIN||'http://127.0.0.1:8100'
const config=await (await fetch(api+'/monad/display')).json()
assert(config.token && config.pool_ids.length,'Select a Monad token before this test')
const browser=await chromium.launch({headless:true})
const page=await browser.newPage({viewport:{width:1600,height:1100}})
const errors=[],calls=[]
page.on('pageerror',e=>errors.push(e.message))
await page.addInitScript(token=>localStorage.setItem('lp-screener.monad-listener-token',token),config.token)
if(process.env.MONAD_API_ORIGIN)await page.route('**/api/**',async route=>{
 const original=new URL(route.request().url())
 calls.push(original.pathname)
 const response=await route.fetch({url:api+original.pathname+original.search})
 await route.fulfill({response})
})
else page.on('request',r=>calls.push(new URL(r.url()).pathname))
try {
 await page.goto(process.env.MONAD_WEB_ORIGIN||'http://127.0.0.1:5173')
 await page.getByRole('button',{name:'Wallet listener'}).click()
 await page.locator('.solana-pool-select').first().waitFor({timeout:45000})
 await page.getByRole('button',{name:'1h',exact:true}).click()
 await page.waitForFunction(()=>document.querySelector('.solana-chart-scale')?.innerText.includes('Price · USD'),null,{timeout:45000})
 assert.equal(await page.locator('.solana-pool-select').count(),config.pool_ids.length)
 assert(!calls.some(p=>p.includes('/solana/')))
 assert.deepEqual(errors,[])
 await mkdir(homedir()+'/.cache/monad-verification',{recursive:true})
 await page.screenshot({path:process.env.MONAD_SCREENSHOT||homedir()+'/.cache/monad-verification/live.png',fullPage:true})
 console.log(JSON.stringify({pools:await page.locator('.solana-pool-select').count(),events:await page.locator('.solana-event-row').count(),chart:await page.locator('.solana-chart-scale').innerText(),errors,solanaListenerCalls:calls.filter(p=>p.includes('/solana/')).length}))
} finally {await browser.close()}
