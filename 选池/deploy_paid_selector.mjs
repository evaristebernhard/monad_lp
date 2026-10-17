import fs from 'node:fs'
import {createPublicClient,createWalletClient,http,parseEther,decodeEventLog} from '../frontend/node_modules/viem/_esm/index.js'
import {privateKeyToAccount} from '../frontend/node_modules/viem/_esm/accounts/index.js'
import {monadTestnet} from '../frontend/node_modules/viem/_esm/chains/index.js'
const root=new URL('./',import.meta.url),read=p=>JSON.parse(fs.readFileSync(new URL(p,root),'utf8'))
const path=new URL('results/paid-testnet-deployment.json',root)
const rpcUrl=process.env.MONAD_TESTNET_RPC_URL||'https://testnet-rpc.monad.xyz'
const rpc=createPublicClient({chain:monadTestnet,transport:http(rpcUrl,{timeout:20000,retryCount:0})})
const artifact=read('contracts/out/PaidMonUsdcRouteSelector.sol/PaidMonUsdcRouteSelector.json')
const ids=read('results/route-catalog.json').candidates.map(q=>q.pool),fee=parseEther('0.01')
function save(r){fs.writeFileSync(path,JSON.stringify(r,null,2)+'\n')}
async function main(){
 if(await rpc.getChainId()!==10143)throw Error('Testnet 10143 only')
 const env=fs.readFileSync(new URL('../examples/monad-buy-usdc/.env.example',root),'utf8')
 const key=process.env.TESTNET_DEPLOYER_PRIVATE_KEY||env.match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1]
 if(!key)throw Error('Missing testnet key')
 const account=privateKeyToAccount(key.startsWith('0x')?key:`0x${key}`)
 const wallet=createWalletClient({account,chain:monadTestnet,transport:http(rpcUrl,{retryCount:0})})
 let r=fs.existsSync(path)?read('results/paid-testnet-deployment.json'):null
 if(r&&(r.creator.toLowerCase()!==account.address.toLowerCase()||JSON.stringify(r.poolIds)!==JSON.stringify(ids)||r.serviceFeeWei!==String(fee)))throw Error('Existing deployment configuration differs')
 const price=await rpc.getGasPrice()*120n/100n
 if(price>200000000000n)throw Error('Testnet gas price cap')
 if(!r){
  const hash=await wallet.deployContract({abi:artifact.abi,bytecode:artifact.bytecode.object,args:[ids,fee],gas:2000000n,maxFeePerGas:price,maxPriorityFeePerGas:0n})
  r={chainId:10143,creator:account.address,poolIds:ids,serviceFeeWei:String(fee),transactionHash:hash,status:'submitted'};save(r)
 }
 const receipt=await rpc.waitForTransactionReceipt({hash:r.transactionHash,timeout:60000})
 if(receipt.status!=='success')throw Error('Deployment reverted')
 r.contractAddress=receipt.contractAddress;r.status='deployed';save(r)
 const address=r.contractAddress
 if((await rpc.readContract({address,abi:artifact.abi,functionName:'creator'})).toLowerCase()!==account.address.toLowerCase())throw Error('Creator mismatch')
 if(await rpc.readContract({address,abi:artifact.abi,functionName:'serviceFee'})!==fee)throw Error('Fee mismatch')
 if(!r.paymentTransactionHash){
  const context={sourceChainId:143n,sourceBlock:1n,sourceBlockHash:'0x'+'01'.padStart(64,'0'),tokenIn:'0x0000000000000000000000000000000000000000',tokenOut:'0x754704Bc059F8C67012fEd69BC8A327a5aafb603',amountIn:1n}
  const quotes=ids.map((poolId,i)=>({poolId,amountOut:BigInt(i+1)}))
  const args=[context,quotes]
  await rpc.simulateContract({account,address,abi:artifact.abi,functionName:'selectAndPay',args,value:fee})
  r.balanceBeforeWei=String(await rpc.getBalance({address:account.address}))
  r.paymentTransactionHash=await wallet.writeContract({address,abi:artifact.abi,functionName:'selectAndPay',args,value:fee,gas:500000n,maxFeePerGas:price,maxPriorityFeePerGas:0n});save(r)
 }
 const payment=await rpc.waitForTransactionReceipt({hash:r.paymentTransactionHash,timeout:60000})
 if(payment.status!=='success')throw Error('Payment reverted')
 const event=payment.logs.flatMap(log=>{try{return [decodeEventLog({abi:artifact.abi,data:log.data,topics:log.topics})]}catch{return []}}).find(e=>e.eventName==='SelectionPaid')
 if(!event||event.args.fee!==fee||event.args.recipient.toLowerCase()!==r.creator.toLowerCase())throw Error('Payment event mismatch')
 r.paymentVerified=true;r.paymentGasUsed=String(payment.gasUsed)
 r.contractBalanceWei=String(await rpc.getBalance({address}))
 r.balanceAfterWei=String(await rpc.getBalance({address:account.address}))
 r.note='Synthetic quote replay. Deployer is also caller: service fee returns to the same address; only gas is spent. No swaps.'
 r.explorer=`https://testnet.monadscan.com/address/${address}`;save(r)
 console.log(JSON.stringify(r))
}
main().catch(e=>{console.error(e.shortMessage||e.message);process.exitCode=1})
