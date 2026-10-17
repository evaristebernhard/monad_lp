import test from 'node:test'
import assert from 'node:assert/strict'
import { decodeFunctionData, decodeAbiParameters, parseAbiParameters, keccak256 } from 'viem'
import { POOL_ID, POOL_KEY, KEY_TYPE, ZERO, USDC, ROUTER, ROUTER_ABI, parseMon, parseAmount, PERMIT2, ERC20_ABI, PERMIT2_ABI, buildApproval, approvalStep, minimumOutput, buildSwap, isFresh, ensureMonad } from '../src/monadTrade.js'

test('MON amount uses exact 18-decimal integers and rejects rounding, exponent and zero', () => {
 assert.equal(parseMon('1.000000000000000001'),1000000000000000001n)
 for(const amount of ['0','-1','1e3','NaN','0.0000000000000000001','1.1234567890123456789'])assert.throws(()=>parseMon(amount))
 assert.equal(minimumOutput(23197n,50),23081n)
 assert.throws(()=>minimumOutput(0n,50));assert.throws(()=>minimumOutput(23197n,10000))
})
test('exact selected pool, minimum received, native payment and refund are encoded locally',()=>{
 const tx=buildSwap({account:'0x1111111111111111111111111111111111111111',amountIn:10n**18n,amountOutMinimum:23081n,deadline:2000000000n})
 assert.equal(tx.to,ROUTER);assert.equal(tx.value,10n**18n)
 const decoded=decodeFunctionData({abi:ROUTER_ABI,data:tx.data})
 assert.equal(decoded.functionName,'execute');assert.equal(decoded.args[0],'0x1004');assert.equal(decoded.args[2],2000000000n)
 const [actions,params]=decodeAbiParameters(parseAbiParameters('bytes,bytes[]'),decoded.args[1][0])
 assert.equal(actions,'0x060c0f')
 const [swap]=decodeAbiParameters(parseAbiParameters(`(${KEY_TYPE} poolKey,bool zeroForOne,uint128 amountIn,uint128 amountOutMinimum,bytes hookData)`),params[0])
 assert.deepEqual({...swap.poolKey,currency1:swap.poolKey.currency1.toLowerCase()},POOL_KEY)
 assert.equal(keccak256('0x'+params[0].slice(2+64,2+64+5*64)),POOL_ID)
 assert.equal(swap.zeroForOne,true);assert.equal(swap.amountIn,tx.value);assert.equal(swap.amountOutMinimum,23081n)
 assert.deepEqual(decodeAbiParameters(parseAbiParameters('address,uint256'),params[1]),[ZERO,10n**18n])
 assert.equal(decodeAbiParameters(parseAbiParameters('address,uint256'),params[2])[0].toLowerCase(),USDC)
 const refund=decodeAbiParameters(parseAbiParameters('address,address,uint256'),decoded.args[1][1])
 assert.deepEqual(refund,[ZERO,'0x0000000000000000000000000000000000000001',0n])
})
test('expired quotes and a wallet which refuses chain switching are rejected',async()=>{
 assert(isFresh({receivedAt:1000},2000));assert(!isFresh({receivedAt:1000},31000))
 const calls=[];let network='0x1'
 const provider={request:async r=>{calls.push(r.method);if(r.method==='eth_chainId')return network;if(r.method==='wallet_switchEthereumChain'){network='0x8f';return null}}}
 await ensureMonad(provider);assert(calls.includes('wallet_switchEthereumChain'))
 await assert.rejects(()=>ensureMonad({request:async r=>r.method==='eth_chainId'?'0x1':null}),/Switch your wallet/)
})

test('reverse swap pays USDC, receives native MON and sends no native input',()=>{
 const tx=buildSwap({account:'0x1111111111111111111111111111111111111111',amountIn:100000n,amountOutMinimum:4n*10n**18n,deadline:2000000000n,zeroForOne:false})
 assert.equal(tx.value,0n)
 const decoded=decodeFunctionData({abi:ROUTER_ABI,data:tx.data})
 const [,params]=decodeAbiParameters(parseAbiParameters('bytes,bytes[]'),decoded.args[1][0])
 const [swap]=decodeAbiParameters(parseAbiParameters(`(${KEY_TYPE} poolKey,bool zeroForOne,uint128 amountIn,uint128 amountOutMinimum,bytes hookData)`),params[0])
 assert.equal(swap.zeroForOne,false);assert.equal(swap.amountIn,100000n);assert.equal(swap.amountOutMinimum,4n*10n**18n)
 assert.equal(decodeAbiParameters(parseAbiParameters('address,uint256'),params[1])[0].toLowerCase(),USDC)
 assert.deepEqual(decodeAbiParameters(parseAbiParameters('address,uint256'),params[2]),[ZERO,4n*10n**18n])
 assert.equal(parseAmount('0.100001','USDC'),100001n);assert.throws(()=>parseAmount('0.1000001','USDC'))
})
test('approvals are exact, router-specific, expire in ten minutes and track changed amounts',()=>{
 const account='0x1111111111111111111111111111111111111111'
 const token=buildApproval({account,amount:100000n,step:'token',timestamp:1000n})
 assert.equal(token.to,USDC);assert.equal(token.value,0n)
 const first=decodeFunctionData({abi:ERC20_ABI,data:token.data})
 assert.equal(first.args[0].toLowerCase(),PERMIT2);assert.equal(first.args[1],100000n)
 const router=buildApproval({account,amount:100000n,step:'router',timestamp:1000n})
 assert.equal(router.to,PERMIT2)
 const second=decodeFunctionData({abi:PERMIT2_ABI,data:router.data})
 assert.equal(second.args[0].toLowerCase(),USDC);assert.equal(second.args[1].toLowerCase(),ROUTER)
 assert.equal(second.args[2],100000n);assert.equal(second.args[3],1600)
 assert.equal(approvalStep(null,100000n,1000),'loading')
 assert.equal(approvalStep({erc20:0n,router:0n,expiration:0},100000n,1000),'token')
 assert.equal(approvalStep({erc20:100000n,router:0n,expiration:0},100000n,1000),'router')
 assert.equal(approvalStep({erc20:100000n,router:100000n,expiration:1600},100000n,1000),'ready')
 assert.equal(approvalStep({erc20:100000n,router:100000n,expiration:1001},100000n,1000),'router')
 assert.equal(approvalStep({erc20:100000n,router:100000n,expiration:1600},100001n,1000),'token')
})
