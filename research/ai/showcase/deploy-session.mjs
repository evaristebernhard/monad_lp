// Idempotent, testnet-only bounded session trader deployment.
// Signed transaction saved BEFORE broadcast to avoid accidental duplicate deploys.
import fs from 'node:fs';
import assert from 'node:assert/strict';
import {createPublicClient,createWalletClient,http,encodeDeployData,parseEther,formatEther,keccak256,parseAbi} from '../../../frontend/node_modules/viem/_esm/index.js';
import {privateKeyToAccount} from '../../../frontend/node_modules/viem/_esm/accounts/index.js';
import {monadTestnet} from '../../../frontend/node_modules/viem/_esm/chains/index.js';
const here=new URL('./',import.meta.url);
const artifact=JSON.parse(fs.readFileSync(new URL('../evm/out/TinyMonadSessionTrader.sol/TinyMonadSessionTrader.json',here),'utf8'));
const target={
 model:'0x5dd31af6d70b7633fd5da3e30c03feb5fde00618',
 router:'0x430c23895c8D44883526e3E0B09327dAD8766660',
 usdc:'0x534b2f3A21130d7a60830c2Df862319e593943A3',
 wmon:'0x97B3070F9Da6C002343862b35E68Bd8e22608943'
};
const recordUrl=new URL('session-deployment.json',here);
const publicUrl=new URL('../../../frontend/public/ai/session-config.json',here);
const network=process.env.MONAD_TESTNET_RPC_URL||'https://testnet-rpc.monad.xyz';
const client=createPublicClient({chain:monadTestnet,transport:http(network,{timeout:35000,retryCount:1})});
const j=(x)=>JSON.stringify(x,(_,v)=>typeof v==='bigint'?String(v):v,2)+'\n';
const walletKey=process.env.TESTNET_DEPLOYER_PRIVATE_KEY||fs.readFileSync(new URL('../../../examples/monad-buy-usdc/.env.example',here),'utf8').match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1];
assert.match(walletKey||'',/^(0x)?[a-fA-F0-9]{64}$/,'missing testnet signing key');
const signer=privateKeyToAccount(walletKey.startsWith('0x')?walletKey:'0x'+walletKey);
const config={...target,keeper:signer.address};
const data=encodeDeployData({abi:artifact.abi,bytecode:artifact.bytecode.object,args:Object.values(config)});
const dataHash=keccak256(data);
const save=(x)=>fs.writeFileSync(recordUrl,j(x));
const publish=(x)=>fs.writeFileSync(publicUrl,j({chainId:10143,contract:x.address,...config,explorer:'https://testnet.monadscan.com/address/'+x.address,
    transaction:x.hash,intervalSeconds:10,rounds:6,sessionTTL:110,buyUSDC:'0.4',sellWMON:'0.1',
    maxUSDC:'0.8',maxWMON:'0.2',mode:'keeper-session'}));
async function verify(address){
  const code=await client.getBytecode({address});
  assert.ok(code&&code!=='0x','deployed contract missing');
  for(const [name,want] of Object.entries(config)){
    const actual=await client.readContract({address,abi:artifact.abi,functionName:name});
    assert.equal(actual.toLowerCase(),want.toLowerCase(),name);
  }
  return keccak256(code);
}
async function main(){
  assert.equal(await client.getChainId(),10143,'only testnet');
  const abi=parseAbi(['function WETH() view returns(address)','function MODEL_SHA256() view returns(bytes32)']);
  assert.equal((await client.readContract({address:target.router,abi,functionName:'WETH'})).toLowerCase(),target.wmon.toLowerCase());
  assert.equal(await client.readContract({address:target.model,abi,functionName:'MODEL_SHA256'}),'0x183fa936ef593253276e4baa3fa8ecb6e1a5334f6dd2df07e38235cd9b8cc8b7');
  let record=fs.existsSync(recordUrl)?JSON.parse(fs.readFileSync(recordUrl,'utf8')):null;
  if(record){
    assert.equal(record.chainId,10143);
    assert.equal(record.dataHash,dataHash,'contract changed; do not redeploy over previous manifest');
    assert.equal(record.signer.toLowerCase(),signer.address.toLowerCase(),'different owner');
    if(record.address){
      record.runtimeHash=await verify(record.address);
      publish(record);
      console.log(j({status:'already deployed and verified',...record}));
      return;
    }
    const receipt=await client.getTransactionReceipt({hash:record.hash}).catch(()=>null);
    if(!receipt) throw Error('Prior transaction is unresolved: '+record.hash+'. Do NOT retry with new nonce.');
    assert.equal(receipt.status,'success','prior deployment failed');
    record={...record,address:receipt.contractAddress,status:'verified',blockNumber:String(receipt.blockNumber)};
    record.runtimeHash=await verify(record.address);save(record);publish(record);console.log(j(record));return;
  }
  const estimated=await client.estimateGas({account:signer.address,data});
  const gas=estimated*115n/100n+1n;
  const gasPrice=await client.getGasPrice();
  const maxFeePerGas=gasPrice*120n/100n+1n;
  const maxCost=gas*maxFeePerGas;
  assert.ok(gas<=2000000n && maxFeePerGas<=200000000000n && maxCost<=parseEther('0.4'),'fee cap');
  assert.ok(await client.getBalance({address:signer.address})>maxCost+parseEther('0.8'),'insufficient MON');
  console.log(j({preflight:true,chainId:10143,account:signer.address,gasEstimated:String(estimated),gasLimit:String(gas),maxFeeMON:formatEther(maxCost)}));
  if(!process.argv.includes('--deploy'))return;
  const w=createWalletClient({chain:monadTestnet,account:signer,transport:http(network,{timeout:35000,retryCount:0})});
  const request=await w.prepareTransactionRequest({account:signer,data,gas,maxFeePerGas,maxPriorityFeePerGas:0n,value:0n});
  const signed=await w.signTransaction(request);
  record={chainId:10143,signer:signer.address,dataHash,hash:keccak256(signed),status:'signed',gasLimit:String(gas)};
  save(record);
  try{await client.sendRawTransaction({serializedTransaction:signed});record.status='broadcast';save(record);}
  catch(e){console.error('Check saved transaction hash before retry:',record.hash);throw e;}
  const receipt=await client.waitForTransactionReceipt({hash:record.hash,timeout:120000,pollingInterval:1200});
  assert.equal(receipt.status,'success','reverted deployment');
  record={...record,status:'verified',address:receipt.contractAddress,blockNumber:String(receipt.blockNumber),gasUsed:String(receipt.gasUsed),
   paidFeeMON:formatEther(gas*receipt.effectiveGasPrice)};
  record.runtimeHash=await verify(record.address);
  save(record);publish(record);console.log(j(record));
}
main().catch(e=>{console.error(e.shortMessage||e.message);process.exitCode=1;});
