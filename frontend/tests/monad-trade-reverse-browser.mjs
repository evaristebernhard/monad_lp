import {chromium} from 'playwright'
import assert from 'node:assert/strict'
import {encodeAbiParameters,parseAbiParameters,decodeFunctionData,toHex} from 'viem'
import {USDC,PERMIT2,ROUTER,POOL_MANAGER,POOL_ID,ERC20_ABI,PERMIT2_ABI,QUOTER_ABI,ROUTER_ABI} from '../src/monadTrade.js'
const browser=await chromium.launch({headless:true}),page=await browser.newPage({viewport:{width:1500,height:1000}})
const account='0x1111111111111111111111111111111111111111',blockHash='0x'+'b'.repeat(64),timestamp=Math.floor(Date.now()/1000)
let tokenAllowance=0n,routerAllowance=0n,expiry=0,sequence=0
const sent=[],receipts=new Map(),errors=[]
page.on('pageerror',e=>errors.push(e.message))
await page.exposeFunction('submitMock',tx=>{
 const to=tx.to.toLowerCase();assert.equal(BigInt(tx.value||0),0n)
 let kind
 if(to===USDC){const call=decodeFunctionData({abi:ERC20_ABI,data:tx.data});assert.equal(call.functionName,'approve');assert.equal(call.args[0].toLowerCase(),PERMIT2);assert.equal(call.args[1],100000n);tokenAllowance=call.args[1];kind='token'}
 else if(to===PERMIT2){const call=decodeFunctionData({abi:PERMIT2_ABI,data:tx.data});assert.equal(call.functionName,'approve');assert.equal(call.args[0].toLowerCase(),USDC);assert.equal(call.args[1].toLowerCase(),ROUTER);assert.equal(call.args[2],100000n);assert.equal(call.args[3],timestamp+600);routerAllowance=call.args[2];expiry=call.args[3];kind='router'}
 else {assert.equal(to,ROUTER);assert(tokenAllowance>=100000n&&routerAllowance>=100000n);const call=decodeFunctionData({abi:ROUTER_ABI,data:tx.data});assert.equal(call.args[0],'0x1004');kind='swap';tokenAllowance-=100000n;routerAllowance-=100000n}
 const hash=toHex(BigInt(++sequence),{size:32});sent.push({kind,tx});receipts.set(hash,kind);return hash
})
await page.addInitScript(({account})=>{window.__reject=true;window.ethereum={on:()=>{},removeListener:()=>{},request:async r=>{if(r.method==='eth_chainId')return'0x8f';if(['eth_accounts','eth_requestAccounts'].includes(r.method))return[account];if(r.method==='eth_sendTransaction'){if(window.__reject)throw Object.assign(new Error('Rejected'),{code:4001});return window.submitMock(r.params[0])}throw Error(r.method)}}},{account})
await page.route('**/api/**',async route=>{
 const path=new URL(route.request().url()).pathname
 if(path!=='/api/monad/trade-rpc'){await route.fulfill({json:path==='/api/monad/demo'?{pools:[]}:{}});return}
 const r=route.request().postDataJSON();let result
 if(r.method==='eth_call'){
  const to=r.params[0].to.toLowerCase()
  if(to===USDC){const call=decodeFunctionData({abi:ERC20_ABI,data:r.params[0].data});result=encodeAbiParameters(parseAbiParameters('uint256'),[call.functionName==='balanceOf'?1000000n:tokenAllowance])}
  else if(to===PERMIT2)result=encodeAbiParameters(parseAbiParameters('uint160,uint48,uint48'),[routerAllowance,expiry,0])
  else {const call=decodeFunctionData({abi:QUOTER_ABI,data:r.params[0].data});assert.equal(call.args[0].zeroForOne,false);assert.equal(call.args[0].exactAmount,100000n);result=encodeAbiParameters(parseAbiParameters('uint256,uint256'),[2n*10n**18n,150000n])}
 }else if(r.method==='eth_getBalance')result=toHex(100n*10n**18n)
 else if(r.method==='eth_estimateGas')result='0x30000'
 else if(r.method==='eth_gasPrice')result='0x174876e800'
 else if(r.method==='eth_blockNumber')result='0x1234'
 else if(r.method==='eth_getBlockByNumber')result={number:'0x1234',hash:blockHash,timestamp:toHex(timestamp),transactions:[],baseFeePerGas:'0x1',gasLimit:'0x1000000',gasUsed:'0x0',extraData:'0x'}
 else if(r.method==='eth_getTransactionReceipt'){
  const hash=r.params[0],kind=receipts.get(hash)
  const logs=kind==='swap'?[{address:POOL_MANAGER,blockHash,blockNumber:'0x1234',transactionHash:hash,transactionIndex:'0x0',logIndex:'0x0',removed:false,topics:['0x40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f',POOL_ID,'0x'+ROUTER.slice(2).padStart(64,'0')],data:encodeAbiParameters(parseAbiParameters('int128,int128,uint160,uint128,int24,uint24'),[2n*10n**18n,-100000n,1n,1n,0,500])}]:[]
  result={transactionHash:hash,blockHash,blockNumber:'0x1234',transactionIndex:'0x0',from:account,to:sent.at(-1).tx.to,cumulativeGasUsed:'0x30000',gasUsed:'0x30000',effectiveGasPrice:'0x1',status:'0x1',type:'0x2',logs,logsBloom:'0x'+'0'.repeat(512),contractAddress:null}
 }else throw Error('Unexpected RPC '+r.method)
 await route.fulfill({json:{jsonrpc:'2.0',id:r.id,result}})
})
try{
 await page.goto('http://127.0.0.1:5173');await page.getByRole('button',{name:'Connect wallet',exact:true}).click();await page.getByRole('button',{name:'Disconnect',exact:true}).waitFor()
 await page.getByRole('combobox',{name:'Trade direction'}).selectOption('USDC');await page.getByRole('heading',{name:'Buy MON with USDC'}).waitFor();await page.getByRole('textbox',{name:'USDC amount'}).fill('0.1')
 const confirm=page.getByRole('button',{name:'Confirm purchase'});assert(await confirm.isDisabled())
 await page.getByRole('button',{name:'1. Approve USDC',exact:true}).click();await page.getByRole('alert').filter({hasText:'Cancelled in wallet'}).waitFor();assert.equal(sent.length,0)
 await page.evaluate(()=>{window.__reject=false})
 await page.getByRole('button',{name:'1. Approve USDC',exact:true}).click();await page.getByRole('button',{name:'2. Authorize swap',exact:true}).waitFor();assert(await confirm.isDisabled())
 await page.getByRole('button',{name:'2. Authorize swap',exact:true}).click();await page.getByText('Approval confirmed',{exact:false}).waitFor()
 await page.getByRole('button',{name:'Get pool quote'}).click();await page.getByText('2 MON',{exact:true}).waitFor();await confirm.click()
 await page.getByText('Received 2 MON (before network fee)',{exact:false}).waitFor()
 assert.deepEqual(sent.map(s=>s.kind),['token','router','swap']);assert.deepEqual(errors,[])
 await page.getByRole('combobox',{name:'Trade direction'}).selectOption('MON');assert(await confirm.isDisabled());assert.equal(await page.getByRole('textbox',{name:'MON amount'}).inputValue(),'')
 console.log('Reverse browser passed: exact USDC approval, expiring router approval, rejection, same-pool swap, native MON receipt, direction invalidation; all wallet submissions mocked.')
}catch(e){console.error(await page.locator('.monad-trade').innerText());throw e}finally{await browser.close()}
