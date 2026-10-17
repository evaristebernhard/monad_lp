// Deploy a tiny on-chain model trading executor to Monad TESTNET only.
// Run without arguments for a read-only fee and configuration preflight.
// Run with --deploy to broadcast one capped creation transaction.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {createPublicClient, createWalletClient, http, encodeDeployData, keccak256, formatEther, parseEther, parseAbi} from '../../../frontend/node_modules/viem/_esm/index.js';
import {privateKeyToAccount} from '../../../frontend/node_modules/viem/_esm/accounts/index.js';
import {monadTestnet} from '../../../frontend/node_modules/viem/_esm/chains/index.js';

const chainId = 10143;
const model = '0x5dd31af6d70b7633fd5da3e30c03feb5fde00618';
const router = '0x430c23895c8D44883526e3E0B09327dAD8766660';
const usdc = '0x534b2f3A21130d7a60830c2Df862319e593943A3';
const wmon = '0x97B3070F9Da6C002343862b35E68Bd8e22608943';
const maxBuy = 1000000n; // 1 USDC, 6 decimals.
const maxSell = 1000000000000000000n; // 1 WMON, 18 decimals.
const feeBudget = parseEther('0.5');
const gasLimitCeiling = 2000000n;
const gasPriceCeiling = 200000000000n; // 200 gwei.
const rpc = createPublicClient({chain:monadTestnet,transport:http(process.env.MONAD_TESTNET_RPC_URL || 'https://testnet-rpc.monad.xyz',{timeout:45000,retryCount:1})});
const readJson = p => JSON.parse(fs.readFileSync(new URL(p,import.meta.url), 'utf8'));
const stringify = p => JSON.stringify(p, (_,v)=>typeof v==='bigint'?String(v):v,2)+'\n';
const artifact = readJson('../evm/out/TinyMonadTrader.sol/TinyMonadTrader.json');
const stateUrl = new URL('./trader-testnet-deployment.json',import.meta.url);
const live = process.argv.includes('--deploy');
assert.deepEqual(process.argv.slice(2).filter(x=>x!=='--deploy'), [], 'unknown argument');

function loadKey() {
  // Prefer an explicitly set key. The existing private local example configuration
  // was used by the prior TinyMonadPolicy testnet deployment.
  const fromEnv = process.env.TESTNET_DEPLOYER_PRIVATE_KEY;
  const localPath = new URL('../../../examples/monad-buy-usdc/.env.example',import.meta.url);
  const local = fromEnv ? null : fs.readFileSync(localPath,'utf8').match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1];
  const key = fromEnv || local;
  if (!key) throw Error('No testnet signing key available');
  assert.match(key, /^(0x)?[0-9a-fA-F]{64}$/, 'bad private key format');
  return privateKeyToAccount(key.startsWith('0x')?key:'0x'+key);
}

