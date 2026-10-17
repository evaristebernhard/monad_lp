// Safe, idempotent Monad Testnet demo deployment. No mainnet option.
import fs from 'node:fs';
import assert from 'node:assert/strict';
import {createPublicClient,createWalletClient,http,encodeDeployData,parseEther,formatEther,keccak256,parseAbi} from '../../../../frontend/node_modules/viem/_esm/index.js';
import {privateKeyToAccount} from '../../../../frontend/node_modules/viem/_esm/accounts/index.js';
import {monadTestnet} from '../../../../frontend/node_modules/viem/_esm/chains/index.js';
const dir=new URL('./',import.meta.url);
const root=new URL('../../',dir);
const artifact=JSON.parse(fs.readFileSync(new URL('evm/out/TinyMonadShowcase.sol/TinyMonadShowcase.json',root)));
const model='0x5dd31af6d70b7633fd5da3e30c03feb5fde00618';
const router='0x430c23895c8D44883526e3E0B09327dAD8766660';
const usdc='0x534b2f3A21130d7a60830c2Df862319e593943A3';
const wmon='0x97B3070F9Da6C002343862b35E68Bd8e22608943';
const recordUrl=new URL('deployment.json',dir);
const publicUrl=new URL('../../../../frontend/public/ai/showcase-config.json',dir);
const rpcUrl=process.env.MONAD_TESTNET_RPC_URL||'https://testnet-rpc.monad.xyz';
const client=createPublicClient({chain:monadTestnet,transport:http(rpcUrl,{timeout:30000,retryCount:2})});
const save=x=>fs.writeFileSync(recordUrl,JSON.stringify(x,null,2)+'\n');
function account(){
  const key=process.env.TESTNET_DEPLOYER_PRIVATE_KEY||fs.readFileSync(new URL('../../../../examples/monad-buy-usdc/.env.example',dir),'utf8').match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1];
  assert.match(key||'',/^(0x)?[a-fA-F0-9]{64}$/,'testnet key required');
  return privateKeyToAccount(key.startsWith('0x')?key:'0x'+key);
}
async function main(){
  assert.equal(await client.getChainId(),10143,'testnet only');
  const signer=account(), args=[model,router,usdc,wmon,signer.address];
  const abi=parseAbi(['function WETH() view returns(address)','function MODEL_SHA256() view returns(bytes32)']);
  assert.equal((await client.readContract({address:router,abi,functionName:'WETH'})).toLowerCase(),wmon.toLowerCase());
  assert.equal(await client.readContract({address:model,abi,functionName:'MODEL_SHA256'}),'0x183fa936ef593253276e4baa3fa8ecb6e1a5334f6dd2df07e38235cd9b8cc8b7');
  const data=encodeDeployData({abi:artifact.abi,bytecode:artifact.bytecode.object,args});
  const hashData=keccak256(data);
  let record=fs.existsSync(recordUrl)?JSON.parse(fs.readFileSync(recordUrl)):null;
  if(record){
    assert.equal(record.chainId,10143);assert.equal(record.dataHash,hashData);
    assert.equal(record.signer.toLowerCase(),signer.address.toLowerCase());
    if(record.address){
      const code=await client.getBytecode({address:record.address});
      assert.ok(code&&code.length>2,'previous deployment missing code');
      for(const [key,want] of Object.entries({owner:signer.address,keeper:signer.address,model,router,usdc,wmon})){
        const got=await client.readContract({address:record.address,abi:artifact.abi,functionName:key});
        assert.equal(got.toLowerCase(),want.toLowerCase(),key);
      }
      fs.writeFileSync(publicUrl,JSON.stringify({chainId:10143,address:record.address,model,router,usdc,wmon,blockNumber:record.blockNumber,explorer:'https://testnet.monadscan.com/address/'+record.address},null,2)+'\n');
      console.log(JSON.stringify({status:'EXISTING_VERIFIED',address:record.address,tx:record.hash}));return;
    }
  }
  const estimate=await client.estimateGas({account:signer.address,data});
  const gas=estimate*112n/100n+1n;
  const price=await client.getGasPrice();
  const maxFee=price*120n/100n+1n;
  const budget=gas*maxFee;
  assert.ok(gas<=2_000_000n && maxFee<=200_000_000_000n && budget<=parseEther('0.4'),'gas limit');
  assert.ok(await client.getBalance({address:signer.address})>=budget+parseEther('0.1'),'insufficient gas budget');
  console.log(JSON.stringify({stage:'preflight',account:signer.address,estimate:String(estimate),gas:String(gas),maximumFeeMON:formatEther(budget)}));
  if(!process.argv.includes('--deploy'))return;
  const wallet=createWalletClient({chain:monadTestnet,account:signer,transport:http(rpcUrl,{timeout:30000,retryCount:1})});
  if(!record){
    const request=await wallet.prepareTransactionRequest({account:signer,data,gas,maxFeePerGas:maxFee,maxPriorityFeePerGas:0n,value:0n});
    const raw=await wallet.signTransaction(request);
    record={chainId:10143,signer:signer.address,dataHash:hashData,hash:keccak256(raw),status:'signed',gasLimit:String(gas)};
    save(record);
    await client.sendRawTransaction({serializedTransaction:raw});
    record.status='broadcast';save(record);
  }
  const receipt=await client.waitForTransactionReceipt({hash:record.hash,timeout:120000,pollingInterval:1200});
  assert.equal(receipt.status,'success','deploy failed');
  record={...record,address:receipt.contractAddress,blockNumber:String(receipt.blockNumber),chargedFeeMON:formatEther(BigInt(record.gasLimit)*receipt.effectiveGasPrice),status:'verified'};
  save(record);
  const result=await client.readContract({address:record.address,abi:artifact.abi,functionName:'model'});
  assert.equal(result.toLowerCase(),model.toLowerCase());
  fs.writeFileSync(publicUrl,JSON.stringify({chainId:10143,address:record.address,model,router,usdc,wmon,blockNumber:record.blockNumber,explorer:'https://testnet.monadscan.com/address/'+record.address},null,2)+'\n');
  console.log(JSON.stringify(record,null,2));
}
main().catch(e=>{console.error(e.shortMessage||e.message);process.exitCode=1;});
