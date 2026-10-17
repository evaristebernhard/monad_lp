import {chromium} from 'playwright'
import assert from 'node:assert/strict'
import {encodeAbiParameters,parseAbiParameters,toFunctionSelector} from '../node_modules/viem/_esm/index.js'
const origin=process.env.MONAD_WEB_ORIGIN||'http://127.0.0.1:5173'
const browser=await chromium.launch({headless:true})
const failures=[]
try{
  const page=await browser.newPage({viewport:{width:1440,height:900}})
  page.on('pageerror',e=>failures.push(e.message))
  await page.goto(origin+'/#ai',{waitUntil:'domcontentloaded'})
  await page.getByRole('heading',{name:/ONE START/}).waitFor({timeout:15000})
  assert.equal(await page.getByRole('row').count(),7)
  const navigation=await page.locator('.side-nav').innerText()
  assert(navigation.includes('Wallet listener'))
  assert(navigation.includes('最优报价'))
  assert(navigation.includes('AI 交易'))
  assert(!navigation.includes('模型研究'))
  await page.screenshot({path:'/tmp/ai-autosession-desktop.png',fullPage:true})
  await page.setViewportSize({width:390,height:844})
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false)
  await page.screenshot({path:'/tmp/ai-autosession-mobile.png',fullPage:true})
  await page.close()
  const user='0x41d5D3B5f2e497d1F4bCF767ffac9DDAb85D2b79'
  const connected=await browser.newPage({viewport:{width:1280,height:860}})
  connected.on('pageerror',e=>failures.push(e.message))
  await connected.addInitScript(({user})=>{
    window.ethereum={request:async({method})=>{
      if(method==='eth_chainId')return '0x279f'
      if(method==='eth_accounts')return []
      if(method==='eth_requestAccounts')return [user]
      throw Error('Unexpected wallet method '+method)
    },on(){},removeListener(){}}
  },{user})
  const sessionSelector=toFunctionSelector('function sessions(address) view returns(uint64,uint64,uint64,uint8,bool)')
  const sessionZero=encodeAbiParameters(parseAbiParameters('uint64,uint64,uint64,uint8,bool'),
    [0n,0n,0n,0,false])
  const numberZero=encodeAbiParameters(parseAbiParameters('uint256'),[0n])
  await connected.route('**/api/selection/testnet-rpc',async route=>{
    const b=JSON.parse(route.request().postData()||'{}')
    let result
    if(b.method==='eth_chainId')result='0x279f'
    else if(b.method==='eth_getBalance')result='0x4563918244f40000'
    else if(b.method==='eth_call')result=(b.params?.[0]?.data||'').startsWith(sessionSelector)?sessionZero:numberZero
    else if(b.method==='eth_blockNumber')result='0x4472'
    else if(b.method==='eth_getLogs')result=[]
    else throw Error('Unexpected RPC '+b.method)
    await route.fulfill({status:200,contentType:'application/json',
      body:JSON.stringify({jsonrpc:'2.0',id:b.id,result})})
  })
  await connected.goto(origin+'/#ai',{waitUntil:'domcontentloaded'})
  await connected.getByRole('button',{name:/连接 MetaMask/}).first().click()
  await connected.getByRole('button',{name:/授权本次演示额度/}).waitFor({timeout:20000})
  assert((await connected.locator('.ows-wallet').first().innerText()).includes('0x41d5'))
  assert.deepEqual(failures,[])
  console.log(JSON.stringify({status:'passed',desktop:true,mobile:true,connectedWallet:true,
    navLabels:true,removedResearchNav:true,errors:failures}))
}finally{await browser.close()}
