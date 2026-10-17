import React, {useEffect,useState} from 'react'
import {createPublicClient, createWalletClient, custom, http, formatUnits, keccak256,decodeEventLog} from 'viem'
import {monadTestnet} from 'viem/chains'
import data from './generated/selection.json'
import {buildInput,solveDcar,json} from './poolSelection.js'
import './poolSelection.css'
import {selectV4} from './selectionQuotes.js'
import useLiveSelection from './useLiveSelection.js'
import useSelectionWallet from './useSelectionWallet.js'
import {buildBestBuy,selectBestFrame,verifyBestQuote} from './bestSwap.js'
import {ensureMonad} from './monadTrade.js'
import {monad} from 'viem/chains'
const tradeClient=createPublicClient({chain:monad,transport:http('/api/monad/trade-rpc',{timeout:20000,retryCount:0})})
const client=createPublicClient({chain:monadTestnet,transport:http('/api/selection/testnet-rpc',{timeout:45000,retryCount:0})})
const short=s=>`${s.slice(0,10)}…${s.slice(-6)}`
export default function PoolSelection(){
 const [mode,setMode]=useState('v4'),[swapAmount,setSwapAmount]=useState('0.1'),[payToken,setPayToken]=useState('MON'),[amount,setAmount]=useState('0.1'),[splits,setSplits]=useState(3)
 const [dcarResult,setResult]=useState(null),[validation,setValidation]=useState(null),[autoRefresh,setAutoRefresh]=useState(true),[busy,setBusy]=useState(false),[error,setError]=useState(''),[status,setStatus]=useState('')
 const [addresses,setAddresses]=useState(()=>{try{return JSON.parse(localStorage.getItem('selection-addresses-v2'))||{}}catch{return {}}}),[tx,setTx]=useState(()=>localStorage.getItem('selection-pending-v1')||'')
 const wallet=useSelectionWallet()
 const [paidInfo,setPaidInfo]=useState(null),[paidError,setPaidError]=useState(''),[paidTx,setPaidTx]=useState(()=>localStorage.getItem('selection-paid-pending-v1')||''),[paidStatus,setPaidStatus]=useState(''),[buyTx,setBuyTx]=useState(()=>localStorage.getItem('selection-buy-pending-v1')||''),[buyStatus,setBuyStatus]=useState('')
 const paid=data.paid,paidAddress=paid.reference.contractAddress
 useEffect(()=>{
  let cancelled=false
  async function load(){try{
   if(await client.getChainId()!==10143)throw Error('收费合约网络错误')
   const [creator,fee,code]=await Promise.all([client.readContract({address:paidAddress,abi:paid.abi,functionName:'creator'}),client.readContract({address:paidAddress,abi:paid.abi,functionName:'serviceFee'}),client.getBytecode({address:paidAddress})])
   if(!code||creator.toLowerCase()!==paid.reference.creator.toLowerCase()||String(fee)!==paid.reference.serviceFeeWei)throw Error('收费合约配置不匹配')
   if(!cancelled)setPaidInfo({creator,fee})
  }catch(e){if(!cancelled)setPaidError(e.shortMessage||e.message)}}
  load();return()=>{cancelled=true}
 },[paidAddress,paid])
 const {account,chain,balance}=wallet
 const networkLabel=chain===10143?'Monad 测试网':chain===143?'Monad 主网':chain?`网络 ${chain}`:'未连接'
 const artifact=data[mode], address=addresses[mode]||artifact.reference?.contractAddress
 const live=useLiveSelection({candidates:data.routes.candidates,amount:swapAmount,payToken,enabled:mode==='v4'&&autoRefresh,paused:busy})
 const snapshot=live.snapshot,result=mode==='v4'?live.decision:dcarResult
 const verified=validation?.mode===mode&&validation.hash===result?.values.at(-1)
 const reset=()=>{setResult(null);setValidation(null);setError('');setStatus('')}
 function local(source=snapshot){
  if(mode==='dcar'){const args=buildInput(data.sample,amount,splits);return {args,values:solveDcar(args)}}
  return selectV4(source)
 }
 async function run(action){setBusy(true);setError('');try{await action()}catch(e){setStatus('');setError(e.shortMessage||e.message||'操作失败')}finally{setBusy(false)}}
 async function verify(){
  const r=local()
  setValidation(null);setStatus('核对中…')
  if(await client.getChainId()!==10143)throw Error('RPC 网络不正确')
  if(!address)throw Error('请先部署合约')
  const code=await client.getBytecode({address}),expected=address.toLowerCase()===artifact.reference?.contractAddress?.toLowerCase()?artifact.reference.runtimeCodeHash:mode==='dcar'?keccak256(artifact.runtime):null
  const normalize=(hex)=>{let text=hex.slice(2);for(const refs of Object.values(artifact.immutableReferences||{}))for(const ref of refs)text=text.slice(0,ref.start*2)+'0'.repeat(ref.length*2)+text.slice((ref.start+ref.length)*2);return '0x'+text}
  if(!code||(expected?keccak256(code)!==expected:normalize(code)!==normalize(artifact.runtime)))throw Error('合约代码与已知版本不一致')
  // V4 has constructor immutables: verify its fixed candidate universe as well.
  if(mode==='v4'){
   const ids=data.routes.candidates.map(q=>q.pool)
   const count=await client.readContract({address,abi:artifact.abi,functionName:'poolCount'})
   const allowed=await Promise.all(ids.map(id=>client.readContract({address,abi:artifact.abi,functionName:'allowedPool',args:[id]})))
   if(count!==BigInt(ids.length)||allowed.some(v=>!v))throw Error('候选池白名单不匹配')
  }
  const remote=await client.readContract({address,abi:artifact.abi,functionName:mode==='dcar'?'solve':'selectBest',args:r.args,gas:28000000n})
  if(json(remote)!==json(r.values))throw Error('本地和链上结果不一致')
  if(mode==='dcar')setResult(r)
  setValidation({mode,hash:r.values.at(-1),block:mode==='v4'?snapshot.sourceBlock:null});setStatus('核对通过')
 }
 async function finishDeployment(hash,kind){
  setStatus('等待测试网部署确认…')
  const receipt=await client.waitForTransactionReceipt({hash,timeout:90000})
  if(receipt.status!=='success'||!receipt.contractAddress)throw Error('部署失败')
  setAddresses(old=>{const next={...old,[kind]:receipt.contractAddress};localStorage.setItem('selection-addresses-v2',JSON.stringify(next));return next})
  localStorage.removeItem('selection-pending-v1');wallet.refreshBalance();setStatus('部署成功，可以进行链上核对')
 }
 async function deploy(){
  if(localStorage.getItem('selection-pending-v1'))throw Error('存在待确认部署，请先恢复确认')
  const provider=wallet.provider,who=await wallet.connect()
  await wallet.switchTestnet()
  const accounts=await provider.request({method:'eth_accounts'})
  if(accounts[0]?.toLowerCase()!==who.toLowerCase()||await provider.request({method:'eth_chainId'})!=='0x279f')throw Error('钱包账户或网络已改变，请重试')
  const signer=createWalletClient({account:who,chain:monadTestnet,transport:custom(provider)}),kind=mode
  const gasPrice=await client.getGasPrice()
  if(gasPrice>160000000000n)throw Error('测试网 gas 价格过高，请稍后重试')
  const hash=await signer.deployContract({abi:artifact.abi,bytecode:artifact.bytecode,args:kind==='v4'?[data.routes.candidates.map(q=>q.pool)]:[],gas:2000000n,maxFeePerGas:gasPrice*120n/100n,maxPriorityFeePerGas:0n})
  localStorage.setItem('selection-pending-v1',hash);localStorage.setItem('selection-pending-kind-v1',kind);setTx(hash)
  await finishDeployment(hash,kind)
 }
 async function confirmPayment(hash){
  setPaidStatus('交易已提交，等待确认…')
  const receipt=await client.waitForTransactionReceipt({hash,timeout:90000})
  if(receipt.status!=='success'){
   localStorage.removeItem('selection-paid-pending-v1');setPaidStatus('交易失败，服务费已回滚；gas 已消耗');return
  }
  const event=receipt.logs.filter(log=>log.address.toLowerCase()===paidAddress.toLowerCase()).flatMap(log=>{try{return [decodeEventLog({abi:paid.abi,data:log.data,topics:log.topics})]}catch{return []}}).find(e=>e.eventName==='SelectionPaid')
  if(!event||event.args.fee!==BigInt(paid.reference.serviceFeeWei)||event.args.recipient.toLowerCase()!==paid.reference.creator.toLowerCase())throw Error('未找到匹配的服务费到账事件，请查看交易')
  localStorage.removeItem('selection-paid-pending-v1');setPaidStatus('调用成功，MON 服务费已转给创建者');wallet.refreshBalance()
 }
 async function payForSelection(){
  if(localStorage.getItem('selection-paid-pending-v1'))throw Error('存在待确认付费交易，请先查询结果')
  if(!paidInfo||live.stale||!snapshot)throw Error('请等待有效报价及收费配置')
  const decision=selectV4(snapshot),provider=wallet.provider,who=await wallet.connect()
  await wallet.switchTestnet()
  const accounts=await provider.request({method:'eth_accounts'})
  if(accounts[0]?.toLowerCase()!==who.toLowerCase()||Number(await provider.request({method:'eth_chainId'}))!==10143)throw Error('钱包账户或网络已改变，请重试')
  const request={account:who,address:paidAddress,abi:paid.abi,functionName:'selectAndPay',args:decision.args,value:paidInfo.fee}
  await client.simulateContract(request)
  const gas=await client.estimateContractGas(request),price=await client.getGasPrice()
  if(gas>1000000n||price>160000000000n)throw Error('测试网交易费用过高，请稍后重试')
  const signer=createWalletClient({account:who,chain:monadTestnet,transport:custom(provider)})
  const hash=await signer.writeContract({...request,gas:gas*120n/100n,maxFeePerGas:price*120n/100n,maxPriorityFeePerGas:0n})
  localStorage.setItem('selection-paid-pending-v1',hash);setPaidTx(hash)
  await confirmPayment(hash)
 }
 async function finishBuy(hash){
  setBuyStatus('等待主网兑换确认…')
  const receipt=await tradeClient.waitForTransactionReceipt({hash,timeout:90000})
  localStorage.removeItem('selection-buy-pending-v1')
  setBuyStatus(receipt.status==='success'?'Buy 成功，USDC 已转入你的钱包':'兑换回滚，本金未扣除；gas 已消耗')
 }
 async function buyBest(){
  if(localStorage.getItem('selection-buy-pending-v1'))throw Error('有待确认兑换，请先查询交易结果')
  if(payToken!=='MON')throw Error('目前 Buy 仅支持 MON → USDC')
  const provider=wallet.provider,who=await wallet.connect()
  await ensureMonad(provider)
  const response=await fetch('/api/selection/quote?'+new URLSearchParams({amount:swapAmount,token_in:'MON'}),{signal:AbortSignal.timeout(15000)})
  if(!response.ok)throw Error('获取最优报价失败')
  const quote=selectBestFrame(await response.json(),data.routes.candidates,swapAmount)
  await verifyBestQuote(tradeClient,quote)
  const block=await tradeClient.getBlock(),{minimum,...request}=buildBestBuy({...quote,wallet:who,deadline:block.timestamp+120n})
  const [gas,gasPrice]=await Promise.all([tradeClient.estimateGas(request),tradeClient.getGasPrice()])
  if(gas>1000000n||gasPrice>200000000000n)throw Error('主网 gas 超过上限，请稍后重试')
  const accounts=await provider.request({method:'eth_accounts'})
  if(accounts[0]?.toLowerCase()!==who.toLowerCase()||Number(await provider.request({method:'eth_chainId'}))!==143)throw Error('钱包账户或网络已改变')
  if(Date.now()-Number(quote.frame.sourceTimestamp)*1000>30000)throw Error('报价已过期，请重新 Buy')
  const signer=createWalletClient({account:who,chain:monad,transport:custom(provider)})
  setBuyStatus(`最优池 ${quote.route.protocol} · 最低到账 ${formatUnits(minimum,6)} USDC，等待钱包确认…`)
  const hash=await signer.sendTransaction({...request,gas:gas*120n/100n,maxFeePerGas:gasPrice*120n/100n,maxPriorityFeePerGas:0n})
  localStorage.setItem('selection-buy-pending-v1',hash);setBuyTx(hash)
  await finishBuy(hash)
 }
 const s=snapshot?.scenarios[0]
 const outputToken=payToken==='MON'?'USDC':'MON'
 const chosen=mode==='v4'&&result?s?.quotes.find(q=>q.pool===result.values[0]):null
 const ranked=s?[...s.quotes].sort((a,b)=>BigInt(a.amountOutRaw)===BigInt(b.amountOutRaw)?(BigInt(a.pool)<BigInt(b.pool)?-1:1):BigInt(a.amountOutRaw)>BigInt(b.amountOutRaw)?-1:1):[]
 const invalidateQuote=()=>reset()
 return <section className="selection-page">
  <section className="selection-wallet" aria-label="钱包">
   <div className="selection-wallet-info"><span className="selection-wallet-title">钱包</span>{account&&<><a href={`https://testnet.monadscan.com/address/${account}`} target="_blank" rel="noreferrer" title={account}>{short(account)}</a><span className={chain===10143?'selection-wallet-network ready':'selection-wallet-network'}>{networkLabel}</span></>}</div>
   <div className="selection-wallet-actions">{wallet.wallets.length>1&&<select aria-label="选择钱包" disabled={busy} value={wallet.walletId} onChange={e=>wallet.setWalletId(e.target.value)}>{wallet.wallets.map(w=><option key={w.id} value={w.id}>{w.name}</option>)}</select>}{account?<><span className="selection-wallet-balance">{chain===143?'MON':'测试 MON'}<strong>{balance===null?wallet.balanceError||'…':Number(formatUnits(balance,18)).toLocaleString('en-US',{maximumFractionDigits:6})}</strong></span>{chain!==10143&&<button disabled={busy} className="primary-button" onClick={()=>run(wallet.switchTestnet)}>切换到测试网</button>}<button disabled={busy} className="secondary-button" onClick={()=>run(wallet.refreshBalance)}>刷新余额</button><button disabled={busy} className="secondary-button" onClick={wallet.disconnect}>断开连接</button></>:<button disabled={busy} className="primary-button" onClick={()=>run(wallet.connect)}>连接钱包</button>}</div>
  </section>
  <div className="selection-page-top"><h2>选池策略</h2><select aria-label="选择策略" value={mode} disabled={busy} onChange={e=>{setMode(e.target.value);reset()}}><option value="v4">单池最高输出</option><option value="dcar">V2 拆单（示例）</option></select></div>
  <div className={`selection-grid ${mode==='v4'?'selection-swap-grid':''}`}>
   <section className={`selection-card ${mode==='v4'?'selection-swap-card':''}`}>
    {mode==='v4'?<>
     <div className="selection-swap-heading"><h2>最优报价</h2><span className="selection-chain">Monad</span></div>
     <div className="selection-token-box"><div className="selection-token-caption"><label htmlFor="selection-pay">支付</label></div><div className="selection-token-line"><input id="selection-pay" aria-label="支付数量" disabled={busy} inputMode="decimal" value={swapAmount} onChange={e=>{setSwapAmount(e.target.value);invalidateQuote()}}/><span className="selection-token"><i className={payToken==='MON'?'token-mon':'token-usdc'}>{payToken==='MON'?'M':'$'}</i>{payToken}</span></div></div>
     <div className="selection-direction"><button aria-label="切换兑换方向" disabled={busy} onClick={()=>{setPayToken(outputToken);invalidateQuote()}}>↓↑</button></div>
     <div className={`selection-token-box selection-token-output ${live.stale?'selection-quote-old':''}`}><div className="selection-token-caption"><span>预计收到</span>{live.stale&&<span className="quote-stale">已过期</span>}</div><div className="selection-token-line"><output aria-label="预计收到数量">{result?formatUnits(result.values[1],s.outputDecimals):'—'}</output><span className="selection-token"><i className={outputToken==='MON'?'token-mon':'token-usdc'}>{outputToken==='MON'?'M':'$'}</i>{outputToken}</span></div></div>
     <div className="selection-live-bar"><span className={live.stale?'quote-stale':'quote-live'}>{!autoRefresh?'已暂停':busy?'核对中':live.stale?'已过期':live.loading?'更新中':'实时 · 1s'}</span><span>{live.ageMs===null?'—':`${(live.ageMs/1000).toFixed(1)}s`} · <span data-testid="selection-source-block">{snapshot?`#${snapshot.sourceBlock}`:'—'}</span></span></div>
     <div className="selection-live-actions"><button disabled={busy} className="primary-button selection-swap-submit" onClick={()=>{setAutoRefresh(true);live.refresh()}}>刷新报价</button><button disabled={busy} className="secondary-button selection-auto-toggle" onClick={()=>setAutoRefresh(v=>!v)}>{autoRefresh?'暂停':'继续'}</button></div>
     <button className="primary-button selection-buy-button" disabled={busy||!result||live.stale||payToken!=='MON'||!!localStorage.getItem('selection-buy-pending-v1')} onClick={()=>run(buyBest)}>Buy USDC · 使用最优报价</button>
     <p className="selection-swap-footnote">主网兑换 · 滑点上限 0.5% · gas 另付{payToken!=='MON'?' · 反向目前仅报价':''}</p>
     {buyTx&&<p className="selection-note"><a href={`https://monadscan.com/tx/${buyTx}`} target="_blank" rel="noreferrer">查看兑换交易</a>{localStorage.getItem('selection-buy-pending-v1')&&<button disabled={busy} onClick={()=>run(()=>finishBuy(buyTx))}>查询兑换结果</button>}</p>}
     {buyStatus&&<p role="status">{buyStatus}</p>}
     <details className="selection-details"><summary>合约调用 / 租金</summary><p>测试网按次调用 {formatUnits(BigInt(paid.reference.serviceFeeWei),18)} MON，成功后转给创建者，与主网兑换分别执行。</p><button className="secondary-button" disabled={busy||!paidInfo||!result||live.stale||!!localStorage.getItem('selection-paid-pending-v1')} onClick={()=>run(payForSelection)}>支付租金并调用</button><p><a href={paid.reference.explorer} target="_blank" rel="noreferrer">收费合约</a></p>{paidTx&&<a href={`https://testnet.monadscan.com/tx/${paidTx}`} target="_blank" rel="noreferrer">调用交易</a>}{localStorage.getItem('selection-paid-pending-v1')&&<button disabled={busy} onClick={()=>run(()=>confirmPayment(paidTx))}>查询调用结果</button>}{paidStatus&&<p role="status">{paidStatus}</p>}{paidError&&<p className="selection-error">{paidError}</p>}</details>
    </>:<>
     <h2>V2 拆单 <span className="selection-badge">示例</span></h2>
     <div className="selection-fields"><label>输入数量 A<input disabled={busy} inputMode="decimal" value={amount} onChange={e=>{setAmount(e.target.value);reset()}}/></label><label>最多拆单数<select disabled={busy} value={splits} onChange={e=>{setSplits(Number(e.target.value));reset()}}>{[1,2,3].map(n=><option key={n}>{n}</option>)}</select></label></div>
     <button disabled={busy} className="primary-button" onClick={()=>run(()=>{setResult(local());setValidation(null);setStatus('计算完成')})}>本地试跑</button>
     {result&&<div className="selection-rows">{result.values[0].map((x,i)=><div key={i}><span>{data.sample.pools[i].name}</span><strong>{formatUnits(x,6)} A</strong></div>)}<p>输出 {formatUnits(result.values[1],6)} B</p></div>}
    </>}
   </section>
   <section className="selection-card selection-route-card">
    {mode==='v4'&&<>
     <div className="selection-pool-heading"><h2>MON/USDC 池子</h2><span title="可报价 / 已接入">{snapshot?.available??'—'}/{data.routes.candidates.length}</span></div>
     {chosen?<>
      <div className={`selection-rows selection-pool-list ${live.stale?'selection-quote-old':''}`} aria-label="池子报价">{ranked.map(q=><div className={q.pool===chosen.pool?'selection-best-row':''} key={q.pool}><span><code title={q.address||q.pool}>{short(q.address||q.pool)}</code><small>{(q.protocol||'Uniswap V4').replace('Uniswap','Uni')} · {(q.fee??q.poolKey.fee)/10000}%</small></span><span>{q.status==='unavailable'?'失败':q.status==='zero'?'过小':q.amountOut}{q.pool===chosen.pool?' ✓':''}</span></div>)}</div>
      {result.tieCount>1&&<p className="selection-note">{result.tieCount} 池并列，按 ID 选择。</p>}
     </>:<p className="selection-empty">{live.error?'暂无报价':'报价中…'}</p>}
    </>}
    <button disabled={busy||!address||(mode==='v4'&&!result)} className="secondary-button selection-verify-button" onClick={()=>run(verify)}>链上核对</button>
    <details className="selection-details"><summary>策略与合约</summary>
     {mode==='v4'?<><p className="selection-note">本地库 {data.routes.totalMonadPools} 池 · 同币对 {data.routes.samePairPools} 池 · 已接入 {data.routes.candidates.length} 池（含 WMON）。</p><p className="selection-note">按兑换输出择优，池手续费已计入。未比较其他币对、多跳与 gas。报价来自主网，核对在测试网执行。</p>{data.routes.excluded.map(q=><p className="selection-note" key={q.pool}><code title={q.pool}>{short(q.pool)}</code> · PoolKey 待核验</p>)}</>:<p className="selection-note">合成 V2 储备，最多 6 池、3 路，整数近似分配。不是实时池数据。</p>}
     <div className="selection-address">{address&&<a href={`https://testnet.monadscan.com/address/${address}`} target="_blank" rel="noreferrer">{address}</a>}</div>
     <button disabled={busy} className="secondary-button" onClick={()=>run(deploy)}>部署到测试网</button>
     <p className="selection-note">部署需测试 MON。当前合约不认证报价来源。</p>
    </details>
    {tx&&<p><a href={`https://testnet.monadscan.com/tx/${tx}`} target="_blank" rel="noreferrer">部署交易</a>{localStorage.getItem('selection-pending-v1')&&<button disabled={busy} onClick={()=>run(()=>finishDeployment(tx,localStorage.getItem('selection-pending-kind-v1')))}>恢复确认</button>}</p>}
   </section>
  </div>
  {mode==='v4'&&live.error&&<div className="selection-error" role="alert">{live.error}{snapshot?' · 上次报价已过期':''}</div>}
  {mode==='v4'&&snapshot&&snapshot.available<snapshot.candidateCount&&<p className="selection-coverage-warning">{snapshot.candidateCount-snapshot.available} 池无有效报价，已排除。</p>}
  {error&&<div className="selection-error" role="alert">{error}</div>}
  {(busy||status)&&<p role="status" className={verified?'selection-success':''}>{busy?'处理中…':validation?`核对通过${validation.block?` · #${validation.block}${!verified?'（历史快照）':''}`:''}`:status}</p>}
  {result&&<details className="selection-details selection-card selection-result"><summary>计算记录</summary><h2>决策结果 {verified&&<span className="selection-badge">链上核对通过</span>}</h2><p><code>{result.values.at(-1)}</code></p><button className="secondary-button" onClick={()=>{const url=URL.createObjectURL(new Blob([json({algorithm:mode,chainId:10143,address,verified,validation,fresh:mode==='v4'?!live.stale:undefined,...result})],{type:'application/json'}));const a=document.createElement('a');a.href=url;a.download='selection-replay.json';a.click();URL.revokeObjectURL(url)}}>下载结果</button></details>}
 </section>
}
