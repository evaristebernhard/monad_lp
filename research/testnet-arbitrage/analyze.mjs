import fs from 'node:fs';
import assert from 'node:assert/strict';
import {createPublicClient,http,formatUnits,parseUnits} from '../../frontend/node_modules/viem/_esm/index.js';
import {monadTestnet} from '../../frontend/node_modules/viem/_esm/chains/index.js';
const root=new URL('./',import.meta.url),snapshot=JSON.parse(fs.readFileSync(new URL('snapshot.json',root)));
assert.equal(snapshot.chainId,10143);
const rpc=createPublicClient({chain:monadTestnet,transport:http('https://testnet-rpc.monad.xyz')});assert.equal(await rpc.getChainId(),10143);
const gasPrice=await rpc.getGasPrice();
const quote=(amount,rin,rout,fee)=>amount*BigInt(1000000-fee)*rout/(rin*1000000n+amount*BigInt(1000000-fee));
const pools=snapshot.pools.filter(p=>p.kind==='v2'&&BigInt(p.r0)>0n&&BigInt(p.r1)>0n);
const comparisons=[];
for(const p of pools){const base=p.t1,other=p.t0;for(const size of ['1','10','100','1000']){
 const amount=parseUnits(size,base.decimals),r0=BigInt(p.r0),r1=BigInt(p.r1),received=quote(amount,r1,r0,p.fee);
 // Second swap uses the state AFTER the first swap, including all fee input.
 const returned=quote(received,r0-received,r1+amount,p.fee);
 comparisons.push({pool:p.address,base:base.symbol,baseAddress:base.address,quote:other.symbol,input:size,output:formatUnits(received,other.decimals),returnedSamePool:formatUnits(returned,base.decimals),lossBeforeGas:formatUnits(amount-returned,base.decimals)});
}}
const crossPool=[];for(const p of pools)for(const q of pools){if(p.address===q.address)continue;const same=p.t0.address===q.t0.address&&p.t1.address===q.t1.address;const reversed=p.t0.address===q.t1.address&&p.t1.address===q.t0.address;if(!same&&!reversed)continue;crossPool.push({buy:p.address,sell:q.address});}
const result={chainId:10143,sourceBlock:snapshot.blockNumber,gasPriceWei:String(gasPrice),illustrativeGasBudget:300000,illustrativeGasCostMON:formatUnits(gasPrice*300000n,18),gasNote:'300000 is an illustration, not an eth_estimateGas result. Demo WMON cannot redeem for native MON.',comparisons,crossPoolCandidates:crossPool,conclusion:crossPool.length?'Needs executable cross-pool quote validation':'No same-address token pair across distinct surveyed V2 pools; no verified cross-pool arbitrage. A same-pool round trip loses fees before gas.'};
fs.writeFileSync(new URL('analysis.json',root),JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify(result,null,2));
