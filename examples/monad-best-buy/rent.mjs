// Optional and independent TESTNET paid selector call, not a swap.
import fs from 'node:fs'
import {createPublicClient,createWalletClient,http,formatUnits,encodeFunctionData,keccak256} from 'viem'
import {privateKeyToAccount} from 'viem/accounts'
import {monadTestnet} from 'viem/chains'
import {selectBestFrame} from './bestSwap.mjs'
const send=process.argv.includes('--send'),at=process.argv.indexOf('--wallet')
if(send&&!/^(0x)?[0-9a-fA-F]{64}$/.test(process.env.PRIVATE_KEY||''))throw Error('请在本机配置 PRIVATE_KEY')
const key=process.env.PRIVATE_KEY||'',account=send?privateKeyToAccount(key.startsWith('0x')?key:'0x'+key):null
const who=account?.address||process.argv[at+1]
if(at===-1&&!account)throw Error('预览请提供 --wallet；发送需要 --send 和本地 PRIVATE_KEY')
const config=JSON.parse(fs.readFileSync(new URL('./rental.json',import.meta.url))),catalog=JSON.parse(fs.readFileSync(new URL('./catalog.json',import.meta.url))).candidates
const url=process.env.MONAD_TESTNET_RPC_URL||'https://testnet-rpc.monad.xyz'
const rpc=createPublicClient({chain:monadTestnet,transport:http(url,{timeout:20000,retryCount:0})})
if(await rpc.getChainId()!==10143)throw Error('租金演示必须使用测试网 10143')
const origin=(process.env.MONAD_API_URL||'https://lp.chayu123.me').replace(/\/$/,'')
const response=await fetch(origin+'/api/selection/quote?amount=0.1&token_in=MON',{signal:AbortSignal.timeout(20000)})
if(!response.ok)throw Error('报价 API HTTP '+response.status)
const frame=await response.json(),quote=selectBestFrame(frame,catalog,'0.1')
const [fee,creator]=await Promise.all(['serviceFee','creator'].map(functionName=>rpc.readContract({address:config.address,abi:config.abi,functionName})))
if(String(fee)!==config.feeWei||creator.toLowerCase()!==config.creator.toLowerCase())throw Error('收费配置不匹配')
const c={sourceChainId:143n,sourceBlock:BigInt(frame.sourceBlock),sourceBlockHash:frame.sourceBlockHash,tokenIn:frame.scenarios[0].inputToken,tokenOut:frame.scenarios[0].outputToken,amountIn:quote.amountIn}
const quotes=frame.scenarios[0].quotes.map(q=>({poolId:q.pool,amountOut:q.status==='quoted'?BigInt(q.amountOutRaw):0n}))
const tx={to:config.address,value:fee,data:encodeFunctionData({abi:config.abi,functionName:'selectAndPay',args:[c,quotes]})}
console.log(JSON.stringify({network:'Monad TESTNET 10143',contract:config.address,recipient:creator,feeTestMON:formatUnits(fee,18),swap:false,broadcast:false},null,2))
if(send){
 const recordPath=new URL('./last-rental.json',import.meta.url)
 if(fs.existsSync(recordPath)&&JSON.parse(fs.readFileSync(recordPath)).status==='submitted')throw Error('存在待确认租金交易，请先核实 last-rental.json')
 const gas=await rpc.estimateGas({...tx,account:who})*120n/100n,gasPrice=await rpc.getGasPrice()
 if(gas>1000000n||gasPrice>200000000000n)throw Error('超过测试网 gas 上限')
 const wallet=createWalletClient({account,chain:monadTestnet,transport:http(url,{retryCount:0})})
 const raw=await wallet.signTransaction({...tx,account,gas,gasPrice,nonce:await rpc.getTransactionCount({address:who,blockTag:'pending'}),chainId:10143})
 const record={status:'submitted',chainId:10143,transactionHash:keccak256(raw),feeWei:String(fee)}
 const save=()=>fs.writeFileSync(recordPath,JSON.stringify(record,null,2)+'\n');save();console.log('transaction_hash='+record.transactionHash)
 await rpc.sendRawTransaction({serializedTransaction:raw})
 const receipt=await rpc.waitForTransactionReceipt({hash:record.transactionHash,timeout:90000})
 record.status=receipt.status;save();console.log('status='+receipt.status)
}
