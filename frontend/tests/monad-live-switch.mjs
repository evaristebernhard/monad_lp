// Opt-in: use an isolated API/display DB when checking live token switching.
import { chromium } from 'playwright'
import assert from 'node:assert/strict'
const api=process.env.MONAD_API_ORIGIN||'http://127.0.0.1:8100'
const browser=await chromium.launch({headless:true}),page=await browser.newPage()
const errors=[],resolves=[],timings=[]
let closing=false
page.on('pageerror',e=>errors.push(e.message))
await page.addInitScript(()=>localStorage.setItem('lp-screener.monad-listener-token','0x3bd359c1119da7da1d913d1c4d2b7c461115433a'))
await page.route('**/api/**',async route=>{
 const u=new URL(route.request().url())
 if(u.pathname==='/api/monad/resolve')resolves.push(u.searchParams.get('address'))
 try {const response=await route.fetch({url:api+u.pathname+u.search});await route.fulfill({response})}catch(e){if(!closing)throw e}
})
try{
 await page.goto(process.env.MONAD_WEB_ORIGIN||'http://127.0.0.1:5173')
 await page.getByRole('button',{name:'Monad listener'}).click()
 await page.locator('.solana-pool-select').first().waitFor({timeout:15000})
 for(const input of ['eth','ETH','Ethereum']){
  await page.getByRole('textbox',{name:'Token or pool'}).fill(input)
  const started=performance.now()
  await page.getByRole('button',{name:'Load',exact:true}).click()
  await page.waitForFunction(()=>{
   const pools=[...document.querySelectorAll('.solana-pool-select')]
   return pools.length===5&&pools.every(p=>p.innerText.includes('WETH'))
  },null,{timeout:10000})
  timings.push({input,switchMs:Math.round(performance.now()-started)})
  assert.equal(await page.getByRole('textbox',{name:'Token or pool'}).inputValue(),input)
 }
 assert.deepEqual(resolves,['eth','ETH','Ethereum'])
 assert.deepEqual(errors,[])
 console.log(JSON.stringify({timings,resolveRequests:resolves.length,pools:5,errors}))
}finally{closing=true;await browser.close()}
