import test from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import {fetchSelectionQuotes,selectV4} from '../src/selectionQuotes.js'
const data=JSON.parse(fs.readFileSync(new URL('../src/generated/selection.json',import.meta.url))),candidates=data.snapshot.scenarios[0].quotes
const hash='0x'+'12'.repeat(32)
function rpc(){const calls=[];return {calls,getChainId:async()=>143,getBlock:async()=>({number:50n,hash}),simulateContract:async args=>{calls.push(args);return {result:[123n,50000n]}}}}
test('0.1 MON is quoted independently in all six pools at the same block',async()=>{
 const fake=rpc(),snapshot=await fetchSelectionQuotes(candidates,'0.1','MON',fake)
 assert.equal(snapshot.scenarios[0].amountInRaw,'100000000000000000');assert.equal(fake.calls.length,6)
 for(const c of fake.calls){assert.equal(c.blockNumber,50n);assert.equal(c.args[0].exactAmount,100000000000000000n);assert.equal(c.args[0].zeroForOne,true)}
 assert.equal(snapshot.scenarios[0].outputDecimals,6)
})
test('reverse direction uses USDC decimals and clears assumptions about MON',async()=>{
 const fake=rpc(),snapshot=await fetchSelectionQuotes(candidates,'0.1','USDC',fake)
 assert.equal(snapshot.scenarios[0].amountInRaw,'100000');assert.equal(snapshot.scenarios[0].outputDecimals,18);assert.ok(fake.calls.every(c=>!c.args[0].zeroForOne))
})
test('reject invalid amount, wrong network, changed block, failed or zero quote',async()=>{
 await assert.rejects(fetchSelectionQuotes(candidates,'0','MON',rpc()))
 const wrong=rpc();wrong.getChainId=async()=>10143;await assert.rejects(fetchSelectionQuotes(candidates,'0.1','MON',wrong),/主网/)
 const reorg=rpc();reorg.getBlock=async arg=>({number:50n,hash:arg?'0x'+'34'.repeat(32):hash});await assert.rejects(fetchSelectionQuotes(candidates,'0.1','MON',reorg),/变化/)
 const zero=rpc();zero.simulateContract=async()=>({result:[0n,0n]});await assert.rejects(fetchSelectionQuotes(candidates,'0.1','MON',zero),/有效报价/)
 const failed=rpc();failed.simulateContract=async()=>{throw Error('RPC unavailable')};await assert.rejects(fetchSelectionQuotes(candidates,'0.1','MON',failed),/所有候选池/)
})

test('one failed or zero-output pool does not erase valid candidates',async()=>{
 const fake=rpc();let count=0
 fake.simulateContract=async()=>{count++;if(count===1)throw Error('revert');return {result:[count===2?0n:BigInt(count),1n]}}
 const snapshot=await fetchSelectionQuotes(candidates,'0.1','MON',fake),decision=selectV4(snapshot)
 assert.equal(snapshot.available,4);assert.equal(snapshot.candidateCount,6);assert.equal(snapshot.scenarios[0].quotes[0].status,'unavailable')
 assert.equal(snapshot.scenarios[0].quotes[1].status,'zero');assert.equal(decision.values[1],6n)
 assert.equal(decision.args[1].length,6);assert.equal(decision.args[1][0].amountOut,0n)
})
test('ties use numeric pool ID and selection compares raw outputs',async()=>{
 const snapshot=await fetchSelectionQuotes(candidates,'0.1','MON',rpc()),decision=selectV4(snapshot)
 const expected=[...candidates].sort((a,b)=>BigInt(a.pool)<BigInt(b.pool)?-1:1)[0].pool
 assert.equal(decision.values[0],expected);assert.equal(decision.tieCount,6)
 const other=structuredClone(snapshot);other.scenarios[0].quotes[5].amountOutRaw='124';other.scenarios[0].quotes[5].amountOut='misleading formatted value'
 assert.equal(selectV4(other).values[0],candidates[5].pool)
 other.scenarios[0].quotes[1].pool=other.scenarios[0].quotes[0].pool
 assert.throws(()=>selectV4(other),/重复/)
})
test('mixed protocol aggregate preserves V3 wrapped input and exact V2 reserve math',async()=>{
 const fake=rpc(),routes=data.routes.candidates
 fake.multicall=async({contracts,blockNumber,batchSize})=>{
  assert.equal(contracts.length,14);assert.equal(blockNumber,50n);assert.equal(batchSize,0)
  return contracts.map((c,i)=>{
   const q=routes[i]
   if(q.kind==='v2')return {status:'success',result:[100000000000000000000n,3000000n,0]}
   if(q.kind==='v3'){assert.equal(c.args[0].tokenIn,'0x3bd359c1119da7da1d913d1c4d2b7c461115433a');assert.equal(c.args[0].amountIn,100000000000000000n);assert.equal(c.args[0].fee,q.fee)}
   return {status:'success',result:[BigInt(i+1),0n,0,0n]}
  })
 }
 const snapshot=await fetchSelectionQuotes(routes,'0.1','MON',fake)
 assert.equal(snapshot.available,14)
 const index=routes.findIndex(q=>q.kind==='v2')
 assert.equal(snapshot.scenarios[0].quotes[index].amountOutRaw,'2988')
 assert.equal(selectV4(snapshot).values[0],routes[index].pool)
})
test('retry failed aggregate subcalls at the same pinned block',async()=>{
 const fake=rpc()
 fake.multicall=async()=>candidates.map((_,i)=>i===0?{status:'failure'}:{status:'success',result:[10n,1n]})
 fake.simulateContract=async c=>{assert.equal(c.blockNumber,50n);return {result:[20n,1n]}}
 const snapshot=await fetchSelectionQuotes(candidates,'0.1','MON',fake)
 assert.equal(snapshot.available,6);assert.equal(selectV4(snapshot).values[0],candidates[0].pool)
})
