import fs from 'node:fs'
import {createPublicClient,createWalletClient,http,parseEther,encodeDeployData,keccak256} from '../frontend/node_modules/viem/_esm/index.js'
import {privateKeyToAccount} from '../frontend/node_modules/viem/_esm/accounts/index.js'
import {monadTestnet} from '../frontend/node_modules/viem/_esm/chains/index.js'
const root=new URL('./',import.meta.url),read=p=>JSON.parse(fs.readFileSync(new URL(p,root),'utf8')),path=new URL('results/market-testnet-deployment.json',root)
const url=process.env.MONAD_TESTNET_RPC_URL||'https://testnet-rpc.monad.xyz'
const rpc=createPublicClient({chain:monadTestnet,transport:http(url,{timeout:20000,retryCount:0})})
const artifact=read('contracts/out/StrategyMarket.sol/StrategyMarket.json'),ids=read('results/route-catalog.json').candidates.map(q=>q.pool)
const terms=[parseEther('1'),parseEther('0.1'),2592000n,parseEther('10')]
function save(r){fs.writeFileSync(path,JSON.stringify(r,null,2)+'\n')}
async function main(){
 if(await rpc.getChainId()!==10143)throw Error('Testnet only')
 const input=fs.readFileSync(new URL('../examples/monad-buy-usdc/.env.example',root),'utf8')
 const key=process.env.TESTNET_DEPLOYER_PRIVATE_KEY||input.match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1]
 if(!key)throw Error('Missing key')
 const account=privateKeyToAccount(key.startsWith('0x')?key:`0x${key}`),wallet=createWalletClient({account,chain:monadTestnet,transport:http(url,{retryCount:0})})
 let r=fs.existsSync(path)?read('results/market-testnet-deployment.json'):null
 if(r&&(r.creator.toLowerCase()!==account.address.toLowerCase()||JSON.stringify(r.poolIds)!==JSON.stringify(ids)))throw Error('Recorded config differs')
 if(!r){
  const data=encodeDeployData({abi:artifact.abi,bytecode:artifact.bytecode.object,args:[ids,...terms]})
  const gas=(await rpc.estimateGas({account:account.address,data}))*120n/100n,price=await rpc.getGasPrice()*120n/100n
  if(gas>3000000n||price>200000000000n)throw Error('Testnet fee cap')
  const hash=await wallet.deployContract({abi:artifact.abi,bytecode:artifact.bytecode.object,args:[ids,...terms],gas,maxFeePerGas:price,maxPriorityFeePerGas:0n})
  r={chainId:10143,creator:account.address,poolIds:ids,buyPriceWei:String(terms[0]),rentPriceWei:String(terms[1]),rentDurationSeconds:String(terms[2]),initialOwnershipPriceWei:String(terms[3]),transactionHash:hash,status:'submitted'};save(r)
 }
 const receipt=await rpc.waitForTransactionReceipt({hash:r.transactionHash,timeout:60000})
 if(receipt.status!=='success')throw Error('Deployment reverted')
 const address=receipt.contractAddress
 const values=await Promise.all(['owner','buyPrice','rentPrice','rentDuration','ownershipPrice','selector'].map(functionName=>rpc.readContract({address,abi:artifact.abi,functionName})))
 if(values[0].toLowerCase()!==account.address.toLowerCase()||values.slice(1,4).some((v,i)=>v!==terms[i]))throw Error('On-chain config differs')
 r={...r,status:'deployed',contractAddress:address,selectorAddress:values[5],runtimeCodeHash:keccak256(await rpc.getBytecode({address})),gasUsed:String(receipt.gasUsed),explorer:`https://testnet.monadscan.com/address/${address}`,verifiedAt:new Date().toISOString(),note:'Testnet demonstration prices. Buy grants permanent wallet access; Rent grants 30 days; Own transfers management and future revenue, preserving prior licenses. Public quotes remain free; no swaps.'};save(r);console.log(JSON.stringify(r))
}
main().catch(e=>{console.error(e.shortMessage||e.message);process.exitCode=1})
