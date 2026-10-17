import {chromium} from 'playwright'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import {encodeAbiParameters,parseAbiParameters,toFunctionSelector,encodeEventTopics,parseAbi} from 'viem'
const origin=process.env.MONAD_WEB_ORIGIN||'http://127.0.0.1:5173'
const browser=await chromium.launch({headless:true})
const failures=[]
const config={...JSON.parse(fs.readFileSync(new URL('../public/ai/session-config.json',import.meta.url))),maxUSDC:'3',maxWMON:'3',buyUSDC:'1',sellWMON:'1',executionRules:{enabled:true}}
const user='0x41d5D3B5f2e497d1F4bCF767ffac9DDAb85D2b79'
const hash=n=>'0x'+n.toString(16).padStart(64,'0')
const hex=n=>'0x'+BigInt(n).toString(16)
const startedAt=Math.floor(Date.now()/1000)-10
let confirmed=0,preparing=false,prepReceipts=0
const abi=parseAbi([
  'event Decision(address indexed wallet,uint64 indexed sessionId,uint8 indexed round,uint8 action,bytes32 inputHash,uint256 amountIn,uint256 amountOut,int256[3] scores)',
  'event MarketInput(address indexed wallet,uint64 indexed sessionId,uint8 indexed round,uint64 candleEnd,bytes32 sourceHash,int32[12] features,uint8 predicted)',
  'event ExecutionRule(address indexed wallet,uint64 indexed sessionId,uint8 indexed round,uint8 reason)'
])
function logs(name){
  return Array.from({length:confirmed},(_,round)=>{
    const action=[1,0,2][round]
    const topics=encodeEventTopics({abi,eventName:name,args:{wallet:user,sessionId:1n,round}})
    const data=name==='Decision'?encodeAbiParameters(parseAbiParameters('uint8,bytes32,uint256,uint256,int256[3]'),[
      action,hash(9),action===1?1000000n:action===2?1000000000000000000n:0n,
      action===1?250000000000000000n:action===2?3900000n:0n,[10n,0n,0n]
    ]):name==='MarketInput'?encodeAbiParameters(parseAbiParameters('uint64,bytes32,int32[12],uint8'),[
      BigInt(Math.floor(startedAt/3600)*3600),hash(8),Array(12).fill(0),0
    ]):encodeAbiParameters(parseAbiParameters('uint8'),[action===1?1:action===2?2:0])
    return {address:config.contract,topics,data,blockNumber:hex(round===0?105:110+round),blockHash:hash(105+round),
      transactionHash:hash(20+round),transactionIndex:'0x0',logIndex:hex(round*3),removed:false}
  })
}
try{
  const page=await browser.newPage({viewport:{width:1440,height:1000}})
  page.on('pageerror',e=>failures.push(e.message))
  await page.addInitScript(({user})=>{
    window.testWallet={submitted:0,transactions:[]}
    window.ethereum={request:async({method,params})=>{
      if(method==='eth_chainId')return '0x279f'
      if(method==='eth_accounts'||method==='eth_requestAccounts')return [user]
      if(method==='eth_sendTransaction'){window.testWallet.submitted++;window.testWallet.transactions.push(params[0]);return '0x'+'11'.repeat(32)}
      throw Error('Unexpected wallet method '+method)
    },on(){},removeListener(){}}
  },{user})
  await page.route('**/ai/session-config.json',r=>r.fulfill({json:config}))
  await page.route('**/api/ai/rpc-live',r=>r.fulfill({json:{observedAt:Date.now(),input:Array(12).fill(0),
    tick:{action:0,scores:['10','0','0'],block:'12345',price:.0226,volume:5000,sampledAt:Date.now()},ticks:[]}}))
  await page.route('**/api/selection/testnet-rpc',async route=>{
    const b=JSON.parse(route.request().postData()||'{}')
    let result
    if(b.method==='eth_chainId')result='0x279f'
    else if(b.method==='eth_blockNumber')result=hex(confirmed>=3?115:110)
    else if(b.method==='eth_getBalance')result=hex(2n*10n**18n)
    else if(b.method==='eth_getBlockByNumber'){
      const number=b.params[0]==='latest'?BigInt(confirmed>=3?115:110):BigInt(b.params[0])
      result={number:hex(number),timestamp:hex(BigInt(startedAt)-100n+number),hash:hash(Number(number)),transactions:[]}
    }else if(b.method==='eth_getLogs'){
      const topic=b.params[0].topics[0]
      const name=['Decision','MarketInput','ExecutionRule'].find(name=>encodeEventTopics({abi,eventName:name})[0]===topic)
      result=logs(name).filter(l=>BigInt(l.blockNumber)>=BigInt(b.params[0].fromBlock)&&BigInt(l.blockNumber)<=BigInt(b.params[0].toBlock))
    }else if(b.method==='eth_getTransactionReceipt'){
      prepReceipts++
      result={transactionHash:b.params[0],transactionIndex:'0x0',blockHash:hash(110),blockNumber:hex(110),from:user,to:config.wmon,
        cumulativeGasUsed:'0x5208',gasUsed:'0x5208',contractAddress:null,logs:[],logsBloom:'0x'+'00'.repeat(256),status:'0x1',effectiveGasPrice:'0x1',type:'0x2'}
    }else if(b.method==='eth_call'){
      const data=b.params[0].data
      if(data.startsWith(toFunctionSelector('sessions(address)')))result=encodeAbiParameters(parseAbiParameters('uint64,uint64,uint64,uint8,bool'),
        confirmed?[1n,BigInt(startedAt),BigInt(startedAt+confirmed*10),confirmed,true]:[0n,0n,0n,0,false])
      else if(data.startsWith(toFunctionSelector('previewAction(address,int32[12])')))result=encodeAbiParameters(parseAbiParameters('uint8,uint8,uint8'),[0,1,1])
      else {
        const allowance=data.startsWith(toFunctionSelector('allowance(address,address)'))
        const value=allowance?(preparing&&prepReceipts<3?0n:10n**20n):
          b.params[0].to.toLowerCase()===config.usdc.toLowerCase()?36832622n:
          preparing&&prepReceipts===0?36506000000000000n:3n*10n**18n
        result=encodeAbiParameters(parseAbiParameters('uint256'),[value])
      }
    }else throw Error('Unexpected RPC '+b.method)
    await route.fulfill({json:{jsonrpc:'2.0',id:b.id,result}})
  })
  await page.goto(origin+'/#ai',{waitUntil:'domcontentloaded'})
  await page.getByRole('heading',{name:/ONE START/}).waitFor()
  await page.getByRole('button',{name:/START AUTO TRADING/}).waitFor()
  assert.equal(await page.locator('.ows-action strong').textContent(),'BUY','preview should show executable action')
  assert.match(await page.locator('.ows-model-signal').textContent(),/HOLD/,'original prediction stays HOLD')
  assert.equal(await page.locator('.ows-data-details').getAttribute('open'),null)
  assert.equal(await page.locator('.ows-samples').getAttribute('open'),null)
  assert.equal(await page.locator('.ows-progress').getByText('RPC',{exact:true}).count(),0)
  await page.getByRole('button',{name:/START AUTO TRADING/}).click()
  await page.waitForFunction(()=>window.testWallet.submitted===1)
  assert.equal(await page.locator('.ows-ledger:not(.ows-samples) tbody tr').count(),0,'broadcast alone is not a confirmed fill')
  confirmed=1
  await page.waitForFunction(()=>document.querySelectorAll('.ows-ledger:not(.ows-samples) tbody tr').length===1)
  const buy=page.locator('.ows-ledger:not(.ows-samples) tbody tr').first()
  assert.match(await buy.textContent(),/BUY.*1 USDC.*0.25 MON.*已确认/)
  confirmed=3
  await page.waitForFunction(()=>document.querySelectorAll('.ows-ledger:not(.ows-samples) tbody tr').length===3)
  assert.equal(await page.locator('.ows-action strong').textContent(),'SELL')
  assert.match(await page.locator('.ows-ledger:not(.ows-samples) tbody tr').last().textContent(),/SELL.*1 MON.*3.9 USDC/)
  assert.match(await page.locator('.ows-model-signal').textContent(),/HOLD/)
  await page.screenshot({path:'/tmp/ai-trading-desktop.png',fullPage:true})
  await page.setViewportSize({width:390,height:844})
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false)
  await page.screenshot({path:'/tmp/ai-trading-mobile.png',fullPage:true})
  preparing=true;confirmed=0
  await page.reload({waitUntil:'domcontentloaded'})
  await page.getByRole('button',{name:/准备并授权/}).click()
  await page.getByRole('button',{name:/START AUTO TRADING/}).waitFor()
  const preparations=await page.evaluate(()=>window.testWallet.transactions)
  assert.equal(preparations.length,3,'prepare WMON, approve USDC, approve WMON')
  assert.equal(preparations[0].to.toLowerCase(),config.wmon.toLowerCase())
  assert.equal(preparations[0].data,'0xd0e30db0')
  assert.equal(BigInt(preparations[0].value),3000000000000000000n-36506000000000000n)
  assert.equal(BigInt('0x'+preparations[1].data.slice(-64)),3000000n)
  assert.equal(BigInt('0x'+preparations[2].data.slice(-64)),3000000000000000000n)
  assert.equal(await page.getByRole('button',{name:/START AUTO TRADING/}).isEnabled(),true)
  assert.deepEqual(failures,[])
  console.log(JSON.stringify({status:'passed',actualActionShown:true,unchangedModelVisible:true,broadcastNotConfirmed:true,
    buyAndSellAmounts:true,unitInputs:true,wrappedInventoryPreparation:true,sixExecutionRounds:true,technicalDetailsCollapsed:true,mobile:true,errors:failures}))
}finally{await browser.close()}
