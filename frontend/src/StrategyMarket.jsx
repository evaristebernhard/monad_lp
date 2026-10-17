import React,{useCallback,useEffect,useRef,useState} from 'react'
import {createPublicClient,createWalletClient,http,custom,formatUnits,parseEther,decodeEventLog,keccak256} from 'viem'
import {monadTestnet} from 'viem/chains'
import data from './generated/selection.json'
import './strategyMarket.css'
const rpc=createPublicClient({chain:monadTestnet,transport:http('/api/selection/testnet-rpc',{timeout:30000,retryCount:0})})
const short=value=>`${value.slice(0,8)}…${value.slice(-6)}`
const storageKey='strategy-market-pending-v1'
const money=value=>value===undefined?'—':formatUnits(value,18)
function pendingTransaction(){try{const r=JSON.parse(localStorage.getItem(storageKey));return r?.hash&&r?.functionName?r:null}catch{return null}}
const plans={buy:{label:'Buy',name:'永久使用',description:'当前钱包永久使用权',detail:'一次购买，永久调用授权入口。每次链上调用仍需 gas。',event:'LicensePurchased'},rent:{label:'Rent',name:'限期租用',description:'30 天使用权',detail:'按期支付，到期手动续租；提前续租会顺延，不自动扣款。',event:'LicenseRented'},buyOwnership:{label:'Own',name:'所有权',description:'管理权与后续授权收入',detail:'转让此市场合约的管理权与未来授权收入。已有授权继续有效，卖家保留永久使用权；旧版按次体验独立结算。',event:'OwnershipPurchased'}}
export default function StrategyMarket({wallet,busy,run,decision,stale,onTrial,trialPending,onConfirmTrial,trialTx,trialStatus,trialReady}){
 const [selected,setSelected]=useState('mon'),[plan,setPlan]=useState('buy'),[info,setInfo]=useState(null),[readError,setReadError]=useState(''),[message,setMessage]=useState(''),[pending,setPending]=useState(pendingTransaction),[lastTx,setLastTx]=useState(''),[listing,setListing]=useState('10'),[actionError,setActionError]=useState('')
 const market=data.market,reference=market.reference,address=reference.contractAddress,account=wallet.account
 const activeAccount=useRef(account);activeAccount.current=account
 const refresh=useCallback(async(queryAccount=account)=>{
  if(await rpc.getChainId()!==10143)throw Error('请连接 Monad 测试网')
  const block=await rpc.getBlockNumber()
  const read=(functionName,args=[])=>rpc.readContract({address,abi:market.abi,functionName,args,blockNumber:block})
  const [owner,buyPrice,rentPrice,rentDuration,ownershipPrice,code,...license]=await Promise.all([read('owner'),read('buyPrice'),read('rentPrice'),read('rentDuration'),read('ownershipPrice'),rpc.getBytecode({address,blockNumber:block}),...(queryAccount?[read('permanent',[queryAccount]),read('rentedUntil',[queryAccount]),read('hasAccess',[queryAccount])]:[])])
  if(!code||keccak256(code)!==reference.runtimeCodeHash)throw Error('市场合约代码不匹配')
  return {owner,buyPrice,rentPrice,rentDuration,ownershipPrice,permanent:license[0]||false,rentedUntil:license[1]||0n,hasAccess:license[2]||false,account:queryAccount}
 },[account,address,market.abi,reference.runtimeCodeHash])
 useEffect(()=>{
  let active=true,loading=false
  setInfo(null);setReadError('')
  async function poll(){if(loading)return;loading=true;try{const value=await refresh();if(active){setInfo(value);setReadError('')}}catch(e){if(active)setReadError(e.shortMessage||e.message)}finally{loading=false}}
  poll();const timer=setInterval(poll,12000)
  return()=>{active=false;clearInterval(timer)}
 },[refresh])
 const isOwner=!!info&&account?.toLowerCase()===info.owner.toLowerCase(),entitled=isOwner||info?.permanent
 const chosen=plans[plan],price=info?.[plan==='buy'?'buyPrice':plan==='rent'?'rentPrice':'ownershipPrice']
 async function confirm(transaction){
  setMessage('等待链上确认…')
  const receipt=await rpc.waitForTransactionReceipt({hash:transaction.hash,timeout:90000})
  if(receipt.status!=='success'){localStorage.removeItem(storageKey);setPending(null);setMessage('交易失败，MON 已回滚，gas 已消耗');return}
  const name=plans[transaction.functionName]?.event||(transaction.functionName==='select'?'StrategyUsed':'OwnershipListed')
  const event=receipt.logs.filter(log=>log.address.toLowerCase()===address.toLowerCase()).flatMap(log=>{try{return [decodeEventLog({abi:market.abi,data:log.data,topics:log.topics})]}catch{return []}}).find(e=>e.eventName===name)
  const actor=event?.args.buyer||event?.args.newOwner||event?.args.caller
  if(!event||(actor&&actor.toLowerCase()!==transaction.account.toLowerCase()))throw Error('未找到匹配的合约事件，请查看交易详情')
  localStorage.removeItem(storageKey);setPending(null);setLastTx(transaction.hash)
  setMessage(transaction.functionName==='buy'?'已获得永久使用权':transaction.functionName==='rent'?'租用成功，授权期限已更新':transaction.functionName==='buyOwnership'?'所有权已转让，后续收入归当前钱包':transaction.functionName==='select'?'策略调用成功':'所有权挂牌已更新')
  const updated=await refresh(activeAccount.current)
  if(updated.account===activeAccount.current)setInfo(updated)
  wallet.refreshBalance()
 }
 async function transact(functionName){
  if(pendingTransaction())throw Error('已有待确认交易，请先查询结果')
  if(!info||readError)throw Error('请等待链上信息恢复')
  const provider=wallet.provider,who=await wallet.connect()
  await wallet.switchTestnet()
  const accounts=await provider.request({method:'eth_accounts'})
  if(accounts[0]?.toLowerCase()!==who.toLowerCase()||Number(await provider.request({method:'eth_chainId'}))!==10143)throw Error('钱包账户或网络已改变，请重试')
  const args=functionName==='select'?decision?.args:functionName==='listOwnership'?[parseEther(listing)]:[]
  if(functionName==='select'&&(!decision||stale))throw Error('请先获取有效报价')
  const value=functionName==='buy'?info.buyPrice:functionName==='rent'?info.rentPrice:functionName==='buyOwnership'?info.ownershipPrice:0n
  const request={account:who,address,abi:market.abi,functionName,args,value}
  await rpc.simulateContract(request)
  const [gas,gasPrice]=await Promise.all([rpc.estimateContractGas(request),rpc.getGasPrice()])
  if(gas>1000000n||gasPrice>160000000000n)throw Error('测试网 gas 费用过高，请稍后重试')
  const signer=createWalletClient({account:who,chain:monadTestnet,transport:custom(provider)})
  const hash=await signer.writeContract({...request,gas:gas*120n/100n,maxFeePerGas:gasPrice*120n/100n,maxPriorityFeePerGas:0n})
  const record={hash,functionName,account:who};localStorage.setItem(storageKey,JSON.stringify(record));setPending(record);setLastTx(hash)
  await confirm(record)
 }
 function execute(action){run(async()=>{setActionError('');try{await action()}catch(e){setActionError(e.shortMessage||e.message||'操作失败')}})}
 const unavailable=selected!=='mon',notForSale=plan==='buyOwnership'&&price===0n
 const buttonText=unavailable?'暂未上架':!info?'读取合约中…':plan==='buyOwnership'?(isOwner?'你已拥有此合约':notForSale?'暂未挂牌':'Own · 购买所有权'):entitled?'已获得永久权限':plan==='rent'&&info?.hasAccess?'Rent · 续租':'Buy'===chosen.label?'Buy · 永久使用':'Rent · 租用 30 天'
 const accessLabel=isOwner?'合约所有者':info?.permanent?'永久使用权':info?.hasAccess?`租用至 ${new Date(Number(info.rentedUntil)*1000).toLocaleDateString()}`:account?'尚未持有授权':'连接钱包查看你的授权'
 return <section className="contract-market" aria-label="合约 Market">
  <header className="market-heading"><div><h1>Contract Market<span>.</span></h1><p>选择策略，按你的方式使用。</p></div><span className="market-network">Monad 测试网</span></header>
  <div className="market-layout">
   <div className="market-catalog" aria-label="选择合约">
    <button className={`market-product ${selected==='mon'?'selected':''}`} aria-pressed={selected==='mon'} onClick={()=>setSelected('mon')}><span className="market-token-pair"><i>◇</i><i>$</i></span><span><strong>MON / USDC</strong><small>单池择优</small></span><span className="market-arrow" aria-hidden="true">↗</span></button>
    <button className={`market-product ${selected==='dcar'?'selected':''}`} aria-pressed={selected==='dcar'} onClick={()=>setSelected('dcar')}><span className="market-dcar-symbol">⌘</span><span><strong>DCAR V2</strong><small>示例 · 暂未上架</small></span><span className="market-arrow" aria-hidden="true">↗</span></button>
    <div className="market-catalog-footer"><span>你的授权</span><strong>{selected==='mon'?accessLabel:'此合约尚未上架'}</strong></div>
   </div>
   <div className="market-main">
    <article className="market-offer" aria-label="合约购买">
     <div className="market-route-art" aria-hidden="true"><span className="market-orbit orbit-one"/><span className="market-orbit orbit-two"/><span className="market-node node-one">◇</span><span className="market-node node-two">$</span><span className="market-node node-three"/></div>
     <div className="market-offer-content"><div className="market-token-pair"><i>{unavailable?'⌘':'◇'}</i>{!unavailable&&<i>$</i>}</div><h2>{unavailable?'DCAR V2':'MON / USDC'}</h2><p className="market-product-description">{unavailable?'多池拆单 · 合成储备演示':'单池择优 · 比较 14 个直接池'}</p>
      <div className="market-plans" role="group" aria-label="授权方式">{Object.entries(plans).map(([key,p])=><button key={key} aria-pressed={plan===key} className={plan===key?'active':''} onClick={()=>{setPlan(key);setMessage('');setActionError('')}} disabled={busy}>{p.label}<small>{p.name}</small></button>)}</div>
      <div className="market-price"><strong>{unavailable?'—':money(price)}</strong><span>MON{plan==='rent'?' / 30 天':''}</span></div>
      <p className="market-license-title">{chosen.description}</p><p className="market-license-detail">{chosen.detail}</p>
      <button className="market-buy" disabled={busy||unavailable||!info||!!readError||!!pending||notForSale||(plan==='buyOwnership'?isOwner:entitled)} onClick={()=>execute(()=>transact(plan))}>{busy?'处理中…':buttonText}<span aria-hidden="true">↗</span></button>
      <p className="market-purchase-note">测试 MON 定价 · gas 另付{plan==='rent'?' · 不自动续费':''}</p>
     </div>
    </article>
    {actionError&&<p className="market-error" role="alert">{actionError}</p>}
    {readError&&<p className="market-error" role="alert">链上读取失败：{readError}</p>}
    <details className="market-details"><summary>合约详情<span>＋</span></summary>{unavailable?<p className="market-license-detail">DCAR V2 为合成储备示例，尚未创建授权市场。</p>:<dl><div><dt>市场合约</dt><dd><a href={reference.explorer} target="_blank" rel="noreferrer">{address}</a></dd></div><div><dt>所有者 / 收款方</dt><dd>{info?<a href={`https://testnet.monadscan.com/address/${info.owner}`} target="_blank" rel="noreferrer">{info.owner}</a>:'读取中…'}</dd></div><div><dt>授权范围</dt><dd>授权当前钱包调用此合约入口；公开报价仍免费。当前不认证外部报价，不执行兑换。旧版按次体验使用独立合约，费用归原创建者，不随市场所有权转让。</dd></div></dl>}
     {isOwner&&!unavailable&&<div className="market-owner"><label htmlFor="ownership-listing">所有权挂牌价（MON，0 为下架）</label><input id="ownership-listing" value={listing} onChange={e=>setListing(e.target.value)} inputMode="decimal" disabled={busy}/><button disabled={busy||!!pending||!!readError} onClick={()=>execute(()=>transact('listOwnership'))}>更新挂牌</button></div>}
    </details>
    {pending&&<div className="market-transaction"><a href={`https://testnet.monadscan.com/tx/${pending.hash}`} target="_blank" rel="noreferrer">待确认交易 {short(pending.hash)}</a><button disabled={busy} onClick={()=>execute(()=>confirm(pending))}>查询结果</button></div>}
    {message&&<p className="market-message" role="status">{message}</p>}
    {lastTx&&!pending&&<a className="market-last-tx" href={`https://testnet.monadscan.com/tx/${lastTx}`} target="_blank" rel="noreferrer">查看交易 ↗</a>}
    {!unavailable&&<div className="market-trial"><span className="market-trial-symbol">ϟ</span><div><strong>{info?.hasAccess?'使用策略':'旧版按次体验'}</strong><small>{info?.hasAccess?'已授权 · 仅需 gas':'0.01 MON / 次 · gas 另付'}</small></div><button disabled={busy||!decision||stale||!!pending||(!info?.hasAccess&&(!trialReady||trialPending))} onClick={()=>execute(info?.hasAccess?()=>transact('select'):onTrial)}>{info?.hasAccess?'调用策略':'调用'}</button></div>}
    {trialPending&&<button className="market-recover" disabled={busy} onClick={()=>execute(onConfirmTrial)}>查询按次调用结果</button>}
    {trialTx&&<a className="market-last-tx" href={`https://testnet.monadscan.com/tx/${trialTx}`} target="_blank" rel="noreferrer">按次调用交易 ↗</a>}
    {trialStatus&&<p role="status" className="market-message">{trialStatus}</p>}
   </div>
  </div>
 </section>
}
