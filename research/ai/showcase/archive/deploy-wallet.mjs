// Monad testnet wallet-owned 6-round model demo deployment.
// No key is included in frontend assets; no funding or trading occurs here.
import fs from 'node:fs';
import assert from 'node:assert/strict';
import {createPublicClient,createWalletClient,http,encodeDeployData,parseEther,formatEther,keccak256,parseAbi} from '../../../../frontend/node_modules/viem/_esm/index.js';
import {privateKeyToAccount} from '../../../../frontend/node_modules/viem/_esm/accounts/index.js';
import {monadTestnet} from '../../../../frontend/node_modules/viem/_esm/chains/index.js';

const here=new URL('./',import.meta.url);
const artifact=JSON.parse(fs.readFileSync(new URL('../../evm/out/TinyMonadWalletShowcase.sol/TinyMonadWalletShowcase.json',here),'utf8'));
const target={
 model:'0x5dd31af6d70b7633fd5da3e30c03feb5fde00618',
 router:'0x430c23895c8D44883526e3E0B09327dAD8766660',
 usdc:'0x534b2f3A21130d7a60830c2Df862319e593943A3',
 wmon:'0x97B3070F9Da6C002343862b35E68Bd8e22608943'
};
const json=x=>JSON.stringify(x,(_,v)=>typeof v==='bigint'?String(v):v,2)+'\n';
const recordFile=new URL('wallet-deployment.json',here);
const publicFile=new URL('../../../../frontend/public/ai/wallet-showcase-config.json',here);
const rpcUrl=process.env.MONAD_TESTNET_RPC_URL||'https://testnet-rpc.monad.xyz';
const publicClient=createPublicClient({chain:monadTestnet,transport:http(rpcUrl,{timeout:30000,retryCount:1})});
const key=process.env.TESTNET_DEPLOYER_PRIVATE_KEY||fs.readFileSync(new URL('../../../../examples/monad-buy-usdc/.env.example',here),'utf8').match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1];
assert.match(key||'',/^(0x)?[0-9A-Fa-f]{64}$/,'Missing testnet deploy key');
const account=privateKeyToAccount(key.startsWith('0x')?key:'0x'+key);

async function verify(address) {
  const bytecode=await publicClient.getBytecode({address});
  assert.ok(bytecode && bytecode!=='0x','no bytecode');
  for (const [fn,want] of Object.entries(target)) {
    const actual=await publicClient.readContract({address,abi:artifact.abi,functionName:fn});
    assert.equal(actual.toLowerCase(),want.toLowerCase(),fn);
  }
  return keccak256(bytecode);
}
function publish(record) {
  fs.writeFileSync(publicFile,json({chainId:10143,address:record.address,model:target.model,
    router:target.router,usdc:target.usdc,wmon:target.wmon,transaction:record.hash,
    explorer:'https://testnet.monadscan.com/address/'+record.address,rounds:6,cooldownSeconds:10,
    buyUSDC:'0.4',sellWMON:'0.1',mode:'wallet-signed'}));
}
async function main() {
  assert.equal(await publicClient.getChainId(),10143,'wrong chain');
  const abi=parseAbi(['function WETH() view returns(address)','function MODEL_SHA256() view returns(bytes32)','function decimals() view returns(uint8)']);
  assert.equal((await publicClient.readContract({address:target.router,abi,functionName:'WETH'})).toLowerCase(),target.wmon.toLowerCase());
  assert.equal(await publicClient.readContract({address:target.model,abi,functionName:'MODEL_SHA256'}),'0x183fa936ef593253276e4baa3fa8ecb6e1a5334f6dd2df07e38235cd9b8cc8b7');
  assert.equal(await publicClient.readContract({address:target.usdc,abi,functionName:'decimals'}),6);
  assert.equal(await publicClient.readContract({address:target.wmon,abi,functionName:'decimals'}),18);
  const data=encodeDeployData({abi:artifact.abi,bytecode:artifact.bytecode.object,args:Object.values(target)});
  const dataHash=keccak256(data);
  let record=fs.existsSync(recordFile)?JSON.parse(fs.readFileSync(recordFile,'utf8')):null;
  if (record) {
    assert.equal(record.chainId,10143);
    assert.equal(record.dataHash,dataHash,'bytecode/config differs from previous deployment');
    assert.equal(record.signer.toLowerCase(),account.address.toLowerCase());
    if(record.address) {
      const runtimeHash=await verify(record.address);
      publish(record);
      console.log(json({status:'EXISTING_VERIFIED',address:record.address,runtimeHash,tx:record.hash}));
      return;
    }
    // If already signed/broadcast, recover from chain; never blindly re-send with a new nonce.
    if(record.hash) {
      const receipt=await publicClient.getTransactionReceipt({hash:record.hash}).catch(()=>null);
      if(!receipt) throw Error('Previous signed transaction unresolved: '+record.hash);
      assert.equal(receipt.status,'success','deployment reverted');
      record.address=receipt.contractAddress;
      record.status='verified';
      fs.writeFileSync(recordFile,json(record));
      await verify(record.address);publish(record);
      console.log(json(record));return;
    }
  }
  const estimate=await publicClient.estimateGas({account:account.address,data});
  const gas=estimate*115n/100n+1n;
  const gasPrice=await publicClient.getGasPrice();
  const fee=gasPrice*120n/100n+1n;
  const maxCharge=gas*fee;
  assert.ok(gas<=1900000n && fee<=200000000000n && maxCharge<=parseEther('0.3'),'deployment fee cap');
  assert.ok(await publicClient.getBalance({address:account.address})>maxCharge+parseEther('0.5'),'low gas balance');
  console.log(json({stage:'preflight',chainId:10143,signer:account.address,
    estimatedGas:String(estimate),gasLimit:String(gas),maximumChargeMON:formatEther(maxCharge)}));
  if(!process.argv.includes('--deploy'))return;
  const wallet=createWalletClient({account,chain:monadTestnet,transport:http(rpcUrl,{timeout:30000,retryCount:1})});
  const request=await wallet.prepareTransactionRequest({account,data,gas,
    maxFeePerGas:fee,maxPriorityFeePerGas:0n,value:0n});
  const raw=await wallet.signTransaction(request);
  record={chainId:10143,signer:account.address,dataHash,hash:keccak256(raw),
    gasLimit:String(gas),status:'signed'};
  fs.writeFileSync(recordFile,json(record));
  try {await publicClient.sendRawTransaction({serializedTransaction:raw});record.status='broadcast';fs.writeFileSync(recordFile,json(record));}
  catch (e) {console.error('Check saved tx hash before retrying:',record.hash);throw e;}
  const receipt=await publicClient.waitForTransactionReceipt({hash:record.hash,timeout:120000,pollingInterval:1500});
  assert.equal(receipt.status,'success','deployment reverted');
  record={...record,address:receipt.contractAddress,status:'verified',blockNumber:String(receipt.blockNumber),
    gasUsed:String(receipt.gasUsed),paidMON:formatEther(gas*receipt.effectiveGasPrice)};
  record.runtimeHash=await verify(record.address);
  fs.writeFileSync(recordFile,json(record));
  publish(record);
  console.log(json(record));
}
main().catch(e=>{console.error(e.shortMessage||e.message);process.exitCode=1;});
