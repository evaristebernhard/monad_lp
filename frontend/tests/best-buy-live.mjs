// Read-only mainnet simulation: artificial caller balance, never broadcasts.
import fs from 'node:fs'
import assert from 'node:assert/strict'
import {createPublicClient,http} from 'viem'
import {buildBestBuy,selectBestFrame,verifyBestQuote} from '../src/bestSwap.js'
const origin='http://127.0.0.1:8100',wallet='0x0000000000000000000000000000000000000123'
const rpc=createPublicClient({transport:http(origin+'/api/monad/trade-rpc',{timeout:30000,retryCount:0})})
const catalog=JSON.parse(fs.readFileSync(new URL('../../选池/results/route-catalog.json',import.meta.url))).candidates
const response=await fetch(origin+'/api/selection/quote?amount=0.1&token_in=MON'),frame=await response.json()
const selected=selectBestFrame(frame,catalog,'0.1')
await verifyBestQuote(rpc,selected)
const rows=[]
for(const route of catalog){
 const q=frame.scenarios[0].quotes.find(q=>q.pool===route.pool)
 if(q.status!=='quoted')continue
 const {minimum,...tx}=buildBestBuy({route,amountIn:10n**17n,amountOut:BigInt(q.amountOutRaw),wallet,deadline:BigInt(frame.sourceTimestamp)+600n})
 try{
  await rpc.call({...tx,gas:15000000n,stateOverride:[{address:wallet,balance:100n*10n**18n}]})
  rows.push({pool:route.pool,protocol:route.protocol,router:tx.to,simulated:true,minimum:String(minimum)})
 }catch(e){rows.push({pool:route.pool,protocol:route.protocol,simulated:false,error:e.shortMessage||e.message})}
}
fs.writeFileSync(new URL('../../output/best-buy-live-check.json',import.meta.url),JSON.stringify({sourceChainId:143,sourceBlock:frame.sourceBlock,selected:frame.selected,readOnly:true,rows},null,2))
console.log(JSON.stringify(rows))
assert.equal(rows.filter(r=>!r.simulated).length,0,'Every quoted route must simulate successfully')