async function main() {
  assert.equal(await rpc.getChainId(), chainId, 'only Monad testnet 10143');
  const account=loadKey();
  const balance=await rpc.getBalance({address:account.address});
  const rpcAbi=parseAbi([
    'function WETH() view returns(address)',
    'function factory() view returns(address)',
    'function getAmountsOut(uint256,address[]) view returns(uint256[])',
    'function MODEL_SHA256() view returns(bytes32)',
    'function decimals() view returns(uint8)',
    'function getPair(address,address) view returns(address)'
  ]);
  for(const address of [model,router,usdc,wmon]) assert.ok(await rpc.getBytecode({address}), 'missing required code '+address);
  const wrapped=await rpc.readContract({address:router,abi:rpcAbi,functionName:'WETH'});
  assert.equal(wrapped.toLowerCase(), wmon.toLowerCase(), 'wrong Router wrapped asset');
  assert.equal(Number(await rpc.readContract({address:usdc,abi:rpcAbi,functionName:'decimals'})),6);
  assert.equal(Number(await rpc.readContract({address:wmon,abi:rpcAbi,functionName:'decimals'})),18);
  const expectedHash = '0x'+readJson('report.json').model_sha256;
  assert.equal(await rpc.readContract({address:model,abi:rpcAbi,functionName:'MODEL_SHA256'}),expectedHash);
  const factory=await rpc.readContract({address:router,abi:rpcAbi,functionName:'factory'});
  const pair=await rpc.readContract({address:factory,abi:rpcAbi,functionName:'getPair',args:[usdc,wmon]});
  assert.notEqual(pair.toLowerCase(),'0x0000000000000000000000000000000000000000','no pair');
  assert.ok(await rpc.getBytecode({address:pair}), 'missing pair bytecode');
  const buyQuote=await rpc.readContract({address:router,abi:rpcAbi,functionName:'getAmountsOut',args:[maxBuy,[usdc,wmon]]});
  const sellQuote=await rpc.readContract({address:router,abi:rpcAbi,functionName:'getAmountsOut',args:[maxSell,[wmon,usdc]]});
  assert.ok(buyQuote[1]>0n && sellQuote[1]>0n, 'missing executable quotes');
  const args=[model,account.address,router,usdc,wmon,maxBuy,maxSell];
  const data=encodeDeployData({abi:artifact.abi,bytecode:artifact.bytecode.object,args});
  const estimated=await rpc.estimateGas({account:account.address,data});
  const gas=estimated*115n/100n+1n;
  const gasPrice=await rpc.getGasPrice();
  const maxFeePerGas=gasPrice*120n/100n+1n;
  const maxCharge=gas*maxFeePerGas;
  const preflight={
    chainId,signer:account.address,keeper:account.address,model,router,usdc,wmon,pair,
    maxBuyUSDC:'1',maxSellWMON:'1',walletBalanceMON:formatEther(balance),
    buyQuoteWMON:formatEther(buyQuote[1]),sellQuoteUSDC:String(Number(sellQuote[1])/1e6),
    estimated,gas,gasPrice,maxFeePerGas,maxChargeMON:formatEther(maxCharge),
    feeBudgetMON:formatEther(feeBudget),checkedAt:new Date().toISOString()
  };
  assert.ok(gas<=gasLimitCeiling, 'gas cap exceeded');
  assert.ok(maxFeePerGas<=gasPriceCeiling, 'gas price cap exceeded');
  assert.ok(maxCharge<=feeBudget,'fee budget exceeded');
  assert.ok(balance>=maxCharge+parseEther('0.05'),'insufficient balance with reserve');

  if(!live) {
    console.log(stringify({mode:'read-only',...preflight,ready:true}));
    return;
  }
  let record=fs.existsSync(stateUrl) ? readJson('trader-testnet-deployment.json') : null;
  if(record) {
    assert.equal(record.chainId,chainId);
    assert.equal(record.signer.toLowerCase(),account.address.toLowerCase());
    assert.equal(record.dataHash,keccak256(data),'old deployment uses different arguments/bytecode');
    if(record.status==='verified') {console.log(stringify(record));return;}
  }
  const wallet=createWalletClient({account,chain:monadTestnet,transport:http(process.env.MONAD_TESTNET_RPC_URL || 'https://testnet-rpc.monad.xyz',{timeout:45000,retryCount:1})});
  if(!record) {
    const request=await wallet.prepareTransactionRequest({account,data,gas,maxFeePerGas,maxPriorityFeePerGas:0n,value:0n});
    const raw=await wallet.signTransaction(request);
    const hash=keccak256(raw);
    record={...preflight,dataHash:keccak256(data),hash,gasLimit:String(gas),status:'signed'};
    fs.writeFileSync(stateUrl,stringify(record));
    try {await rpc.sendRawTransaction({serializedTransaction:raw});record.status='submitted';fs.writeFileSync(stateUrl,stringify(record));}
    catch(error){console.error('Broadcast interrupted; inspect saved hash before retrying:',hash);throw error;}
  }
  const receipt=await rpc.waitForTransactionReceipt({hash:record.hash,timeout:120000,pollingInterval:1200});
  assert.equal(receipt.status,'success','deployment reverted');
  assert.ok(receipt.contractAddress,'missing deployed address');
  const deployed=receipt.contractAddress;
  const onchainCode=await rpc.getBytecode({address:deployed});
  assert.ok(onchainCode&&onchainCode.length>2);
  for(const [method,want] of [['owner',account.address],['keeper',account.address],['model',model],['router',router],['usdc',usdc],['wmon',wmon]]) {
    const got=await rpc.readContract({address:deployed,abi:artifact.abi,functionName:method});
    assert.equal(got.toLowerCase(),want.toLowerCase(),'wrong '+method);
  }
  assert.equal(await rpc.readContract({address:deployed,abi:artifact.abi,functionName:'maxBuyUSDC'}),maxBuy);
  assert.equal(await rpc.readContract({address:deployed,abi:artifact.abi,functionName:'maxSellWMON'}),maxSell);
  record={...record,address:deployed,blockNumber:String(receipt.blockNumber),
    gasUsed:String(receipt.gasUsed),effectiveGasPrice:String(receipt.effectiveGasPrice),
    paidFeeMON:formatEther(BigInt(record.gasLimit)*receipt.effectiveGasPrice),
    runtimeCodeHash:keccak256(onchainCode),
    explorer:'https://testnet.monadscan.com/address/'+deployed,
    verifiedAt:new Date().toISOString(),status:'verified'};
  fs.writeFileSync(stateUrl,stringify(record));
  console.log(stringify(record));
}
main().catch(e=>{console.error(e.shortMessage||e.message);process.exitCode=1;});
