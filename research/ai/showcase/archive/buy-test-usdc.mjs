// Buy TEST USDC with exactly 10 native TEST MON via fixed testnet V2 router.
// Run without args to check route/balances/quote. --send signs a single transaction.
import fs from 'node:fs';
import assert from 'node:assert/strict';
import {createPublicClient,createWalletClient,http,parseAbi,parseEther,formatUnits,formatEther,encodeFunctionData,keccak256} from '../../../../frontend/node_modules/viem/_esm/index.js';
import {privateKeyToAccount} from '../../../../frontend/node_modules/viem/_esm/accounts/index.js';
import {monadTestnet} from '../../../../frontend/node_modules/viem/_esm/chains/index.js';
const router='0x430c23895c8D44883526e3E0B09327dAD8766660';
const wmon='0x97B3070F9Da6C002343862b35E68Bd8e22608943';
const usdc='0x534b2f3A21130d7a60830c2Df862319e593943A3';
const nativeAmount=parseEther('10');
const abi=parseAbi([
'function WETH() view returns(address)',
'function getAmountsOut(uint256,address[]) view returns(uint256[])',
'function swapExactETHForTokens(uint256 amountOutMin,address[] path,address to,uint256 deadline) payable returns(uint256[])',
'function balanceOf(address) view returns(uint256)'
]);
const url=process.env.MONAD_TESTNET_RPC_URL||'https://testnet-rpc.monad.xyz';
const client=createPublicClient({chain:monadTestnet,transport:http(url,{timeout:30000,retryCount:1})});
const recordUrl=new URL('purchase-10-mon-usdc.json',import.meta.url);
const save=x=>fs.writeFileSync(recordUrl,JSON.stringify(x,null,2)+'\n');
const key=process.env.TESTNET_DEPLOYER_PRIVATE_KEY||fs.readFileSync(new URL('../../../../examples/monad-buy-usdc/.env.example',import.meta.url),'utf8').match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1];
assert.match(key||'',/^(0x)?[0-9A-Fa-f]{64}$/,'valid testnet key required');
const account=privateKeyToAccount(key.startsWith('0x')?key:'0x'+key);
async function main(){
  assert.equal(await client.getChainId(),10143,'never mainnet');
  assert.equal((await client.readContract({address:router,abi,functionName:'WETH'})).toLowerCase(),wmon.toLowerCase(),'router WMON mismatch');
  for(const addr of [router,wmon,usdc])assert.ok(await client.getBytecode({address:addr}), 'missing code');
  const balanceBefore=await client.getBalance({address:account.address});
  const usdcBefore=await client.readContract({address:usdc,abi,functionName:'balanceOf',args:[account.address]});
  const amountOut=(await client.readContract({address:router,abi,functionName:'getAmountsOut',args:[nativeAmount,[wmon,usdc]]}))[1];
  assert.ok(amountOut>30_000_000n,'pool quote unexpectedly low for 10 MON');
  const minOut=amountOut*98n/100n; // max 2% negative quote drift.
  const deadline=BigInt(Math.floor(Date.now()/1000)+10*60);
  const data=encodeFunctionData({abi,functionName:'swapExactETHForTokens',args:[minOut,[wmon,usdc],account.address,deadline]});
  const gasEstimate=await client.estimateGas({account:account.address,to:router,data,value:nativeAmount});
  const gas=gasEstimate*115n/100n+1n;
  const gasPrice=await client.getGasPrice();
  const maxFeePerGas=gasPrice*120n/100n+1n;
  const maximumGasCharge=gas*maxFeePerGas;
  assert.ok(gas<500000n&&maxFeePerGas<=200000000000n&&maximumGasCharge<parseEther('0.1'),'gas ceiling');
  assert.ok(balanceBefore>nativeAmount+maximumGasCharge+parseEther('1'),'MON balance too low');
  const preflight={
    chainId:10143,account:account.address,router,wmon,usdc,spendMon:'10',
    quoteUSDC:formatUnits(amountOut,6),minimumUSDC:formatUnits(minOut,6),
    walletNativeMON:formatEther(balanceBefore),walletUSDCBefore:formatUnits(usdcBefore,6),
    gasEstimate:String(gasEstimate),gasLimit:String(gas),maxFeePerGas:String(maxFeePerGas),
    maximumGasFeeMON:formatEther(maximumGasCharge),
    deadline:String(deadline)
  };
  let record=fs.existsSync(recordUrl)?JSON.parse(fs.readFileSync(recordUrl,'utf8')):null;
  if(record){
    assert.equal(record.chainId,10143);
    assert.equal(record.account.toLowerCase(),account.address.toLowerCase());
    assert.equal(record.spendMon,'10');
    if(record.hash){
      const receipt=await client.getTransactionReceipt({hash:record.hash}).catch(()=>null);
      if(receipt){
        const usdcAfter=await client.readContract({address:usdc,abi,functionName:'balanceOf',args:[account.address]});
        record.status=receipt.status;
        record.blockNumber=String(receipt.blockNumber);
        record.finalUSDC=String(usdcAfter);
        save(record);
        console.log(JSON.stringify({status:'existing '+receipt.status,transactionHash:record.hash,block:record.blockNumber,usdcBalance:formatUnits(usdcAfter,6)},null,2));return;
      }
      throw Error('Existing signed hash '+record.hash+' has no receipt. Do not broadcast again until nonce/tx is investigated.');
    }
  }
  console.log(JSON.stringify({stage:'preflight',...preflight},null,2));
  if(!process.argv.includes('--send'))return;
  const wallet=createWalletClient({account,chain:monadTestnet,transport:http(url,{timeout:30000,retryCount:1})});
  const request=await wallet.prepareTransactionRequest({account,to:router,data,value:nativeAmount,gas,maxFeePerGas,maxPriorityFeePerGas:0n});
  const raw=await wallet.signTransaction(request);
  const hash=keccak256(raw);
  record={...preflight,status:'signed',hash,dataHash:keccak256(data)};
  save(record);
  try {await client.sendRawTransaction({serializedTransaction:raw});record.status='broadcast';save(record);}
  catch(e){console.error('Tx may be pending:',hash);throw e;}
  const receipt=await client.waitForTransactionReceipt({hash,timeout:120000,pollingInterval:1500});
  const usdcAfter=await client.readContract({address:usdc,abi,functionName:'balanceOf',args:[account.address]});
  record={...record,status:receipt.status,blockNumber:String(receipt.blockNumber),gasUsed:String(receipt.gasUsed),
    chargedFeeMON:formatEther(BigInt(record.gasLimit)*receipt.effectiveGasPrice),
    walletUSDCAfter:formatUnits(usdcAfter,6),receivedUSDC:formatUnits(usdcAfter-usdcBefore,6),
    explorer:'https://testnet.monadscan.com/tx/'+hash};
  save(record);
  assert.equal(receipt.status,'success','swap reverted');
  assert.ok(usdcAfter-usdcBefore>=minOut,'USDC out below minimum');
  console.log(JSON.stringify(record,null,2));
}
main().catch(e=>{console.error(e.shortMessage||e.message);process.exitCode=1;});
