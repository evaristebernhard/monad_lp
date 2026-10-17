import fs from 'node:fs'
import {createPublicClient,http,parseAbi,padHex} from '../frontend/node_modules/viem/_esm/index.js'
const root=new URL('./',import.meta.url),read=p=>JSON.parse(fs.readFileSync(new URL(p,root),'utf8'))
const rpc=createPublicClient({cacheTime:0,transport:http('http://127.0.0.1:8545/rpc/monad',{timeout:15000,retryCount:0})})
const WMON='0x3bd359c1119da7da1d913d1c4d2b7c461115433a',USDC='0x754704bc059f8c67012fed69bc8a327a5aafb603'
const quoters={'Uniswap V3':'0x661e93cca42afacb172121ef892830ca3b70f08d','PancakeSwap V3':'0xb048bbc1ee6b733fffcfb9e9cef7375518e25997'}
const abi=parseAbi(['function token0() view returns(address)','function token1() view returns(address)','function fee() view returns(uint24)','function factory() view returns(address)','function getPool(address,address,uint24) view returns(address)','function getPair(address,address) view returns(address)'])
if(await rpc.getChainId()!==143)throw Error('Wrong chain')
const block=await rpc.getBlock(),call=(address,functionName,args=[])=>rpc.readContract({address,abi,functionName,args,blockNumber:block.number})
const report=read('results/pool-coverage.json'),old=read('results/live-v4-selection.json').scenarios[0].quotes,candidates=[],excluded=[]
for(const row of report.pools){
 if(row.protocol==='Uniswap V4'){
  const verified=old.find(q=>q.pool===row.pool_address)
  if(!verified){excluded.push({pool:row.pool_address,protocol:row.protocol,reason:'PoolKey not verified'});continue}
  candidates.push({...verified,kind:'v4',protocol:row.protocol,fee:verified.poolKey.fee});continue
 }
 try{
 const address=row.pool_address
 const [token0,token1,factory]=await Promise.all([call(address,'token0'),call(address,'token1'),call(address,'factory')])
 if(![token0.toLowerCase(),token1.toLowerCase()].includes(WMON)||![token0.toLowerCase(),token1.toLowerCase()].includes(USDC))throw Error('On-chain tokens differ')
 const kind=row.protocol==='Uniswap V2'?'v2':'v3',fee=kind==='v3'?Number(await call(address,'fee')):3000
 const quoter=quoters[row.protocol]
 if(kind==='v3'&&!quoter)throw Error('No verified quoter')
 if(kind==='v3'&&(await call(quoter,'factory')).toLowerCase()!==factory.toLowerCase())throw Error('Quoter factory mismatch')
 const canonical=await call(factory,kind==='v3'?'getPool':'getPair',kind==='v3'?[token0,token1,fee]:[token0,token1])
 if(canonical.toLowerCase()!==address.toLowerCase())throw Error('Factory pool mismatch')
 candidates.push({pool:padHex(address,{size:32}),address,kind,protocol:row.protocol,token0,token1,factory,fee,quoter})
 }catch(e){excluded.push({pool:row.pool_address,protocol:row.protocol,reason:e.shortMessage||e.message})}
}
fs.writeFileSync(new URL('results/route-catalog.json',root),JSON.stringify({...report,pools:undefined,verificationBlock:String(block.number),candidates,excluded},null,2)+'\n')
console.log(JSON.stringify({total:report.totalMonadPools,samePair:report.samePairPools,verified:candidates.length,excluded},null,2))
