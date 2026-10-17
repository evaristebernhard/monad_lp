import fs from 'node:fs';
import assert from 'node:assert/strict';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {createPublicClient, createWalletClient, http, encodeDeployData, encodeFunctionData, decodeEventLog, keccak256, formatEther, parseEther} from '../../../frontend/node_modules/viem/_esm/index.js';
import {privateKeyToAccount} from '../../../frontend/node_modules/viem/_esm/accounts/index.js';
import {monadTestnet} from '../../../frontend/node_modules/viem/_esm/chains/index.js';
const root = process.env.MONAD_AI_RUN_DIR ? pathToFileURL(resolve(process.env.MONAD_AI_RUN_DIR) + '/') : new URL('./', import.meta.url);
const read = p => JSON.parse(fs.readFileSync(new URL(p, root), 'utf8'));
const json = x => JSON.stringify(x, (_, v) => typeof v === 'bigint' ? String(v) : v, 2) + '\n';
const artifact = read('../evm/out/TinyMonadPolicy.sol/TinyMonadPolicy.json');
const report = read('report.json'), vectors = read('vectors.json');
const path = new URL('deployment.json', root);
const rpcUrl = process.env.MONAD_TESTNET_RPC_URL || 'https://testnet-rpc.monad.xyz';
const client = createPublicClient({chain: monadTestnet, transport: http(rpcUrl, {timeout: 30000, retryCount: 0})});
let record;
const save = () => fs.writeFileSync(path, json(record));
async function main() {
  assert.equal(await client.getChainId(), 10143, 'Testnet only');
  const local = new URL('../../../examples/monad-buy-usdc/.env.example', root);
  const key = process.env.TESTNET_DEPLOYER_PRIVATE_KEY || fs.readFileSync(local, 'utf8').match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1];
  if (!key) throw Error('Missing local testnet deployer key');
  const account = privateKeyToAccount(key.startsWith('0x') ? key : `0x${key}`);
  const wallet = createWalletClient({account, chain: monadTestnet, transport: http(rpcUrl, {timeout: 30000, retryCount: 0})});
  record = fs.existsSync(path) ? read('deployment.json') : {chainId: 10143, sourceChainId: 143, deployer: account.address,
    modelSha256: report.model_sha256, datasetSha256: report.dataset_sha256, operations: {}, status: 'deploying'};
  assert.equal(record.modelSha256, report.model_sha256);
  assert.equal(record.datasetSha256, report.dataset_sha256);
  assert.equal(record.deployer.toLowerCase(), account.address.toLowerCase());
  const startBalance = await client.getBalance({address: account.address});
  record.initialBalanceWei ||= String(startBalance);
  save();
  async function send(label, data, to) {
    assert.equal(await client.getChainId(), 10143);
    let op = record.operations[label];
    if (!op) {
      const gas = ((await client.estimateGas({account: account.address, data, ...(to ? {to} : {})})) * 110n + 99n) / 100n;
      const price = await client.getGasPrice() * 120n / 100n;
      if (gas > 1500000n || price > 200000000000n) throw Error('Testnet fee cap');
      const spent = Object.values(record.operations).reduce((s, o) => s + BigInt(o.feeWei || 0), 0n);
      if (spent + gas * price > parseEther('0.5')) throw Error('Total testnet budget cap: 0.5 MON');
      const request = await wallet.prepareTransactionRequest({account, data, ...(to ? {to} : {}), gas, maxFeePerGas: price, maxPriorityFeePerGas: 0n, value: 0n});
      const serializedTransaction = await wallet.signTransaction(request);
      const hash = keccak256(serializedTransaction);
      op = {hash, dataHash: keccak256(data), gasLimit: String(gas), status: 'signed'};
      record.operations[label] = op; save();
      await client.sendRawTransaction({serializedTransaction});
      op.status = 'submitted'; save();
      console.log(json({label, hash, maximumFeeMon: formatEther(gas * price)}));
    }
    assert.equal(op.dataHash, keccak256(data));
    const receipt = await client.waitForTransactionReceipt({hash: op.hash, timeout: 60000, pollingInterval: 1000});
    Object.assign(op, {status: receipt.status, gasUsed: String(receipt.gasUsed), effectiveGasPrice: String(receipt.effectiveGasPrice),
      feeWei: String(BigInt(op.gasLimit) * receipt.effectiveGasPrice), contractAddress: receipt.contractAddress, blockNumber: String(receipt.blockNumber)});
    save(); assert.equal(receipt.status, 'success'); return receipt;
  }
  const receipt = await send('deploy', encodeDeployData({abi: artifact.abi, bytecode: artifact.bytecode.object}));
  const address = receipt.contractAddress;
  record.address = address;
  record.runtimeCodeHash = keccak256(await client.getBytecode({address}));
  assert.equal(await client.readContract({address, abi: artifact.abi, functionName: 'MODEL_SHA256'}), `0x${report.model_sha256}`);
  assert.equal(await client.readContract({address, abi: artifact.abi, functionName: 'DATASET_SHA256'}), `0x${report.dataset_sha256}`);
  record.vectors = [];
  for (let i = 0; i < vectors.length; i++) {
    const vector = vectors[i];
    const args = [vector.input_q12];
    const [scores, action] = await client.readContract({address, abi: artifact.abi, functionName: 'predict', args});
    assert.deepEqual(scores.map(String), vector.scores_q12.map(String)); assert.equal(action, vector.action);
    const gas = await client.estimateContractGas({account: account.address, address, abi: artifact.abi, functionName: 'predict', args});
    record.vectors.push({index: i, asset: vector.asset, timestamp: vector.timestamp, action, scores: scores.map(String), gasEstimate: String(gas), matches: true}); save();
  }
  for (const asset of report.assets.map(a => a.symbol)) {
    const vector = vectors.filter(v => v.asset === asset).at(-1);
    const receipt = await send(`decision-${asset}`, encodeFunctionData({abi: artifact.abi, functionName: 'decide', args: [vector.input_q12]}), address);
    const log = receipt.logs.find(l => l.address.toLowerCase() === address.toLowerCase());
    assert.ok(log);
    const event = decodeEventLog({abi: artifact.abi, topics: log.topics, data: log.data});
    assert.equal(event.eventName, 'Decision'); assert.equal(event.args.action, vector.action);
    assert.deepEqual([event.args.score0, event.args.score1, event.args.score2].map(String), vector.scores_q12.map(String));
    assert.equal(event.args.caller.toLowerCase(), account.address.toLowerCase());
    record.operations[`decision-${asset}`].eventVerified = true; save();
  }
  record.finalBalanceWei = String(await client.getBalance({address: account.address}));
  record.totalFeeMon = formatEther(Object.values(record.operations).reduce((s, o) => s + BigInt(o.feeWei), 0n));
  record.balanceDecreaseMon = formatEther(BigInt(record.initialBalanceWei) - BigInt(record.finalBalanceWei));
  record.explorer = `https://testnet.monadscan.com/address/${address}`;
  record.verifiedAt = new Date().toISOString(); record.status = 'verified'; save();
  console.log(json(record));
}
main().catch(e => {console.error(e.shortMessage || e.message); process.exitCode = 1;});
