// Off-chain bounded Monad Testnet keeper. Only executes sessions that wallets started.
// For development: node research/ai/showcase/keeper.mjs
// For detached mode: node research/ai/showcase/keeper.mjs --daemon
import fs from 'node:fs';
import path from 'node:path';
import {spawn} from 'node:child_process';
import assert from 'node:assert/strict';
import {fileURLToPath} from 'node:url';
import {createPublicClient,createWalletClient,http,parseAbi,formatEther,parseEther} from '../../../frontend/node_modules/viem/_esm/index.js';
import {privateKeyToAccount} from '../../../frontend/node_modules/viem/_esm/accounts/index.js';
import {monadTestnet} from '../../../frontend/node_modules/viem/_esm/chains/index.js';
const here=new URL('./',import.meta.url);
const dir=fileURLToPath(here);
const config=JSON.parse(fs.readFileSync(new URL('../../../frontend/public/ai/session-config.json',here),'utf8'));
const frames=JSON.parse(fs.readFileSync(new URL('../monad-small/vectors.json',here),'utf8'));
const sequence=[0,2,8,7,3,8].map(i=>frames[i].input_q12);
const rpcUrl=process.env.MONAD_TESTNET_RPC_URL||'https://testnet-rpc.monad.xyz';
const publicClient=createPublicClient({chain:monadTestnet,transport:http(rpcUrl,{timeout:18000,retryCount:1})});
const abi=parseAbi([
 'event SessionStarted(address indexed wallet,uint64 indexed sessionId,uint64 startedAt,uint64 expiresAt)',
 'event Decision(address indexed wallet,uint64 indexed sessionId,uint8 indexed round,uint8 action,bytes32 inputHash,uint256 amountIn,uint256 amountOut,int256[3] scores)',
 'function sessions(address) view returns(uint64 id,uint64 startedAt,uint64 lastExecution,uint8 nextRound,bool active)',
 'function executeFor(address,uint64,uint8,int32[12],uint256) returns(uint8,uint256)'
]);
const modelAbi=parseAbi(['function predict(int32[12]) view returns(int256[3],uint8)']);
const routerAbi=parseAbi(['function getAmountsOut(uint256,address[]) view returns(uint256[])']);
const store=new URL('./keeper-runtime.json',here);
const log=(...v)=>console.log(new Date().toISOString(),...v);
const wait=ms=>new Promise(resolve=>setTimeout(resolve,ms));
function save(obj){fs.writeFileSync(store,JSON.stringify(obj,null,2)+'\n');}
function secret(){
  const k=process.env.MONAD_TESTNET_KEEPER_PRIVATE_KEY||
    process.env.TESTNET_DEPLOYER_PRIVATE_KEY||
    fs.readFileSync(new URL('../../../examples/monad-buy-usdc/.env.example',here),'utf8').match(/^PRIVATE_KEY\s*=\s*(\S+)$/m)?.[1];
  assert.match(k||'',/^(0x)?[A-Fa-f0-9]{64}$/,'Keeper TESTNET key missing');
  return privateKeyToAccount(k.startsWith('0x')?k:'0x'+k);
}
if(process.argv.includes('--daemon')){
  const pidFile=path.join(dir,'keeper.pid');
  if(fs.existsSync(pidFile)){
    const pid=Number(fs.readFileSync(pidFile,'utf8'));
    try{process.kill(pid,0);throw Error('Keeper already running, PID '+pid);}catch(e){if(e.code!=='ESRCH')throw e;}
  }
  const logFd=fs.openSync(path.join(dir,'keeper.log'),'a');
  const child=spawn(process.execPath,[fileURLToPath(import.meta.url),'--run'],{
    cwd:process.cwd(),env:process.env,detached:true,stdio:['ignore',logFd,logFd]
  });
  child.unref();
  fs.writeFileSync(pidFile,String(child.pid));
  console.log('Keeper started, PID',child.pid,'log: research/ai/showcase/keeper.log');
  process.exit(0);
}
const signer=secret();
assert.equal(signer.address.toLowerCase(),config.keeper.toLowerCase(),'Keeper key does not match on-chain keeper');
const walletClient=createWalletClient({account:signer,chain:monadTestnet,transport:http(rpcUrl,{timeout:18000,retryCount:1})});
const known=new Map(),unresolved=new Map();
const old=fs.existsSync(store)?JSON.parse(fs.readFileSync(store,'utf8')):{};
for(const item of old.wallets||[])known.set(item.wallet.toLowerCase(),item.wallet);
for(const entry of old.unresolved||[])unresolved.set(entry.key,entry.tx);
const persist=()=>save({wallets:[...known.values()].map(wallet=>({wallet})),unresolved:[...unresolved.entries()].map(([key,tx])=>({key,tx})),updatedAt:new Date().toISOString()});
async function checkSessions(){
  const height=await publicClient.getBlockNumber();
  const min=height>70n?height-70n:0n;
  const logs=await publicClient.getContractEvents({address:config.contract,abi,
    eventName:'SessionStarted',fromBlock:min,toBlock:height});
  for(const item of logs)known.set(item.args.wallet.toLowerCase(),item.args.wallet);
  persist();
  for(const user of known.values()){
    const [id,startedAt,lastExecution,nextRound,active]=await publicClient.readContract({
      address:config.contract,abi,functionName:'sessions',args:[user]});
    const step=Number(nextRound),key=user.toLowerCase()+':'+id+':'+step;
    if(unresolved.has(key)){
      const h=unresolved.get(key);
      const rcpt=await publicClient.getTransactionReceipt({hash:h}).catch(()=>null);
      if(!rcpt){log('pending or unknown tx for',key,h);continue;}
      if(rcpt.status!=='success'){log('reverted, manual inspection required',key,h);continue;}
      unresolved.delete(key);persist();
      continue;
    }
    if(!active||step>=6)continue;
    const now=Math.floor(Date.now()/1000);
    if(now>Number(startedAt)+110)continue;
    if(now<Number(startedAt)+step*10 || (lastExecution>0n&&now<Number(lastExecution)+10))continue;
    const vector=sequence[step];
    const [,act]=await publicClient.readContract({address:config.model,abi:modelAbi,
      functionName:'predict',args:[vector]});
    const action=Number(act);
    let minOut=0n;
    if(action){
      const from=action===1?config.usdc:config.wmon;
      const to=action===1?config.wmon:config.usdc;
      const amount=action===1?400000n:100000000000000000n;
      const amounts=await publicClient.readContract({address:config.router,abi:routerAbi,
        functionName:'getAmountsOut',args:[amount,[from,to]]});
      minOut=amounts[1]*995n/1000n; // 0.5% below fresh quote
      if(minOut===0n)throw Error('DEX quote unavailable');
    }
    const args=[user,id,step,vector,minOut];
    const estimated=await publicClient.estimateContractGas({address:config.contract,abi,
      functionName:'executeFor',args,account:signer.address});
    const gas=estimated*118n/100n+1n;
    const gasPrice=await publicClient.getGasPrice();
    const maxFee=gasPrice*120n/100n+1n;
    assert.ok(gas<=650000n && maxFee<=200000000000n &&
      gas*maxFee<=parseEther('0.13'),'Round gas cap exceeded');
    const hash=await walletClient.writeContract({address:config.contract,abi,
      functionName:'executeFor',args,gas,maxFeePerGas:maxFee,maxPriorityFeePerGas:0n});
    unresolved.set(key,hash);persist();
    log('submitted',key,'action',action,'tx',hash);
    const receipt=await publicClient.waitForTransactionReceipt({hash,timeout:65000,pollingInterval:1200});
    if(receipt.status!=='success')throw Error('reverted tx '+hash);
    unresolved.delete(key);persist();
    log('confirmed',key,'block',String(receipt.blockNumber),'paidMaxGasFee',formatEther(gas*receipt.effectiveGasPrice));
  }
  fs.writeFileSync(new URL('../../../frontend/public/ai/keeper-status.json',here), JSON.stringify({
    chainId:10143,contract:config.contract,keeper:signer.address,online:true,
    checkedAt:new Date().toISOString(),walletsKnown:known.size
  },null,2)+'\n');
}
async function main(){
  assert.equal(await publicClient.getChainId(),10143,'only testnet 10143');
  assert.ok(await publicClient.getBytecode({address:config.contract}),'session contract missing');
  log('keeper watching',config.contract,'from',signer.address);
  if(process.argv.includes('--once')) {await checkSessions(); log('one-pass check complete'); return;}
  let failures=0;
  while(true){
    try{await checkSessions();failures=0;}
    catch(e){failures++;log('error',e.shortMessage||e.message);if(failures>=8)throw Error('Too many consecutive keeper errors');}
    await wait(1200);
  }
}
main().catch(e=>{log('STOPPED',e.message);process.exitCode=1;});
