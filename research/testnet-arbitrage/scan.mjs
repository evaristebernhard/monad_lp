// Read-only Monad TESTNET survey. No wallet, private key, signing or mainnet data.
import fs from 'node:fs';
import assert from 'node:assert/strict';
import {createPublicClient,http,parseAbi,formatUnits} from '../../frontend/node_modules/viem/_esm/index.js';
import {monadTestnet} from '../../frontend/node_modules/viem/_esm/chains/index.js';
const out=new URL('./',import.meta.url);
const rpc=createPublicClient({chain:monadTestnet,transport:http(process.env.MONAD_TESTNET_RPC_URL||'https://testnet-rpc.monad.xyz',{timeout:25000,retryCount:1})});
assert.equal(await rpc.getChainId(),10143,'TESTNET ONLY');
const block=await rpc.getBlock();
const abi=parseAbi(['function getPair(address,address) view returns(address)','function allPairsLength() view returns(uint256)','function allPairs(uint256) view returns(address)','function getPool(address,address,uint24) view returns(address)','function token0() view returns(address)','function token1() view returns(address)','function factory() view returns(address)','function symbol() view returns(string)','function decimals() view returns(uint8)','function balanceOf(address) view returns(uint256)','function fee() view returns(uint24)','function liquidity() view returns(uint128)','function getReserves() view returns(uint112,uint112,uint32)','function slot0() view returns(uint160,int24,uint16,uint16,uint16,uint8,bool)','function quoteExactInputSingle((address tokenIn,address tokenOut,uint256 amountIn,uint24 fee,uint160 sqrtPriceLimitX96)) returns(uint256,uint160,uint32,uint256)']);
const read=(address,functionName,args=[])=>rpc.readContract({address:address.toLowerCase(),abi,functionName,args,blockNumber:block.number});
async function map(items,fn){let result=[];for(let i=0;i<items.length;i+=6)result.push(...await Promise.all(items.slice(i,i+6).map(fn)));return result;}
const errors=[];
async function safe(label,fn){try{return await fn();}catch(e){errors.push({label,error:e.shortMessage||e.message});return null;}}
const tokens=[
 ['WMON-old','0x760afe86e5de5fa0ee542fc7b7b713e1c5425701'],
 ['USDC-old','0xf817257fed379853cde0fa4f97ab987181b1e5ea'],
 ['USDC-Uni-example','0xfbc2d240a5ed44231aca3a9e9066bc4b33f01149'],
 ['USDT-old','0x88b8e2161dedc77ef4ab7585569d2415a1c1055d'],
 ['WETH-old','0xb5a30b0fdc42e3e9760cb8449fb3700000000000'],
 ['WMON-list','0xfb8bf4c1cc7a94c73d209a149ea2abea852bc541'],
 ['USDC-list','0x534b2f3a21130d7a60830c2df862319e593943a3'],
 ['WETH-list','0x45477f4709771331db81944a5e20ef95bc7ba2d7'],
 ['WMON-demo','0x6d427a189325473725c89ed13cb734cfacbd75a6'],
 ['USDC-demo','0x0ac0b00cf3e7ab4b287a99c907c93f74bd73e737']
];
const meta=new Map();
async function metadata(address){address=address.toLowerCase();if(meta.has(address))return meta.get(address);const [symbol,decimals]=await Promise.all([read(address,'symbol'),read(address,'decimals')]);const t={address,symbol,decimals:Number(decimals)};meta.set(address,t);return t;}
const valid=(await map(tokens,([label,address])=>safe(label,async()=>({...await metadata(address),label})))).filter(Boolean);
console.log('block',String(block.number),'tokens',valid.length);
const factories=[
 {name:'FableSwap demo',kind:'v2',address:'0x514d4ad259143c4a6be7c2399d46cbe8b1f9e2db',fee:3000},
 {name:'Uniswap V2',kind:'v2',address:'0x733e88f248b742db6c14c0b1713af5ad7fdd59d0',fee:3000},
 {name:'Uniswap V3',kind:'v3',address:'0x961235a9020b05c44df1026d956d1f4d78014276',quoter:'0x1ba215c17565de7b0cb7ecab971bcf540c24a862'},
 {name:'Pancake candidate',kind:'v3',address:'0x0bfbcf9fa4f9c56b0f40a671ad40e0805a091865',quoter:'0xb048bbc1ee6b733fffcfb9e9cef7375518e25997'},
 {name:'DODO configured V2',kind:'v2',address:'0xc176ecf1eae0883b2356593d1ccd5dded0441eb1',fee:3000},
 {name:'DODO configured V3',kind:'v3',address:'0x816d85d853a7da1f91f427e4132056d88620e7d7'}
];
const pools=[],discovery=[];
for(const f of factories){
 const code=await rpc.getCode({address:f.address,blockNumber:block.number});if(!code||code==='0x'){discovery.push({...f,status:'no bytecode'});continue;}
 const found=new Set();
 const pairs=[];for(let i=0;i<valid.length;i++)for(let j=i+1;j<valid.length;j++)pairs.push([valid[i].address,valid[j].address]);
 const queries=pairs.flatMap(p=>f.kind==='v2'?[{p,fee:f.fee}]:[100,500,2500,3000,10000].map(fee=>({p,fee})));
 await map(queries,q=>safe(`${f.name} discovery`,async()=>{const a=await read(f.address,f.kind==='v2'?'getPair':'getPool',f.kind==='v2'?q.p:[...q.p,q.fee]);if(a!=='0x0000000000000000000000000000000000000000')found.add(a.toLowerCase());}));
 let total=null;if(f.kind==='v2'){total=await safe(f.name+' count',()=>read(f.address,'allPairsLength'));if(total!==null){const n=Number(total);const ids=[...new Set([...Array(Math.min(25,n)).keys(),...Array.from({length:Math.min(25,n)},(_,i)=>n-1-i)])];await map(ids,i=>safe('allPairs',async()=>found.add((await read(f.address,'allPairs',[BigInt(i)])).toLowerCase())));}}
 discovery.push({...f,status:'queried',totalPairs:total,found:found.size});
 await map([...found],address=>safe(address,async()=>{
  const [a,b,factory]=await Promise.all([read(address,'token0'),read(address,'token1'),read(address,'factory')]);assert.equal(factory.toLowerCase(),f.address);
  const [t0,t1,bal0,bal1]=await Promise.all([metadata(a),metadata(b),read(a,'balanceOf',[address]),read(b,'balanceOf',[address])]);
  const p={address,protocol:f.name,kind:f.kind,factory:f.address,quoter:f.quoter,t0,t1,bal0,bal1};
  if(f.kind==='v2'){const r=f.name==='FableSwap demo'?await rpc.readContract({address,abi:parseAbi(['function getReserves() view returns(uint112,uint112)']),functionName:'getReserves',blockNumber:block.number}):await read(address,'getReserves');p.r0=r[0];p.r1=r[1];p.fee=f.fee;p.price=Number(formatUnits(p.r1,t1.decimals))/Number(formatUnits(p.r0,t0.decimals));}
  else{const [s,l,fee]=await Promise.all([read(address,'slot0'),read(address,'liquidity'),read(address,'fee')]);p.sqrt=s[0];p.tick=s[1];p.liquidity=l;p.fee=Number(fee);p.price=(Number(s[0])/2**96)**2*10**(t0.decimals-t1.decimals);}
  pools.push(p);
 }));console.log(f.name,'found',found.size,'read',pools.length);
}
const wallet=process.env.TESTNET_WALLET_ADDRESS||'0x41d5D3B5f2e497d1F4bCF767ffac9DDAb85D2b79';
const balances=await map([...meta.values()],t=>safe('balance '+t.address,async()=>({...t,amount:formatUnits(await read(t.address,'balanceOf',[wallet]),t.decimals)})));
const result={chainId:10143,readOnly:true,blockNumber:block.number,blockHash:block.hash,blockTimestamp:block.timestamp,capturedAt:new Date().toISOString(),wallet,nativeMON:formatUnits(await rpc.getBalance({address:wallet,blockNumber:block.number}),18),tokens:valid,discovery,pools,balances:balances.filter(Boolean),errors,scope:'Candidate-token cross pairs and first/last 25 V2 pairs; not full-chain coverage. V2 fee assumed 0.30% pending implementation verification. V3 slot prices, when found, are not executable quotes. No execution.'};
fs.writeFileSync(new URL('snapshot.json',out),JSON.stringify(result,(_,v)=>typeof v==='bigint'?String(v):v,2)+'\n');
console.log(JSON.stringify({pools:pools.length,nativeMON:result.nativeMON,errors:errors.length}));
