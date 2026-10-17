// Slow feeds/OHLCV must not delay a token switch or replace its results later.
import { chromium } from 'playwright'
import assert from 'node:assert/strict'
const mon='0x'+'1'.repeat(40),eth='0x'+'2'.repeat(40),quote='0x'+'3'.repeat(40)
const pools={}
for(const [token,symbol,n] of [[mon,'WMON','4'],[eth,'WETH','5']])pools[token]={pool_address:'0x'+n.repeat(40),protocol:'Uniswap V3',pool_type:'concentrated_v3',pair:symbol+'/USDC',token_a_address:token,token_b_address:quote,token_a_symbol:symbol,token_b_symbol:'USDC',liquidity_usd:100000,volume_24h_usd:10000}
const browser=await chromium.launch({headless:true}),page=await browser.newPage()
const resolves=[],errors=[]
page.on('pageerror',e=>errors.push(e.message))
await page.addInitScript(token=>localStorage.setItem('lp-screener.monad-listener-token',token),mon)
await page.route('**/api/**',async route=>{
 const u=new URL(route.request().url()),path=u.pathname,token=u.searchParams.get('token')||mon
 let body={}
 if(path==='/api/meta')body={chains:['monad'],protocols:[],pairs:[]}
 if(path==='/api/pools')body={items:[],total:0}
 if(path==='/api/summary')body={chains:[],protocols:[],pairs:[]}
 if(path==='/api/monad/resolve'){resolves.push(u.searchParams.get('address'));body={success:true,resolved_token:eth,address_type:'symbol',input_address:'ETH'}}
 if(path.endsWith('/listener/start')||path.endsWith('/listener/status'))body={listener:true}
 if(path==='/api/monad/pools'){await new Promise(r=>setTimeout(r,30));body={pools:[pools[token]],pool_count:1}}
 if(['/api/monad/events','/api/monad/whale-alerts','/api/monad/pool-snapshots','/api/monad/protocols'].includes(path)){await new Promise(r=>setTimeout(r,3000));body={events:[],alerts:[],snapshots:[],protocols:[]}}
 if(path==='/api/monad/pool-history')body={candles:[{timestamp:Date.now(),open:1,high:1,low:1,close:1,volume_usd:0}],trades:[],summary:{}}
 if(path==='/api/monad/pool-candles'){await new Promise(r=>setTimeout(r,4000));body={price_unit:'USD',data:[{timestamp:Date.now(),open:2,high:2,low:2,close:2,volume_usd:0}]}}
 await route.fulfill({json:body}).catch(()=>{})
})
try{
 await page.goto(process.env.MONAD_WEB_ORIGIN||'http://127.0.0.1:5173')
 await page.getByRole('button',{name:'Monad listener'}).click()
 await page.locator('.solana-pool-select').filter({hasText:'WMON/USDC'}).waitFor({timeout:1500})
 await page.getByRole('textbox',{name:'Token or pool'}).fill('ETH')
 const started=performance.now()
 await page.getByRole('button',{name:'Load',exact:true}).click()
 await page.locator('.solana-pool-select').filter({hasText:'WETH/USDC'}).waitFor({timeout:1500})
 const switchMs=Math.round(performance.now()-started)
 assert(switchMs<1000,`switch took ${switchMs}ms`)
 assert.deepEqual(resolves,['ETH'],'duplicate name/address resolution')
 await page.getByRole('img',{name:'Pool price candlesticks with OHLC and volume'}).waitFor({timeout:1000})
 assert.match(await page.locator('.solana-chart-scale').innerText(),/quote \/ target/)
 await page.waitForTimeout(3200)
 assert.equal(await page.locator('.solana-pool-select').filter({hasText:'WMON/USDC'}).count(),0)
 assert.deepEqual(errors,[])
 console.log(JSON.stringify({switchMs,slowFeedMs:3000,slowChartMs:4000,resolveRequests:resolves.length,staleResultsRejected:true}))
}finally{await browser.close()}
