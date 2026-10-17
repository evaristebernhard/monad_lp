// Deploy a read-only quote-selection baseline to chain 10143 only.
// Reads a local example key in-process. Never print credentials or use mainnet RPC.
import fs from 'node:fs'
import { createPublicClient, createWalletClient, http, encodeDeployData, encodeAbiParameters, parseAbiParameters, keccak256 } from '../frontend/node_modules/viem/_esm/index.js'
import { privateKeyToAccount } from '../frontend/node_modules/viem/_esm/accounts/index.js'
import { monadTestnet } from '../frontend/node_modules/viem/_esm/chains/index.js'
const base=new URL('./',import.meta.url)
const RPC=process.env.MONAD_TESTNET_RPC_URL||'https://testnet-rpc.monad.xyz'
const publicClient=createPublicClient({chain:monadTestnet,transport:http(RPC,{timeout:20000,retryCount:1})})
async function main(){
 if(await publicClient.getChainId()!==10143)throw new Error('Refusing any network except Monad testnet 10143')
 if(process.argv.includes('--deploy')&&fs.existsSync(new URL('results/testnet-deployment.json',base)))throw new Error('A testnet deployment is already recorded; refusing duplicate broadcast')
 const snapshot=JSON.parse(fs.readFileSync(new URL('results/live-v4-selection.json',base),'utf8'))
 const artifact=JSON.parse(fs.readFileSync(new URL('contracts/out/MonUsdcQuoteSelector.sol/MonUsdcQuoteSelector.json',base),'utf8'))
 const first=snapshot.scenarios[0]
 const poolIds=first.quotes.map(q=>q.pool)
 if(new Set(poolIds).size!==poolIds.length||poolIds.length!==snapshot.verifiedPoolKeys)throw new Error('Invalid universe')
 if(snapshot.scenarios.some(s=>s.quotes.length!==poolIds.length||!s.selected))throw new Error('Incomplete snapshots')
 const data=encodeDeployData({abi:artifact.abi,bytecode:artifact.bytecode.object,args:[poolIds]})
 const input=fs.readFileSync(new URL('../examples/monad-buy-usdc/.env.example',base),'utf8')
 const key=process.env.TESTNET_DEPLOYER_PRIVATE_KEY||input.match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1]
 if(!key)throw new Error('Missing local key')
 const account=privateKeyToAccount(key.startsWith('0x')?key:`0x${key}`)
 const gasEstimate=await publicClient.estimateGas({account:account.address,data})
 const gas=gasEstimate*120n/100n
 const gasPrice=await publicClient.getGasPrice()
 const maxFeePerGas=gasPrice*120n/100n
 if(gas>2000000n||maxFeePerGas>200000000000n)throw new Error('Deployment exceeds testnet fee cap')
 const balance=await publicClient.getBalance({address:account.address})
 if(balance<gas*maxFeePerGas)throw new Error('Insufficient test MON for deployment cap')
 const prepared={chainId:10143,deployer:account.address,sourceChainId:snapshot.sourceChainId,sourceBlock:snapshot.sourceBlock,sourceBlockHash:snapshot.sourceBlockHash,poolIds,algorithm:'K=1 gross quote maximum; not V2 DCAR',constructorHash:keccak256(data),gasEstimate:String(gasEstimate),gasLimit:String(gas),maxFeePerGas:String(maxFeePerGas),maximumTestMonWei:String(gas*maxFeePerGas),scope:'Read-only mainnet quote replay. Caller-supplied quotes are not authenticated. No swaps/custody.'}
 fs.writeFileSync(new URL('results/testnet-deployment-prepared.json',base),JSON.stringify(prepared,null,2)+'\n')
 if(!process.argv.includes('--deploy')){console.log(JSON.stringify({...prepared,status:'prepared_not_sent'},null,2));return}
 const wallet=createWalletClient({account,chain:monadTestnet,transport:http(RPC,{timeout:20000,retryCount:0})})
 const hash=await wallet.deployContract({abi:artifact.abi,bytecode:artifact.bytecode.object,args:[poolIds],gas,maxFeePerGas,maxPriorityFeePerGas:0n})
 // Persist the hash immediately so a timeout cannot lead to duplicate deployment.
 const deployment={...prepared,transactionHash:hash,status:'submitted',submittedAt:new Date().toISOString()}
 const resultPath=new URL('results/testnet-deployment.json',base)
 fs.writeFileSync(resultPath,JSON.stringify(deployment,null,2)+'\n')
 const receipt=await publicClient.waitForTransactionReceipt({hash,timeout:90000,pollingInterval:1500})
 if(receipt.status!=='success'||!receipt.contractAddress)throw new Error('Testnet deployment reverted')
 const address=receipt.contractAddress
 deployment.status='deployed';deployment.contractAddress=address;deployment.blockNumber=String(receipt.blockNumber);deployment.gasUsed=String(receipt.gasUsed)
 deployment.explorer=`https://testnet.monadscan.com/address/${address}`
 fs.writeFileSync(resultPath,JSON.stringify(deployment,null,2)+'\n')
 const universeHash=await publicClient.readContract({address,abi:artifact.abi,functionName:'universeHash'})
 const results=[]
 for(const scenario of snapshot.scenarios){
  const context={sourceChainId:143n,sourceBlock:BigInt(snapshot.sourceBlock),sourceBlockHash:snapshot.sourceBlockHash,tokenIn:scenario.inputToken,tokenOut:scenario.outputToken,amountIn:BigInt(scenario.amountInRaw)}
  const quotes=scenario.quotes.map(q=>({poolId:q.pool,amountOut:q.status==='quoted'?BigInt(q.amountOutRaw):0n}))
  const [poolId,amountOut,replayHash]=await publicClient.readContract({address,abi:artifact.abi,functionName:'selectBest',args:[context,quotes]})
  const localHash=keccak256(encodeAbiParameters(parseAbiParameters('bytes32,(uint256 sourceChainId,uint256 sourceBlock,bytes32 sourceBlockHash,address tokenIn,address tokenOut,uint256 amountIn),(bytes32 poolId,uint256 amountOut)[]'),[universeHash,context,quotes]))
  const matches=poolId.toLowerCase()===scenario.selected.pool.toLowerCase()&&String(amountOut)===scenario.selected.amountOutRaw&&localHash===replayHash
  if(!matches)throw new Error('On-chain selection differs from local snapshot')
  results.push({direction:scenario.direction,amountIn:scenario.amountIn,poolId,amountOutRaw:String(amountOut),replayHash,localMatches:true})
 }
 const code=await publicClient.getBytecode({address})
 deployment.runtimeCodeHash=keccak256(code);deployment.replayResults=results;deployment.quoteAuthentication=false;deployment.swapsExecuted=false
 fs.writeFileSync(resultPath,JSON.stringify(deployment,null,2)+'\n')
 console.log(JSON.stringify({status:deployment.status,chainId:10143,contractAddress:address,transactionHash:hash,gasUsed:deployment.gasUsed,replayed:results.length,allMatch:results.every(r=>r.localMatches),explorer:deployment.explorer},null,2))
}
main().catch(error=>{console.error(error.shortMessage||error.message||'Testnet operation failed');process.exitCode=1})
