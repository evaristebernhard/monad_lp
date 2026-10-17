import { chromium } from 'playwright'
import assert from 'node:assert/strict'
const browser = await chromium.launch({headless:true})
const page = await browser.newPage({viewport:{width:1600,height:1100}})
const errors=[], calls=[]
let marketAvailable=true
page.on('pageerror',e=>errors.push(e.message))
const token='0x'+'0'.repeat(40), quote='0x'+'1'.repeat(40), id='0x'+'2'.repeat(64), now=Math.floor(Date.now()/1000)
const pool={pool_address:id,protocol:'Uniswap V4',pair:'MON/TEST',pool_type:'uniswap_v4',protocol_rank:1,token_a_address:token,token_b_address:quote,token_a_symbol:'MON',token_b_symbol:'TEST',liquidity_usd:123000,volume_24h_usd:45000,updated_at:now-3600,monad_price:{value:2.5,status:'ok'},monad_vol:{value:null,status:'requires_24_consecutive_hourly_pool_prices'},monad_fee_apr:{value:0.15,status:'ok'}}
const event={watched_pool:id,watched_protocol:'Uniswap V4',signature:'0x'+'3'.repeat(64),log_index:'0x1',observed_at_ms:now*1000,whale_status:'valued',enrichment:{transaction:{event_kind:'swap',trade:{side:'buy',action:'buy',target_mint:token,quote_mint:quote,target_amount_ui:100,quote_amount_ui:250,swap_ratio_quote_per_target:2.5,notional_usd:12000,trader:quote}}}}
await page.route('**/api/**',async route=>{
 const u=new URL(route.request().url());calls.push(u.pathname)
 let body={}
 if(u.pathname==='/api/meta')body={chains:['monad'],protocols:['Uniswap V4'],pairs:[]}
 if(u.pathname==='/api/pools')body={items:[],total:0}
 if(u.pathname==='/api/summary')body={chains:[],protocols:[],pairs:[]}
 if(u.pathname==='/api/monad/display')body={token:null,pools:[],pool_ids:[],version:0}
 if(u.pathname==='/api/monad/resolve'){
  const input=u.searchParams.get('address')
  body=input==='SAME'?{success:false,resolved_token:null,address_type:'symbol',input_address:input,requires_selection:true,hint:'Choose an explicit token address',candidates:[{address:token,symbol:'SAME'},{address:quote,symbol:'SAME'}]}:input==='UNKNOWN'?{success:false,resolved_token:null,address_type:'symbol',input_address:input,requires_selection:false,hint:'No Monad catalog match',candidates:[]}:{success:true,resolved_token:token,address_type:'symbol',input_address:'MON'}
 }
 if(u.pathname==='/api/monad/listener/start'||u.pathname==='/api/monad/listener/status')body={listener:true}
 if(u.pathname==='/api/monad/pools')body={pools:[pool],pool_count:1}
 if(u.pathname==='/api/monad/protocols')body={protocols:[{decoder:'evm-abi',pool_types:['uniswap_v4']}]}
 if(u.pathname==='/api/monad/events')body={events:[event]}
 if(u.pathname==='/api/monad/whale-alerts')body={alerts:[event]}
 if(u.pathname==='/api/monad/pool-snapshots')body={snapshots:[]}
 if(u.pathname==='/api/monad/pool-history')body={data:[{timestamp:now*1000,open:2.4,high:2.6,low:2.3,close:2.5,volume_usd:12000}],candles:[{timestamp:(now-3600)*1000,open:2.4,high:2.6,low:2.3,close:2.5,volume_usd:12000}],hourly_prices:[{}],summary:{volume_usd:12000},source:'monad_local_trades'}
 if(u.pathname==='/api/monad/pool-candles'){
  assert.equal(u.searchParams.get('token'),token)
  if(!marketAvailable){await route.fulfill({status:502,json:{error:'OHLCV unavailable'}});return}
  body={source:'geckoterminal_onchain_ohlcv',price_unit:'USD',data:[{timestamp:now*1000,open:99,high:100,low:98,close:99.5,volume_usd:50000}]}
 }
 await route.fulfill({json:body})
})
try {
 await page.goto(process.env.MONAD_WEB_ORIGIN||'http://127.0.0.1:5173')
 await page.getByRole('button',{name:'Monad listener'}).click()
 assert.equal(await page.getByRole('group',{name:'Token search mode'}).count(),0)
 await page.getByRole('textbox',{name:'Token or pool'}).fill('UNKNOWN')
 await page.getByRole('button',{name:'Load',exact:true}).click()
 await page.getByText('No Monad catalog match',{exact:true}).waitFor()
 assert.deepEqual(errors,[])
 await page.getByRole('textbox',{name:'Token or pool'}).fill('SAME')
 await page.getByRole('button',{name:'Load',exact:true}).click()
 await page.getByRole('combobox',{name:'Choose token address'}).waitFor()
 assert.equal(calls.filter(x=>x==='/api/monad/listener/start').length,0)
 await page.getByRole('combobox',{name:'Choose token address'}).selectOption(token)
 assert.equal(await page.getByRole('textbox',{name:'Token or pool'}).inputValue(),'SAME')
 await page.locator('.solana-pool-select').waitFor()
 await page.getByRole('img',{name:'Pool price candlesticks with OHLC and volume'}).waitFor()
 assert.match(await page.locator('.solana-table').innerText(),/15.00%/)
 assert.match(await page.locator('.solana-chart-scale').innerText(),/1 candles[\s\S]*Price · USD/)
 assert.equal(await page.locator('.solana-event-row').count(),1)
 assert.equal(await page.locator('.solana-alert-row').count(),1)
 assert.equal(await page.locator('a[href*="/address/'+id+'"]').count(),0)
 assert(!calls.some(x=>x.includes('/solana/')))
 assert.equal(await page.evaluate(()=>localStorage.getItem('lp-screener.monad-listener-token')),token)
 marketAvailable=false
 await page.getByRole('button',{name:'5m',exact:true}).click()
 await page.getByText('Quote / target · local trades',{exact:false}).waitFor()
 assert.match(await page.locator('.solana-chart-scale').innerText(),/Price · quote \/ target/)
 assert.deepEqual(errors,[])
 await page.screenshot({path:process.env.MONAD_SCREENSHOT||'/tmp/monad-browser.png',fullPage:true})
 // The same input supports addresses and symbols, and repeated Load refreshes.
 const starts=calls.filter(p=>p==='/api/monad/listener/start').length
 await page.getByRole('textbox',{name:'Token or pool'}).fill(token)
 await page.getByRole('button',{name:'Load',exact:true}).click()
 await page.waitForFunction(()=>document.querySelector('.solana-pool-select')!=null)
 for(let i=0;i<100&&calls.filter(p=>p==='/api/monad/listener/start').length<=starts;i++)await page.waitForTimeout(50)
 assert(calls.filter(p=>p==='/api/monad/listener/start').length>starts)
 const secondStarts=calls.filter(p=>p==='/api/monad/listener/start').length
 await page.getByRole('textbox',{name:'Token or pool'}).fill('WMON')
 await page.getByRole('button',{name:'Load',exact:true}).click()
 for(let i=0;i<100&&calls.filter(p=>p==='/api/monad/listener/start').length<=secondStarts;i++)await page.waitForTimeout(50)
 assert(calls.filter(p=>p==='/api/monad/listener/start').length>secondStarts)
 await page.reload()
 await page.getByRole('button',{name:'Monad listener'}).click()
 assert.equal(await page.getByRole('textbox',{name:'Token or pool'}).inputValue(),'WMON')
 assert.deepEqual(errors,[])
 console.log('Browser passed: null resolution without blank screen, unified name/address input, repeated Load and name persistence, ambiguous token choice, pools, external USD chart, local ratio fallback, events, whales, V4 ID links, Monad-only endpoints')
} finally {await browser.close()}
