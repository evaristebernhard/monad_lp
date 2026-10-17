import { chromium } from 'playwright'
import assert from 'node:assert/strict'
import { encodeAbiParameters, parseAbiParameters, decodeFunctionData } from 'viem'
import { ROUTER_ABI, ROUTER, USDC } from '../src/monadTrade.js'
const browser=await chromium.launch({headless:true}),page=await browser.newPage({viewport:{width:1500,height:1000}})
const account='0x1111111111111111111111111111111111111111', hash='0x'+'a'.repeat(64), blockHash='0x'+'b'.repeat(64)
const errors=[];let insufficient=false
page.on('pageerror',e=>errors.push(e.message))
await page.addInitScript(({account,hash})=>{
 window.__walletCalls=[];window.__reject=true;let chain='0x1';const handlers={}
 window.ethereum={on:(name,fn)=>{handlers[name]=fn},removeListener:name=>delete handlers[name],request:async r=>{
  window.__walletCalls.push(r)
  if(r.method==='eth_chainId')return chain
  if(r.method==='wallet_switchEthereumChain'){chain='0x8f';handlers.chainChanged?.(chain);return null}
  if(r.method==='eth_accounts'||r.method==='eth_requestAccounts')return [account]
  if(r.method==='eth_sendTransaction'){if(window.__reject){const e=new Error('User rejected');e.code=4001;throw e}return hash}
  throw Error('Unexpected wallet request '+r.method)
 }}
},{account,hash})
await page.route('**/api/**',async route=>{
 const path=new URL(route.request().url()).pathname
 if(path!=='/api/monad/trade-rpc'){await route.fulfill({json:path==='/api/monad/demo'?{pools:[]}:{}});return}
 const request=route.request().postDataJSON();let result
 switch(request.method){
 case 'eth_chainId':result='0x8f';break
 case 'eth_call':result=encodeAbiParameters(parseAbiParameters('uint256,uint256'),[23197n,150000n]);break
 case 'eth_getBalance':result=insufficient?'0x0':'0x56bc75e2d63100000';break
 case 'eth_estimateGas':result='0x30000';break
 case 'eth_gasPrice':result='0x174876e800';break
 case 'eth_blockNumber':result='0x1234';break
 case 'eth_getBlockByNumber':result={number:'0x1234',hash:blockHash,timestamp:'0x6b000000',transactions:[],baseFeePerGas:'0x1',gasLimit:'0x1000000',gasUsed:'0x0',extraData:'0x'};break
 case 'eth_getTransactionReceipt':result={transactionHash:hash,blockHash,blockNumber:'0x1234',transactionIndex:'0x0',from:account,to:ROUTER,cumulativeGasUsed:'0x30000',gasUsed:'0x30000',effectiveGasPrice:'0x1',status:'0x1',type:'0x2',logs:[{address:USDC,blockHash,blockNumber:'0x1234',transactionHash:hash,transactionIndex:'0x0',logIndex:'0x0',removed:false,data:'0x'+(23197n).toString(16).padStart(64,'0'),topics:['0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef','0x'+'0'.repeat(64),'0x'+account.slice(2).padStart(64,'0')]}],logsBloom:'0x'+'0'.repeat(512),contractAddress:null};break
 default:throw Error('Unexpected RPC '+request.method)
 }
 await route.fulfill({json:{jsonrpc:'2.0',id:request.id,result}})
})
try{
 await page.goto('http://127.0.0.1:5173');await page.getByRole('heading',{name:'Buy USDC with MON'}).waitFor()
 const confirm=page.getByRole('button',{name:'Confirm purchase'}),preview=page.getByRole('button',{name:'Get pool quote'})
 assert(await confirm.isDisabled())
 await page.getByRole('button',{name:'Connect wallet',exact:true}).click()
 await page.getByRole('button',{name:'Disconnect',exact:true}).waitFor()
 await page.getByRole('textbox',{name:'MON amount'}).fill('1')
 await preview.click();await page.getByText('0.023197 USDC',{exact:true}).waitFor()
 await confirm.click();await page.getByRole('alert').filter({hasText:'Cancelled in wallet'}).waitFor()
 assert.equal(await page.locator('a[href*="/tx/"]').count(),0)
 insufficient=true;await confirm.click();await page.getByRole('alert').filter({hasText:'Insufficient MON'}).waitFor()
 assert.equal((await page.evaluate(()=>window.__walletCalls)).filter(r=>r.method==='eth_sendTransaction').length,1)
 insufficient=false
 await page.getByRole('textbox',{name:'MON amount'}).fill('2');assert(await confirm.isDisabled())
 await preview.click();await page.getByText('0.023197 USDC',{exact:true}).waitFor()
 await page.evaluate(()=>{window.__reject=false});await confirm.click()
 await page.getByText('Transaction confirmed',{exact:false}).waitFor()
 await page.getByText('Received 0.023197 USDC',{exact:false}).waitFor()
 const calls=await page.evaluate(()=>window.__walletCalls),sent=calls.filter(r=>r.method==='eth_sendTransaction').at(-1).params[0]
 assert.equal(sent.to.toLowerCase(),ROUTER);assert.equal(BigInt(sent.value),2n*10n**18n)
 const decoded=decodeFunctionData({abi:ROUTER_ABI,data:sent.data});assert.equal(decoded.args[0],'0x1004')
 assert(!calls.some(r=>r.method==='personal_sign'||r.method==='eth_signTypedData_v4'))
 assert.deepEqual(errors,[])
 await page.screenshot({path:new URL('../../output/monad-trade-browser.png',import.meta.url).pathname,fullPage:true})
 console.log('Trade browser passed: Monad switch, chain quote, native MON purchase, rejection, insufficient gas funds, input invalidation, confirmed receipt; wallet submission mocked.')
}catch(e){console.error(await page.locator('.monad-trade').innerText());console.error(await page.evaluate(()=>window.__walletCalls));throw e}finally{await browser.close()}
