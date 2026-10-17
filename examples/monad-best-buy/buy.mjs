// Node >=22: node --env-file-if-exists=.env buy.mjs 0.1 --wallet 0x...
// Add --send to sign locally. No retry or private key upload.
import fs from 'node:fs'
import {createPublicClient,createWalletClient,http,formatUnits,parseUnits,keccak256} from 'viem'
import {privateKeyToAccount} from 'viem/accounts'
import {monad} from 'viem/chains'
import {selectBestFrame,verifyBestQuote,buildBestBuy} from './bestSwap.mjs'
const args=process.argv.slice(2),amount=args[0]||'0.1',send=args.includes('--send')
if(args.includes('--help')){
 console.log('Usage: node --env-file=.env buy-standalone.mjs AMOUNT [--wallet 0xADDRESS] [--send] [--record FILE]\nDefault: preview only. --send signs locally and broadcasts ONE real MON -> USDC swap.\n--quote-only: fetch and verify best selection without a wallet or a separate RPC.\nEnvironment: MONAD_API_URL, MONAD_RPC_URL, PRIVATE_KEY (only on the signing computer).\n--slippage-bps 50 (1-100), --min-usdc 0, --protocol PROTOCOL (optional restriction).\nNode.js 24.18+ tested. Add --use-env-proxy before the filename if your network uses HTTPS_PROXY.');process.exit(0)
}
const quoteOnly=args.includes('--quote-only')
if(quoteOnly&&send)throw Error('--quote-only 不能和 --send 同时使用')
const arg=(name,fallback)=>args.includes(name)?args[args.indexOf(name)+1]:fallback
if(send&&!/^(0x)?[0-9a-fA-F]{64}$/.test(process.env.PRIVATE_KEY||''))throw Error('请在本机 .env 配置有效 PRIVATE_KEY')
const localKey=process.env.PRIVATE_KEY||''
if(localKey&&!/^(0x)?[0-9a-fA-F]{64}$/.test(localKey))throw Error('PRIVATE_KEY 格式无效，请仅在本机配置')
const account=localKey?privateKeyToAccount(localKey.startsWith('0x')?localKey:'0x'+localKey):null
const recordPath=arg('--record',new URL('./last-buy.json',import.meta.url))
if(send&&fs.existsSync(recordPath)){const previous=JSON.parse(fs.readFileSync(recordPath));if(previous.status==='submitted')throw Error('存在待核实交易 '+previous.transactionHash+'；请先查询，不要重复发送')}
if(account&&arg('--wallet')&&arg('--wallet').toLowerCase()!==account.address.toLowerCase())throw Error('--wallet 与本机签名钱包不一致')
const wallet=account?.address||arg('--wallet')
if(!wallet&&!quoteOnly)throw Error('预览请加 --wallet 0x地址或配置本地 PRIVATE_KEY；仅报价使用 --quote-only')
const origin=(process.env.MONAD_API_URL||'https://lp.chayu123.me').replace(/\/$/,'')
const rpcUrl=process.env.MONAD_RPC_URL||'https://rpc.monad.xyz'
const rpc=createPublicClient({chain:monad,transport:http(rpcUrl,{timeout:20000,retryCount:0})})
const response=await fetch(origin+'/api/selection/quote?'+new URLSearchParams({amount,token_in:'MON',token_out:'USDC'}),{headers:{'User-Agent':'monad-best-buy/1.0'},signal:AbortSignal.timeout(20000)})
if(!response.ok)throw Error('报价 API HTTP '+response.status+': '+(await response.text()).slice(0,800))
const catalog=JSON.parse(fs.readFileSync(new URL('./catalog.json',import.meta.url),'utf8')).candidates
const protocol=arg('--protocol')
if(protocol&&!['Uniswap V2','Uniswap V3','Uniswap V4','PancakeSwap V3'].includes(protocol))throw Error('不支持的 --protocol')
const quote=selectBestFrame(await response.json(),catalog,amount,Date.now(),{protocol})
if(quoteOnly){console.log(JSON.stringify(quote.frame,null,2));process.exit(0)}
await verifyBestQuote(rpc,quote)
const block=await rpc.getBlock(),deadline=block.timestamp+120n
const {minimum,...tx}=buildBestBuy({...quote,wallet,deadline,slippageBps:Number(arg('--slippage-bps','50'))})
if(minimum<parseUnits(arg('--min-usdc','0'),6))throw Error('最低到账低于 --min-usdc')
console.log(JSON.stringify({network:'Monad mainnet 143',selectionScope:quote.scope,pool:quote.route.pool,protocol:quote.route.protocol,router:tx.to,spendMON:amount,expectedUSDC:formatUnits(quote.amountOut,6),minimumUSDC:formatUnits(minimum,6),coverage:`${quote.available}/${catalog.length}`,transaction:{to:tx.to,value:String(tx.value)},broadcast:false},null,2))
if(send){
 const gas=(await rpc.estimateGas(tx))*120n/100n,gasPrice=await rpc.getGasPrice()
 if(gas>1000000n||gasPrice>200000000000n)throw Error('超过默认 gas 上限：1,000,000 gas / 200 gwei')
 if(await rpc.getBalance({address:wallet})<tx.value+gas*gasPrice)throw Error('MON 不足以支付本金和 gas')
 if(Date.now()-Number(quote.frame.sourceTimestamp)*1000>30000||deadline<=BigInt(Math.floor(Date.now()/1000)))throw Error('报价已过期，请重新运行')
 const signer=createWalletClient({account,chain:monad,transport:http(rpcUrl,{retryCount:0})})
 const nonce=await rpc.getTransactionCount({address:wallet,blockTag:'pending'})
 const serializedTransaction=await signer.signTransaction({...tx,account,gas,gasPrice,nonce,chainId:143})
 const hash=keccak256(serializedTransaction)
 const record={status:'submitted',chainId:143,selectionScope:quote.scope,wallet,transactionHash:hash,pool:quote.route.pool,protocol:quote.route.protocol,router:tx.to,inputMON:amount,expectedUSDC:formatUnits(quote.amountOut,6),minimumUSDC:formatUnits(minimum,6),quoteBlock:quote.frame.sourceBlock,submittedAt:new Date().toISOString()}
 const save=()=>fs.writeFileSync(recordPath,JSON.stringify(record,null,2)+'\n')
 save();console.log('transaction_hash='+hash)
 await rpc.sendRawTransaction({serializedTransaction})
 const receipt=await rpc.waitForTransactionReceipt({hash,timeout:180000})
 record.status=receipt.status==='success'?'confirmed':'reverted';record.blockNumber=String(receipt.blockNumber);record.gasUsed=String(receipt.gasUsed);record.gasCostMON=formatUnits(receipt.gasUsed*receipt.effectiveGasPrice,18)
 const transferTopic=keccak256(new TextEncoder().encode('Transfer(address,address,uint256)'))
 let received=0n
 for(const log of receipt.logs)if(log.address.toLowerCase()==='0x754704bc059f8c67012fed69bc8a327a5aafb603'&&log.topics[0]===transferTopic){if(log.topics[2]?.slice(-40).toLowerCase()===wallet.slice(2).toLowerCase())received+=BigInt(log.data);if(log.topics[1]?.slice(-40).toLowerCase()===wallet.slice(2).toLowerCase())received-=BigInt(log.data)}
 record.receivedUSDC=formatUnits(received,6);record.explorer='https://monadscan.com/tx/'+hash;save()
 if(receipt.status!=='success')throw Error('交易回滚，请检查上述哈希')
 if(received<minimum)throw Error('回执中未核实足额 USDC，请检查记录；不要重发')
 console.log(JSON.stringify(record,null,2))
}
