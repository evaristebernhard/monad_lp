// Read-only integration: discover storage slots using eth_call state overrides,
// then simulate the full USDC -> native MON swap. No signatures or broadcasts.
import {createPublicClient,http,encodeAbiParameters,encodeFunctionData,decodeFunctionResult,parseAbiParameters,keccak256,toHex} from 'viem'
import {quoteSwap,buildSwap,USDC,PERMIT2,ROUTER,ERC20_ABI,PERMIT2_ABI} from '../src/monadTrade.js'
import assert from 'node:assert/strict'
import {writeFile} from 'node:fs/promises'
const client=createPublicClient({transport:http(`${process.env.MONAD_API_ORIGIN||'http://127.0.0.1:8100'}/api/monad/trade-rpc`,{retryCount:0,timeout:22000})})
const account='0x1111111111111111111111111111111111111111',amount=100000n
const slot=(owner,index)=>keccak256(encodeAbiParameters(parseAbiParameters('address,uint256'),[owner,BigInt(index)]))
const word=n=>toHex(n,{size:32})
const block=await client.getBlock()
const expiry=Number(block.timestamp)+600
async function probe(to,abi,args,key,value){
 const data=encodeFunctionData({abi,functionName:to===PERMIT2?'allowance':args.length===1?'balanceOf':'allowance',args})
 const result=await client.request({method:'eth_call',params:[{to,data},'latest',{[to]:{stateDiff:{[key]:word(value)}}}]})
 return decodeFunctionResult({abi,functionName:to===PERMIT2?'allowance':args.length===1?'balanceOf':'allowance',data:result})
}
async function discover(limit,fn,match){
 for(let i=0;i<limit;i+=3){const values=await Promise.all(Array.from({length:Math.min(3,limit-i)},async(_,j)=>{try{return{index:i+j,value:await fn(i+j)}}catch{return null}}));const found=values.find(v=>v&&match(v.value));if(found)return found.index}
 throw Error('Cannot locate overridden storage; no transaction has been sent.')
}
const balanceIndex=await discover(30,i=>probe(USDC,ERC20_ABI,[account],slot(account,i),910001n),value=>value===910001n)
console.log('Found USDC balance storage index',balanceIndex)
const tokenIndex=await discover(30,i=>probe(USDC,ERC20_ABI,[account,PERMIT2],slot(PERMIT2,slot(account,i)),920001n),value=>value===920001n)
console.log('Found USDC allowance storage index',tokenIndex)
const packed=amount|(BigInt(expiry)<<160n)
const permitIndex=await discover(6,i=>probe(PERMIT2,PERMIT2_ABI,[account,USDC,ROUTER],slot(ROUTER,slot(USDC,slot(account,i))),packed),value=>value[0]===amount&&value[1]===expiry)
console.log('Found Permit2 allowance storage index',permitIndex)
const quote=await quoteSwap(amount,50,false,client)
const current=await client.getBlock()
const tx=buildSwap({account,amountIn:amount,amountOutMinimum:quote.minimum,zeroForOne:false,deadline:current.timestamp+120n})
const overrides={
 [account]:{balance:toHex(100n*10n**18n)},
 [USDC]:{stateDiff:{[slot(account,balanceIndex)]:word(amount),[slot(PERMIT2,slot(account,tokenIndex))]:word(amount)}},
 [PERMIT2]:{stateDiff:{[slot(ROUTER,slot(USDC,slot(account,permitIndex)))]:word(packed)}},
}
const request={from:account,to:tx.to,data:tx.data,value:toHex(tx.value)}
const result=await client.request({method:'eth_call',params:[request,'latest',overrides]})
assert.equal(result,'0x')
// With no authorization, the same calldata must fail. Do not bypass approvals.
const unauthorized=structuredClone(overrides)
unauthorized[PERMIT2].stateDiff[slot(ROUTER,slot(USDC,slot(account,permitIndex)))]=word(0n)
await assert.rejects(()=>client.request({method:'eth_call',params:[request,'latest',unauthorized]}))
console.log('Full reverse swap simulated successfully; missing authorization rejected; no funds moved.')
await writeFile(new URL('../../output/monad-trade-reverse-live-check.json',import.meta.url),JSON.stringify({readOnly:true,broadcast:false,syntheticBalanceAndAllowanceOverrides:true,quote,simulation:result,missingAuthorizationRejected:true},(_,v)=>typeof v==='bigint'?v.toString():v,2))
