import fs from 'node:fs';
import assert from 'node:assert/strict';
import {createPublicClient, createWalletClient, http, encodeDeployData, encodeFunctionData, decodeFunctionResult, decodeEventLog, keccak256, formatEther, parseEther} from '../../frontend/node_modules/viem/_esm/index.js';
import {privateKeyToAccount} from '../../frontend/node_modules/viem/_esm/accounts/index.js';
import {monadTestnet} from '../../frontend/node_modules/viem/_esm/chains/index.js';

const root = new URL('./', import.meta.url);
const json = x => JSON.stringify(x, (_, v) => typeof v === 'bigint' ? String(v) : v, 2) + '\n';
const read = p => JSON.parse(fs.readFileSync(new URL(p, root), 'utf8'));
const hex = p => '0x' + fs.readFileSync(new URL(p, root)).toString('hex');
const artifact = name => read(`evm/out/${name}.sol/${name}.json`);
const url = process.env.MONAD_TESTNET_RPC_URL || 'https://testnet-rpc.monad.xyz';
const rpc = createPublicClient({chain: monadTestnet, transport: http(url, {timeout: 60000, retryCount: 0})});
const broadcast = process.argv.includes('--deploy');
const weights = hex('model-study/actor-int16.bin'), table = hex('model-study/tanh-q16.bin');
const identity = {weights: keccak256(weights), table: keccak256(table), checkpoint: read('pretrained-model-manifest.json')};
const statePath = new URL('model-study/testnet-deployment.json', root);
const preparedPath = new URL('model-study/testnet-preflight.json', root);
const budget = parseEther('20');
const factoryArt = artifact('ModelChunkFactory'), actorArt = artifact('ExperimentalPpoActor'), recorderArt = artifact('PpoDecisionRecorder');
let state;
const save = () => fs.writeFileSync(statePath, json(state));

