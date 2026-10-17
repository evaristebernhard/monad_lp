// Wallet signing is mocked; the read-only contract/replay tests use real testnet RPC.
import {chromium} from 'playwright'
import assert from 'node:assert/strict'
const browser=await chromium.launch({headless:true})
async function pageWith(mode){
 const page=await browser.newPage()
 await page.addInitScript(({mode})=>{
 let chain='0x8f',accounts=['0x41d5D3B5f2e497d1F4bCF767ffac9DDAb85D2b79'];window.walletCalls=[]
 const listeners=new Map();window.emitWallet=(event,value)=>{if(event==='accountsChanged')accounts=value;if(event==='chainChanged')chain=value;for(const handler of listeners.get(event)||[])handler(value)}
 window.ethereum={on:(event,handler)=>listeners.set(event,[...(listeners.get(event)||[]),handler]),removeListener:(event,handler)=>listeners.set(event,(listeners.get(event)||[]).filter(h=>h!==handler)),request:async({method,params})=>{window.walletCalls.push({method,params,chain});if(method==='eth_requestAccounts'||method==='eth_accounts')return accounts;if(method==='eth_chainId')return chain;if(method==='wallet_switchEthereumChain'){if(mode==='reject')throw {code:4001,message:'用户拒绝切换网络'};chain=params[0].chainId;if(mode==='changed')accounts=['0x0000000000000000000000000000000000000001'];return null}if(method==='eth_sendTransaction')throw {code:4001,message:'用户取消部署签名'};throw Error('Unexpected wallet method '+method)}}
 },{mode})
 await page.goto('http://127.0.0.1:5173/#selection');await page.getByRole('heading',{name:'选池策略',exact:true}).waitFor();await page.getByRole('button',{name:'暂停',exact:true}).click();return page
}
for(const mode of ['reject','changed','sign']){
 const page=await pageWith(mode);await page.getByText('策略与合约',{exact:true}).click();await page.getByRole('button',{name:'部署到测试网',exact:true}).click();await page.getByRole('alert').waitFor({timeout:45000})
 const calls=await page.evaluate(()=>window.walletCalls)
 assert.ok(calls.some(c=>c.method==='wallet_switchEthereumChain'&&c.params[0].chainId==='0x279f'))
 const sent=calls.filter(c=>c.method==='eth_sendTransaction')
 assert.equal(sent.length,mode==='sign'?1:0)
 if(sent.length){assert.equal(sent[0].chain,'0x279f');assert.equal(sent[0].params[0].value,undefined)}
 console.log(`Mock wallet ${mode} passed`);await page.close()
}
const page=await pageWith('sign')
await page.route('**/api/selection/testnet-rpc',async route=>{
 const body=route.request().postDataJSON()
 if(body.method==='eth_getBalance')return route.fulfill({json:{jsonrpc:'2.0',id:body.id,result:body.params[0].toLowerCase().startsWith('0x41d5')?'0x1bc16d674ec8000':'0x0'}})
 return route.continue()
})
await page.getByRole('button',{name:'连接钱包',exact:true}).click()
await page.getByRole('button',{name:'切换到测试网',exact:true}).waitFor()
await page.getByText('0.125',{exact:true}).waitFor()
await page.getByRole('button',{name:'切换到测试网',exact:true}).click()
await page.locator('.selection-wallet-network.ready').waitFor()
assert.equal(await page.getByRole('button',{name:'切换到测试网',exact:true}).count(),0)
await page.evaluate(()=>window.emitWallet('accountsChanged',['0x0000000000000000000000000000000000000001']))
await page.waitForFunction(()=>document.querySelector('.selection-wallet-balance strong')?.textContent==='0')
assert.ok((await page.locator('.selection-wallet-info>a').getAttribute('title')).endsWith('0001'))
await page.evaluate(()=>window.emitWallet('chainChanged','0x8f'))
await page.getByRole('button',{name:'切换到测试网',exact:true}).waitFor()
await page.screenshot({path:'/tmp/selection-wallet-desktop.png',fullPage:true})
await page.setViewportSize({width:390,height:844})
assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false)
await page.screenshot({path:'/tmp/selection-wallet-mobile.png',fullPage:true})
await page.getByRole('button',{name:'断开连接',exact:true}).click()
await page.getByRole('button',{name:'连接钱包',exact:true}).waitFor()
await page.evaluate(()=>window.emitWallet('accountsChanged',['0x41d5D3B5f2e497d1F4bCF767ffac9DDAb85D2b79']))
assert.equal(await page.getByRole('button',{name:'连接钱包',exact:true}).count(),1)
assert.equal(await page.evaluate(()=>window.walletCalls.filter(c=>c.method==='eth_sendTransaction').length),0)
console.log('Visible wallet connection, testnet switch, balance, account/network events, disconnect and mobile passed')
await page.close()
await browser.close()
