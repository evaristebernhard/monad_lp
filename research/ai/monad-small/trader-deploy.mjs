import fs from 'node:fs';
import assert from 'node:assert/strict';
import {
  createPublicClient, createWalletClient, http, encodeDeployData,
  keccak256, formatEther, parseEther, parseAbi
} from '../../../frontend/node_modules/viem/_esm/index.js';
import {privateKeyToAccount} from '../../../frontend/node_modules/viem/_esm/accounts/index.js';
import {monadTestnet} from '../../../frontend/node_modules/viem/_esm/chains/index.js';

const root = new URL('./', import.meta.url);
const artifact = JSON.parse(fs.readFileSync(new URL('../evm/out/TinyMonadTrader.sol/TinyMonadTrader.json', root), 'utf8'));
const constants = Object.freeze({
  model: '0x5dd31af6d70b7633fd5da3e30c03feb5fde00618',
  router: '0x430c23895c8D44883526e3E0B09327dAD8766660',
  usdc: '0x534b2f3A21130d7a60830c2Df862319e593943A3',
  wmon: '0x97B3070F9Da6C002343862b35E68Bd8e22608943',
  modelSha256: '0x183fa936ef593253276e4baa3fa8ecb6e1a5334f6dd2df07e38235cd9b8cc8b7',
  maxBuyUSDC: 1_000_000n, // 1 USDC, 6 decimals
  maxSellWMON: 250_000_000_000_000_000n // 0.25 WMON, 18 decimals
});
const recordPath = new URL('trader-deployment.json', root);
const save = record => fs.writeFileSync(recordPath, JSON.stringify(record, (_, v) => typeof v === 'bigint' ? String(v) : v, 2) + '\n');
const client = createPublicClient({
  chain: monadTestnet,
  transport: http(process.env.MONAD_TESTNET_RPC_URL || 'https://testnet-rpc.monad.xyz', {timeout: 30000, retryCount: 1})
});
const abi = parseAbi([
  'function MODEL_SHA256() view returns(bytes32)',
  'function WETH() view returns(address)',
  'function decimals() view returns(uint8)',
  'function getAmountsOut(uint256,address[]) view returns(uint256[])',
  'function owner() view returns(address)',
  'function keeper() view returns(address)',
  'function model() view returns(address)',
  'function router() view returns(address)',
  'function usdc() view returns(address)',
  'function wmon() view returns(address)',
  'function maxBuyUSDC() view returns(uint256)',
  'function maxSellWMON() view returns(uint256)'
]);
async function inspect(address, deployer) {
  const runtime = await client.getBytecode({address});
  assert.ok(runtime && runtime.length > 2, 'No contract bytecode at ' + address);
  const actual = {};
  for (const property of ['owner','keeper','model','router','usdc','wmon','maxBuyUSDC','maxSellWMON']) {
    actual[property] = String(await client.readContract({address, abi, functionName: property}));
  }
  for (const property of ['owner','keeper']) assert.equal(actual[property].toLowerCase(), deployer.toLowerCase(), property);
  for (const property of ['model','router','usdc','wmon']) assert.equal(actual[property].toLowerCase(), constants[property].toLowerCase(), property);
  for (const property of ['maxBuyUSDC','maxSellWMON']) assert.equal(actual[property], String(constants[property]), property);
  return {address, runtimeCodeHash: keccak256(runtime), actual};
}
async function main() {
  assert.equal(await client.getChainId(), 10143, 'TESTNET ONLY');
  const local = new URL('../../../examples/monad-buy-usdc/.env.example',root);
  let key = process.env.TESTNET_DEPLOYER_PRIVATE_KEY;
  // Follow the existing project deployer's local configuration. Never print secrets.
  if (!key && fs.existsSync(local)) key = fs.readFileSync(local, 'utf8').match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1];
  if (!key) throw Error('Missing TESTNET_DEPLOYER_PRIVATE_KEY');
  const account = privateKeyToAccount(key.startsWith('0x') ? key : '0x' + key);
  assert.equal(
    (await client.readContract({address: constants.model,abi,functionName:'MODEL_SHA256'})).toLowerCase(),
    constants.modelSha256
  );
  assert.equal(
    (await client.readContract({address: constants.router,abi,functionName:'WETH'})).toLowerCase(),
    constants.wmon.toLowerCase()
  );
  assert.equal(await client.readContract({address: constants.usdc, abi, functionName:'decimals'}), 6);
  assert.equal(await client.readContract({address: constants.wmon, abi, functionName:'decimals'}), 18);
  for (const name of ['model','router','usdc','wmon']) {
    assert.ok((await client.getBytecode({address: constants[name]}))?.length > 2, 'Missing ' + name + ' code');
  }
  const args = [constants.model,account.address,constants.router,constants.usdc,constants.wmon,constants.maxBuyUSDC,constants.maxSellWMON];
  const data = encodeDeployData({abi: artifact.abi, bytecode: artifact.bytecode.object, args});
  const dataHash = keccak256(data);
  let record = fs.existsSync(recordPath) ? JSON.parse(fs.readFileSync(recordPath, 'utf8')) : null;
  if (record) {
    assert.equal(record.chainId,10143);
    assert.equal(record.deployer.toLowerCase(), account.address.toLowerCase());
    assert.equal(record.deployDataHash, dataHash);
    if (record.address) {
      const check=await inspect(record.address,account.address);
      console.log(JSON.stringify({status:'VERIFIED_EXISTING',transactionHash:record.txHash,...check},null,2));
      return;
    }
  }
  const balance = await client.getBalance({address:account.address});
  const gasEstimate = await client.estimateGas({account: account.address, data});
  const gasLimit = (gasEstimate * 110n + 99n) / 100n;
  const gasPrice = await client.getGasPrice();
  const maxFeePerGas = (gasPrice * 120n + 99n) / 100n;
  const reservedCost = gasLimit * maxFeePerGas;
  assert.ok(gasEstimate <= 3_000_000n && gasLimit <= 3_300_000n, 'Deployment gas above cap');
  assert.ok(reservedCost <= parseEther('0.5'), 'Deployment cost above 0.5 test MON cap');
  assert.ok(balance > reservedCost + parseEther('0.2'), 'Insufficient MON gas reserve');
  const preflight = {
    chainId:10143,deployer:account.address,model:constants.model,router:constants.router,
    usdc:constants.usdc,wmon:constants.wmon,maxBuyUSDC:String(constants.maxBuyUSDC),
    maxSellWMON:String(constants.maxSellWMON),deployDataHash:dataHash,
    nativeBalanceMON:formatEther(balance),gasEstimate:String(gasEstimate),
    gasLimit:String(gasLimit),gasPrice:String(gasPrice),maxFeePerGas:String(maxFeePerGas),
    maxFeeMON:formatEther(reservedCost)
  };
  console.log(JSON.stringify({status:'PREFLIGHT',...preflight},null,2));
  if (!process.argv.includes('--deploy')) return;
  if (!record) {
    const wallet = createWalletClient({
      chain: monadTestnet, account,
      transport: http(process.env.MONAD_TESTNET_RPC_URL || 'https://testnet-rpc.monad.xyz', {timeout: 30000, retryCount: 0})
    });
    const request = await wallet.prepareTransactionRequest({
      account, data, gas:gasLimit, maxFeePerGas, maxPriorityFeePerGas:0n, value:0n
    });
    const signed = await wallet.signTransaction(request);
    const txHash = keccak256(signed);
    record = {...preflight,txHash,address:null,status:'signed',signedAt:new Date().toISOString()};
    save(record); // Save tx hash before sending to prevent duplicate deployment.
    await client.sendRawTransaction({serializedTransaction:signed});
    record.status='broadcast';
    save(record);
    console.log(JSON.stringify({status:'BROADCAST',txHash},null,2));
  }
  // A rerun resumes the same transaction without re-signing.
  const receipt = await client.waitForTransactionReceipt({hash:record.txHash,timeout:120000,pollingInterval:1300});
  record.receiptStatus=receipt.status;
  record.blockNumber=String(receipt.blockNumber);
  record.gasUsed=String(receipt.gasUsed);
  record.effectiveGasPrice=String(receipt.effectiveGasPrice);
  record.chargedFeeMON=formatEther(BigInt(record.gasLimit)*receipt.effectiveGasPrice);
  record.address=receipt.contractAddress;
  record.status=receipt.status==='success'?'mined':'failed';
  save(record);
  assert.equal(receipt.status,'success','Deployment reverted');
  const result=await inspect(receipt.contractAddress,account.address);
  record.runtimeCodeHash=result.runtimeCodeHash;
  record.status='verified';record.verifiedAt=new Date().toISOString();
  save(record);
  console.log(JSON.stringify({status:'VERIFIED',txHash:record.txHash,blockNumber:record.blockNumber,chargedFeeMON:record.chargedFeeMON,...result},null,2));
}
main().catch(error=>{console.error(error.shortMessage || error.message);process.exitCode=1;});
