// Market UI was intentionally rolled back; contract artifacts remain available.
import {chromium} from 'playwright'
import assert from 'node:assert/strict'
import fs from 'node:fs'
const data=JSON.parse(fs.readFileSync(new URL('../src/generated/selection.json',import.meta.url)))
for(const name of ['buy','rent','buyOwnership'])assert(data.market.abi.some(row=>row.name===name))
const browser=await chromium.launch({headless:true})
try{
 const page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[]
 page.on('pageerror',e=>errors.push(e.message))
 await page.addInitScript(()=>{window.ethereum={request:async({method})=>{if(method==='eth_requestAccounts')throw Object.assign(Error('用户取消签名'),{code:4001});if(method==='eth_chainId')return '0x8f';if(method==='eth_accounts')return []},on(){},removeListener(){}}})
 await page.goto('http://127.0.0.1:5173/#selection',{waitUntil:'domcontentloaded'})
 await page.getByRole('heading',{name:'最优报价',exact:true}).waitFor()
 assert.equal(await page.getByRole('region',{name:'合约 Market'}).count(),0)
 const buy=page.getByRole('button',{name:'Buy USDC · 使用最优报价'})
 await page.waitForFunction(()=>{const b=[...document.querySelectorAll('button')].find(b=>b.textContent==='Buy USDC · 使用最优报价');return b&&!b.disabled},{},{timeout:60000})
 await buy.click();await page.getByRole('alert').filter({hasText:'用户取消签名'}).waitFor()
 assert.equal(await page.evaluate(()=>localStorage.getItem('selection-buy-pending-v1')),null)
 await page.getByText('合约调用 / 租金',{exact:true}).click()
 await page.getByRole('button',{name:'支付租金并调用',exact:true}).click()
 await page.getByRole('alert').filter({hasText:'用户取消签名'}).waitFor()
 await page.screenshot({path:'/tmp/selection-restored-desktop.png',fullPage:true})
 await page.setViewportSize({width:390,height:844})
 assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false)
 await page.screenshot({path:'/tmp/selection-restored-mobile.png',fullPage:true})
 assert.deepEqual(errors,[])
 console.log('Quote-first UI, best Buy and rental wallet rejection, retained contracts, mobile passed')
}finally{await browser.close()}
