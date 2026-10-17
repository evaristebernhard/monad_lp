// Read-only Monad Testnet preflight. Do not load a wallet or sign transactions.
import {createPublicClient,http,parseAbi,formatUnits} from '../../../frontend/node_modules/viem/_esm/index.js';
import {monadTestnet} from '../../../frontend/node_modules/viem/_esm/chains/index.js';
import assert from 'node:assert/strict';
const rpc=createPublicClient({chain:monadTestnet,transport:http(process.env.MONAD_TESTNET_RPC_URL||'https://testnet-rpc.monad.xyz',{timeout:25000,retryCount:1})});
assert.equal(await rpc.getChainId(),10143);
const factory='0xd498f5beBD0C9f1FE0135a0Cf942dA67Ee6e8A9B';
const router='0x430c23895c8D44883526e3E0B09327dAD8766660';
const usdc='0x534b2f3A21130d7a60830c2Df862319e593943A3';
const wmon='0x97B3070F9Da6C002343862b35E68Bd8e22608943';
const officialWmon='0xFb8bf4c1CC7a94c73D209a149eA2AbEa852BC541';
const abi=parseAbi(['function getPair(address,address) view returns(address)','function getReserves() view returns(uint112,uint112,uint32)','function token0() view returns(address)','function token1() view returns(address)','function WETH() view returns(address)','function factory() view returns(address)','function getAmountsOut(uint256,address[]) view returns(uint256[])','function balanceOf(address) view returns(uint256)','function decimals() view returns(uint8)','function symbol() view returns(string)']);
const read=(address,functionName,args=[])=>rpc.readContract({address,abi,functionName,args});
const pair=await read(factory,'getPair',[usdc,wmon]);
const code={};for(const [key,address] of Object.entries({factory,router,usdc,wmon,officialWmon,pair}))code[key]=Boolean(await rpc.getBytecode({address}));
let out={chainId:10143,block:String(await rpc.getBlockNumber()),factory,router,usdc,wmon,officialWmon,pair,code};
if(code.router)out.routerWETH=await read(router,'WETH');
if(code.pair){const [a,b,r]=await Promise.all([read(pair,'token0'),read(pair,'token1'),read(pair,'getReserves')]);out.reserves={token0:a,token1:b,raw0:String(r[0]),raw1:String(r[1])};}
if(code.router&&code.pair){for(const [side,amt,path] of [['buy','1000000',[usdc,wmon]],['sell','1000000000000000000',[wmon,usdc]]]){try{const q=await read(router,'getAmountsOut',[BigInt(amt),path]);out[side]={amountIn:amt,amountOut:String(q.at(-1))};}catch(e){out[side]={error:e.shortMessage||e.message}}}}
console.log(JSON.stringify(out,null,2));
