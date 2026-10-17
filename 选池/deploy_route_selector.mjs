import fs from 'node:fs'
import {createPublicClient,createWalletClient,http,keccak256,encodeDeployData} from '../frontend/node_modules/viem/_esm/index.js'
import {privateKeyToAccount} from '../frontend/node_modules/viem/_esm/accounts/index.js'
import {monadTestnet} from '../frontend/node_modules/viem/_esm/chains/index.js'
const root=new URL('./',import.meta.url),read=p=>JSON.parse(fs.readFileSync(new URL(p,root),'utf8')),path=new URL('results/route-testnet-deployment.json',root)
const rpc=createPublicClient({chain:monadTestnet,transport:http('https://testnet-rpc.monad.xyz',{timeout:20000,retryCount:0})})
async function main(){
 if(await rpc.getChainId()!==10143)throw Error('Wrong chain')
 const artifact=read('contracts/out/MonUsdcRouteSelector.sol/MonUsdcRouteSelector.json'),ids=read('results/route-catalog.json').candidates.map(q=>q.pool)
 if(new Set(ids).size!==ids.length||ids.length>32)throw Error('Bad universe')
 let record=fs.existsSync(path)?read('results/route-testnet-deployment.json'):null
 if(record&&JSON.stringify(record.poolIds)!==JSON.stringify(ids))throw Error('Existing deployment has a different universe')
 if(!record){
 const env=fs.readFileSync(new URL('../examples/monad-buy-usdc/.env.example',root),'utf8'),key=env.match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1]
 const account=privateKeyToAccount(key.startsWith('0x')?key:`0x${key}`),wallet=createWalletClient({account,chain:monadTestnet,transport:http('https://testnet-rpc.monad.xyz')})
 const estimate=await rpc.estimateGas({account:account.address,data:encodeDeployData({abi:artifact.abi,bytecode:artifact.bytecode.object,args:[ids]})}),gas=estimate*120n/100n,fee=await rpc.getGasPrice()*120n/100n
 if(gas>2000000n||fee>200000000000n)throw Error('Testnet fee cap')
 const hash=await wallet.deployContract({abi:artifact.abi,bytecode:artifact.bytecode.object,args:[ids],gas,maxFeePerGas:fee,maxPriorityFeePerGas:0n})
 record={chainId:10143,poolIds:ids,transactionHash:hash,status:'submitted'};fs.writeFileSync(path,JSON.stringify(record,null,2))
 }
 const receipt=await rpc.waitForTransactionReceipt({hash:record.transactionHash,timeout:90000})
 if(receipt.status!=='success')throw Error('Deployment reverted')
 const address=receipt.contractAddress
 const allowed=await Promise.all(ids.map(id=>rpc.readContract({address,abi:artifact.abi,functionName:'allowedPool',args:[id]})))
 if(allowed.some(v=>!v))throw Error('Universe mismatch')
 record={...record,contractAddress:address,status:'deployed',gasUsed:String(receipt.gasUsed),runtimeCodeHash:keccak256(await rpc.getBytecode({address}))}
 fs.writeFileSync(path,JSON.stringify(record,null,2)+'\n');console.log(JSON.stringify({address,gasUsed:record.gasUsed,pools:ids.length,chainId:10143}))
}
main().catch(e=>{console.error(e.shortMessage||e.message);process.exitCode=1})
