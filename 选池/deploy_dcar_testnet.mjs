import fs from 'node:fs'
import {createPublicClient,createWalletClient,http,keccak256} from '../frontend/node_modules/viem/_esm/index.js'
import {privateKeyToAccount} from '../frontend/node_modules/viem/_esm/accounts/index.js'
import {monadTestnet} from '../frontend/node_modules/viem/_esm/chains/index.js'
import {buildInput,solveDcar,json} from '../frontend/src/poolSelection.js'
const root=new URL('./',import.meta.url),read=p=>JSON.parse(fs.readFileSync(new URL(p,root),'utf8')),path=new URL('results/dcar-testnet-deployment.json',root)
const client=createPublicClient({chain:monadTestnet,transport:http('https://testnet-rpc.monad.xyz',{timeout:30000})})
async function main(){
 if(await client.getChainId()!==10143)throw Error('Wrong chain')
 const artifact=read('contracts/out/DcarV2Router.sol/DcarV2Router.json')
 let record=fs.existsSync(path)?read('results/dcar-testnet-deployment.json'):null
 if(!record){
 const text=fs.readFileSync(new URL('../examples/monad-buy-usdc/.env.example',root),'utf8'),key=text.match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1]
 const account=privateKeyToAccount(key.startsWith('0x')?key:`0x${key}`),wallet=createWalletClient({account,chain:monadTestnet,transport:http('https://testnet-rpc.monad.xyz')})
 const estimate=await client.estimateGas({account:account.address,data:artifact.bytecode.object}),gas=estimate*120n/100n,fee=await client.getGasPrice()*120n/100n
 if(gas>2000000n||fee>200000000000n)throw Error('Fee cap')
 const hash=await wallet.deployContract({abi:artifact.abi,bytecode:artifact.bytecode.object,gas,maxFeePerGas:fee,maxPriorityFeePerGas:0n})
 record={chainId:10143,transactionHash:hash,status:'submitted'};fs.writeFileSync(path,json(record))
 }
 const receipt=await client.waitForTransactionReceipt({hash:record.transactionHash,timeout:90000})
 if(receipt.status!=='success')throw Error('Deployment reverted')
 record={...record,status:'deployed',contractAddress:receipt.contractAddress,gasUsed:String(receipt.gasUsed),runtimeCodeHash:keccak256(await client.getBytecode({address:receipt.contractAddress})),replays:[]}
 fs.writeFileSync(path,json(record))
 for(const [amount,k]of [['10000',3],['100',1],['1000',2]]){
 const args=buildInput(read('monad_pools_example.json'),amount,k),local=solveDcar(args),remote=await client.readContract({address:record.contractAddress,abi:artifact.abi,functionName:'solve',args,gas:28000000n})
 if(json(local)!==json(remote))throw Error('Replay mismatch')
 record.replays.push({amount,maxSplits:k,match:true,result:remote});fs.writeFileSync(path,json(record))
 }
 console.log(json(record))
}
main().catch(e=>{console.error(e.shortMessage||e.message);process.exitCode=1})