async function main() {
  assert.equal(await rpc.getChainId(), 10143, 'Monad testnet only');
  const legacy = new URL('../../examples/monad-buy-usdc/.env.example', root);
  const text = fs.existsSync(legacy) ? fs.readFileSync(legacy, 'utf8') : '';
  const key = process.env.TESTNET_DEPLOYER_PRIVATE_KEY || text.match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1];
  if (!key) throw Error('Configure TESTNET_DEPLOYER_PRIVATE_KEY locally');
  const account = privateKeyToAccount(key.startsWith('0x') ? key : `0x${key}`);
  const wallet = createWalletClient({account, chain: monadTestnet, transport: http(url, {timeout: 60000, retryCount: 0})});
  const balance = await rpc.getBalance({address: account.address});
  const price = await rpc.getGasPrice();
  if (price > 160000000000n) throw Error('Testnet gas price exceeds cap');
  const plan = {chainId: 10143, deployer: account.address, balanceMon: formatEther(balance), gasPriceWei: price,
    weightBytes: (weights.length - 2) / 2, chunks: 4, tableBytes: (table.length - 2) / 2,
    spendingCapMon: '20', scope: 'Original actor int16/Q16. Public weights and on-chain inference. Synthetic vector verification. No DEX swaps.',
    identity, preparedAt: new Date().toISOString()};
  fs.writeFileSync(preparedPath, json(plan));
  if (!broadcast) { console.log(json(plan)); return; }
  state = fs.existsSync(statePath) ? read('model-study/testnet-deployment.json') : {
    ...plan, initialBalanceWei: String(balance), operations: {}, startedAt: new Date().toISOString(), status: 'deploying'};
  assert.equal(state.chainId, 10143);
  assert.equal(state.deployer.toLowerCase(), account.address.toLowerCase());
  assert.equal(state.identity.weights, identity.weights);
  assert.equal(state.identity.table, identity.table);
  save();

  async function transaction(label, data, to, ceiling = 30000000n) {
    assert.equal(await rpc.getChainId(), 10143);
    let op = state.operations[label];
    if (!op) {
      const gasEstimate = await rpc.estimateGas({account: account.address, data, ...(to ? {to} : {})});
      const gas = (gasEstimate * 105n + 99n) / 100n;
      const maxFeePerGas = (await rpc.getGasPrice()) * 120n / 100n;
      if (gas > ceiling || maxFeePerGas > 200000000000n) throw Error(`${label}: gas cap`);
      const spent = Object.values(state.operations).reduce((s, o) => s + BigInt(o.chargedFeeWei || 0), 0n);
      const maximumFee = gas * maxFeePerGas;
      if (spent + maximumFee > budget) throw Error(`${label}: total fee cap`);
      if (await rpc.getBalance({address: account.address}) < maximumFee + parseEther('0.1')) throw Error(`${label}: insufficient test MON`);
      const request = await wallet.prepareTransactionRequest({account, chain: monadTestnet, data, ...(to ? {to} : {}), gas,
        maxFeePerGas, maxPriorityFeePerGas: 0n, value: 0n});
      const serialized = await wallet.signTransaction(request);
      const hash = keccak256(serialized);
      op = {hash, dataHash: keccak256(data), to: to || null, gasEstimate: String(gasEstimate), gasLimit: String(gas),
        maxFeePerGas: String(maxFeePerGas), nonce: request.nonce, status: 'signed'};
      state.operations[label] = op; save();
      console.log(json({operation: label, estimatedGas: gasEstimate, maximumFeeMon: formatEther(maximumFee), transactionHash: hash}));
      await rpc.sendRawTransaction({serializedTransaction: serialized});
      op.status = 'submitted'; save();
    }
    assert.equal(op.dataHash, keccak256(data), `${label}: transaction changed`);
    assert.equal(op.to, to || null);
    // A saved hash is always resolved before another transaction is signed.
    // If submission was interrupted, inspect this hash and nonce before retrying.
    const receipt = await rpc.waitForTransactionReceipt({hash: op.hash, timeout: 60000, pollingInterval: 1000});
    op.status = receipt.status;
    op.gasUsed = String(receipt.gasUsed);
    op.blockNumber = String(receipt.blockNumber);
    op.contractAddress = receipt.contractAddress;
    op.effectiveGasPrice = String(receipt.effectiveGasPrice);
    op.chargedFeeWei = String(BigInt(op.gasLimit) * receipt.effectiveGasPrice);
    save();
    assert.equal(receipt.status, 'success', `${label}: reverted`);
    return receipt;
  }
  async function deploy(label, art, args = []) {
    const receipt = await transaction(label, encodeDeployData({abi: art.abi, bytecode: art.bytecode.object, args}));
    const address = receipt.contractAddress;
    assert.ok(address);
    const code = await rpc.getBytecode({address});
    assert.ok(code && code !== '0x');
    state.operations[label].runtimeHash = keccak256(code); save();
    return address;
  }
  const factory = await deploy('factory', factoryArt);
  const chunkAddresses = [];
  const payloads = [];
  for (let i = 2; i < weights.length; i += 114688 * 2) payloads.push('0x' + weights.slice(i, i + 114688 * 2));
  payloads.push(table);
  for (let i = 0; i < payloads.length; i++) {
    const data = payloads[i];
    const address = await rpc.readContract({address: factory, abi: factoryArt.abi, functionName: 'chunkAddress', args: [keccak256(data)]});
    await transaction(`chunk-${i}`, encodeFunctionData({abi: factoryArt.abi, functionName: 'createChunk', args: [data]}), factory);
    assert.equal((await rpc.getBytecode({address})).toLowerCase(), ('0x00' + data.slice(2)).toLowerCase());
    chunkAddresses.push(address);
  }
  const actor = await deploy('actor', actorArt, [chunkAddresses.slice(0, 4), chunkAddresses[4], identity.weights, identity.table]);
  const recorder = await deploy('recorder', recorderArt, [actor]);
  state.factory = factory; state.actor = actor; state.recorder = recorder; state.chunks = chunkAddresses; save();
  state.vectors = [];
  for (let i = 0; i < 6; i++) {
    const input = hex(`model-study/evm-vectors/${i}.bin`);
    const expected = read(`model-study/evm-vectors/${i}.json`);
    const data = encodeFunctionData({abi: actorArt.abi, functionName: 'predict', args: [input]});
    const result = await rpc.call({to: actor, data, gas: 29000000n});
    const [raw, action, size] = decodeFunctionResult({abi: actorArt.abi, functionName: 'predict', data: result.data});
    assert.deepEqual([raw[0], raw[1], BigInt(action), size], [BigInt(expected.raw0), BigInt(expected.raw1), BigInt(expected.action), BigInt(expected.size_q16)]);
    const gasEstimate = await rpc.estimateGas({account: account.address, to: actor, data});
    state.vectors.push({index: i, inputHash: keccak256(input), raw, action, sizeQ16: size, gasEstimate, matches: true}); save();
    if ([0, 4, 5].includes(i)) {
      const receipt = await transaction(`decision-${i}`, encodeFunctionData({abi: recorderArt.abi, functionName: 'decide', args: [input]}), recorder);
      const logs = receipt.logs.filter(l => l.address.toLowerCase() === recorder.toLowerCase());
      assert.equal(logs.length, 1);
      const event = decodeEventLog({abi: recorderArt.abi, data: logs[0].data, topics: logs[0].topics});
      assert.equal(event.args.inputHash, keccak256(input));
      assert.equal(event.args.caller.toLowerCase(), account.address.toLowerCase());
      assert.deepEqual([event.args.raw0, event.args.raw1, BigInt(event.args.action), event.args.sizeQ16], [raw[0], raw[1], BigInt(action), size]);
      state.operations[`decision-${i}`].eventVerified = true; save();
    }
  }
  state.finalBalanceWei = String(await rpc.getBalance({address: account.address}));
  state.balanceDecreaseMon = formatEther(BigInt(state.initialBalanceWei) - BigInt(state.finalBalanceWei));
  state.totalChargedFeeMon = formatEther(Object.values(state.operations).reduce((s, o) => s + BigInt(o.chargedFeeWei), 0n));
  state.explorer = `https://testnet.monadscan.com/address/${actor}`;
  state.status = 'verified'; state.verifiedAt = new Date().toISOString(); save();
  console.log(json({status: state.status, actor, recorder, vectorsMatched: state.vectors.length, totalChargedFeeMon: state.totalChargedFeeMon, balanceDecreaseMon: state.balanceDecreaseMon, explorer: state.explorer}));
}
main().catch(e => { console.error(e.shortMessage || e.message); process.exitCode = 1; });
