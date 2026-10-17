import {useCallback,useEffect,useRef,useState} from 'react'
import {createPublicClient,createWalletClient,custom,http,formatUnits,parseAbi} from 'viem'
import {monadTestnet} from 'viem/chains'
import training from '../generated/monad-ai.json'
import './aiShowcase.css'

const api=createPublicClient({chain:monadTestnet,transport:http('/api/selection/testnet-rpc',{timeout:25000,retryCount:1})})
const erc20=parseAbi([
  'function balanceOf(address) view returns(uint256)',
  'function allowance(address,address) view returns(uint256)',
  'function approve(address,uint256) returns(bool)'
])
const sessionAbi=parseAbi([
  'function sessions(address) view returns(uint64 id,uint64 startedAt,uint64 lastExecution,uint8 nextRound,bool active)',
  'function start()', 'function cancel()',
  'event SessionStarted(address indexed wallet,uint64 indexed sessionId,uint64 startedAt,uint64 expiresAt)',
  'event Decision(address indexed wallet,uint64 indexed sessionId,uint8 indexed round,uint8 action,bytes32 inputHash,uint256 amountIn,uint256 amountOut,int256[3] scores)'
])
const frameIds=[0,2,8,7,3,8]
const frames=frameIds.map(i=>training.vectors[i])
const labels=['HOLD','BUY','SELL']
const meaning=['无兑换 · 记录本轮链上决策','0.4 USDC → WMON','0.1 WMON → USDC']
const trim=s=>s?s.slice(0,8)+'…'+s.slice(-5):'—'
const fmt=(n,d=4)=>Number(n).toLocaleString('en-US',{maximumFractionDigits:d})
const errorText=e=>e?.shortMessage||e?.message?.split('\n')[0]||'请求失败'
const approveUSDC=800000n
const approveWMON=200000000000000000n
function provider(){
  if(!window.ethereum?.request)throw Error('请用 MetaMask 打开网页')
  return window.ethereum
}
async function ensureNetwork(p){
  if(parseInt(await p.request({method:'eth_chainId'}),16)===10143)return
  try{await p.request({method:'wallet_switchEthereumChain',params:[{chainId:'0x279f'}]})}
  catch(e){
    if(e.code!==4902)throw e
    await p.request({method:'wallet_addEthereumChain',params:[{
      chainId:'0x279f',chainName:'Monad Testnet',
      nativeCurrency:{name:'MON',symbol:'MON',decimals:18},
      rpcUrls:['https://testnet-rpc.monad.xyz'],blockExplorerUrls:['https://testnet.monadscan.com']
    }]})
  }
  if(parseInt(await p.request({method:'eth_chainId'}),16)!==10143)throw Error('请连接 Monad Testnet')
}
function Track(){return <div className="ows-market-track"><span>YOUR WALLET</span><i/><span>SESSION START</span><i/><span>ONCHAIN AI</span><i/><span>AUTO DEX</span></div>}
async function activity(config,address,id){
  const latest=await api.getBlockNumber()
  const from=latest>210n?latest-210n:0n
  const ranges=[]
  for(let n=from;n<=latest;n+=70n)ranges.push([n,n+69n>latest?latest:n+69n])
  const batches=await Promise.all(ranges.map(async([fromBlock,toBlock])=>
    api.getContractEvents({address:config.contract,abi:sessionAbi,eventName:'Decision',
      args:{wallet:address},fromBlock,toBlock})))
  return batches.flat().filter(log=>Number(log.args.sessionId)===id).map(log=>({
    round:Number(log.args.round),action:Number(log.args.action),hash:log.transactionHash,
    block:String(log.blockNumber),amountIn:String(log.args.amountIn),amountOut:String(log.args.amountOut),
    at:Number(log.blockNumber)
  })).sort((a,b)=>a.round-b.round)
}
export default function AiShowcase(){
  const [config,setConfig]=useState(null)
  const [address,setAddress]=useState('')
  const [session,setSession]=useState({id:0,startedAt:0,lastExecution:0,nextRound:0,active:false})
  const [balances,setBalances]=useState({usdc:'0',wmon:'0',mon:'0'})
  const [allowances,setAllowances]=useState({usdc:0n,wmon:0n})
  const [history,setHistory]=useState([])
  const [busy,setBusy]=useState('')
  const [error,setError]=useState('')
  const [now,setNow]=useState(Date.now())
  const [keeper,setKeeper]=useState(null)
  const refreshing=useRef(false)
  const disconnected=useRef(false)

  useEffect(()=>{
    let active=true
    fetch('/ai/session-config.json',{cache:'no-store'}).then(r=>{if(!r.ok)throw Error('会话合约未配置');return r.json()})
      .then(d=>{if(d.chainId!==10143)throw Error('测试网配置有误');if(active)setConfig(d)})
      .catch(e=>{if(active)setError(errorText(e))})
    const timer=setInterval(()=>setNow(Date.now()),1000)
    return()=>{active=false;clearInterval(timer)}
  },[])
  useEffect(()=>{
    if(!config||!window.ethereum?.request)return
    let live=true
    const p=window.ethereum
    p.request({method:'eth_accounts'}).then(a=>{if(live&&a[0])setAddress(a[0])}).catch(()=>{})
    const changed=a=>{setAddress(a?.[0]||'');setHistory([]);setError('')}
    const chainChanged=()=>{setError('钱包网络已更改，请检查 Monad Testnet')}
    p.on?.('accountsChanged',changed);p.on?.('chainChanged',chainChanged)
    return()=>{live=false;p.removeListener?.('accountsChanged',changed);p.removeListener?.('chainChanged',chainChanged)}
  },[config])

  const refresh=useCallback(async()=>{
    if(!config||!address||refreshing.current)return
    refreshing.current=true
    try{
      const [id,mon,usdc,wmon,usdcAllowance,wmonAllowance]=await Promise.all([
        api.readContract({address:config.contract,abi:sessionAbi,functionName:'sessions',args:[address]}),
        api.getBalance({address}),
        api.readContract({address:config.usdc,abi:erc20,functionName:'balanceOf',args:[address]}),
        api.readContract({address:config.wmon,abi:erc20,functionName:'balanceOf',args:[address]}),
        api.readContract({address:config.usdc,abi:erc20,functionName:'allowance',args:[address,config.contract]}),
        api.readContract({address:config.wmon,abi:erc20,functionName:'allowance',args:[address,config.contract]})
      ])
      const [sid,startedAt,lastExecution,nextRound,isActive]=id
      const next={id:Number(sid),startedAt:Number(startedAt),lastExecution:Number(lastExecution),
        nextRound:Number(nextRound),active:isActive}
      setSession(next)
      setBalances({mon:formatUnits(mon,18),usdc:formatUnits(usdc,6),wmon:formatUnits(wmon,18)})
      setAllowances({usdc:usdcAllowance,wmon:wmonAllowance})
      if(next.id>0){
        try{
          const events=await activity(config,address,next.id)
          setHistory(events)
        }catch{/* current on-chain round is still shown if log RPC is limited */}
      }else setHistory([])
    }finally{refreshing.current=false}
  },[address,config])

  useEffect(()=>{
    if(!config||!address)return
    disconnected.current=false
    refresh().catch(e=>{if(!disconnected.current)setError(errorText(e))})
    const timer=setInterval(()=>refresh().catch(()=>{}),4500)
    return()=>{disconnected.current=true;clearInterval(timer)}
  },[config,address,refresh])

  useEffect(()=>{
    let live=true
    async function check(){
      try{
        const r=await fetch('/ai/keeper-status.json?t='+Date.now(),{cache:'no-store'})
        if(!r.ok)throw Error('offline')
        const result=await r.json()
        if(live)setKeeper(Date.now()-new Date(result.checkedAt).getTime()<15000&&result.chainId===10143)
      }catch{if(live)setKeeper(false)}
    }
    check();const timer=setInterval(check,4000)
    return()=>{live=false;clearInterval(timer)}
  },[])

  async function wallet(){
    const p=provider();await ensureNetwork(p)
    const account=(await p.request({method:'eth_accounts'}))[0]
    if(account?.toLowerCase()!==address.toLowerCase())throw Error('钱包已切换，请重新连接')
    return createWalletClient({account:address,chain:monadTestnet,transport:custom(p)})
  }
  async function connect(){
    setBusy('连接钱包');setError('')
    try{
      const p=provider();await ensureNetwork(p)
      const accounts=await p.request({method:'eth_requestAccounts'})
      if(!accounts[0])throw Error('未选择钱包')
      setAddress(accounts[0])
    }catch(e){setError(errorText(e))}
    finally{setBusy('')}
  }
  async function grant(){
    setBusy('授权中');setError('')
    try{
      const w=await wallet()
      const steps=[
        [config.usdc,approveUSDC,allowances.usdc,'USDC'],
        [config.wmon,approveWMON,allowances.wmon,'WMON']
      ]
      for(const [token,amount,existing,name] of steps){
        if(existing>=amount)continue
        setBusy('批准 '+name)
        const hash=await w.writeContract({address:token,abi:erc20,functionName:'approve',
          args:[config.contract,amount]})
        const receipt=await api.waitForTransactionReceipt({hash,timeout:150000})
        if(receipt.status!=='success')throw Error(name+' 授权回滚')
      }
      await refresh()
    }catch(e){setError(errorText(e))}
    finally{setBusy('')}
  }
  async function sendAction(method){
    setBusy(method==='start'?'等待 Start 签名':'取消会话');setError('')
    try{
      const w=await wallet()
      const hash=await w.writeContract({address:config.contract,abi:sessionAbi,functionName:method})
      setBusy('等待交易确认')
      const receipt=await api.waitForTransactionReceipt({hash,timeout:150000})
      if(receipt.status!=='success')throw Error('交易被回滚')
      await refresh()
    }catch(e){setError(errorText(e))}
    finally{setBusy('')}
  }

  const approved=allowances.usdc>=approveUSDC&&allowances.wmon>=approveWMON
  const time=Math.floor(now/1000)
  const remaining=session.active?Math.max(0,session.startedAt+110-time):0
  const step=session.nextRound
  const currentAction=step<6?frames[step].action:null
  const finished=step===6
  const expired=session.active&&remaining===0
  const canStart=address&&config&&approved&&!busy&&(!session.active||expired||finished)
  const done=history.filter(h=>h.round<6)
  const latest=done.at(-1)
  const hasWmon=Number(balances.wmon)>=.1
  const viewAction=latest?.action??currentAction
  return <section className="ows-page">
    <div className="ows-mast">
      <div className="ows-overline"><span className="ows-live-dot"/> MONAD TESTNET <span className="ows-dim">/</span> ONE-CLICK AGENT</div>
      <div className="ows-mast-row">
        <div><h1>ONE START.<em>SIX TRADES.</em></h1><p>连接钱包 · 签署一次 Start · Keeper 每 10 秒自动执行链上 AI 决策。</p></div>
        <div className="ows-wallet"><span>{address?'CONNECTED WALLET':'YOUR WALLET'}</span><strong>{address?trim(address):'NOT CONNECTED'}</strong>
          <button onClick={connect} disabled={Boolean(busy)||!config}>{address?'切换 / 检查钱包':'连接 MetaMask'} <span>↗</span></button></div>
      </div>
      <Track/>
    </div>
    <div className="ows-main">
      <div className="ows-decision-stage">
        <div className="ows-top-line"><span>12 → 8 → 3 QUANTIZED MODEL · ONCHAIN</span><span>{config?'● CONTRACT DEPLOYED':'● LOADING'}</span></div>
        <div className={'ows-action ows-action-'+(viewAction??'idle')}>
          <div className="ows-action-kicker">{finished?'SESSION COMPLETE':session.active?'AGENT RUNNING':'NEXT MODEL SIGNAL'}</div>
          <strong>{finished?'DONE':address?labels[viewAction]||'READY':'READY'}</strong>
          <p>{finished?'6 / 6 transactions completed':viewAction!==null?meaning[viewAction]:'连接钱包查看模型决策'}</p>
        </div>
        <div className="ows-process">
          <div><span>已执行</span><strong>{step}<small> / 6</small></strong></div>
          <div><span>会话剩余时间</span><strong>{session.active?remaining:'110'}<small> sec</small></strong></div>
          <div><span>自动交易</span><strong>{session.active?'RUNNING':finished?'DONE':'READY'}</strong></div>
        </div>
        <div className="ows-progress">{frames.map((f,i)=>{
          const actual=done.find(x=>x.round===i)
          return <div key={i} className={'ows-progress-step '+(i<step?'completed':i===step?'now':'future')}>
            <span>{String(i+1).padStart(2,'0')}</span><strong>{labels[actual?.action??f.action]}</strong><small>{i<step?'✓':i===step?'NEXT':'·'}</small>
          </div>
        })}</div>
        <div className="ows-callout">六组历史真实行情特征回放；模型在 Monad 测试网合约中决定动作。每笔真实兑换按测试网当前池价成交。</div>
      </div>
      <aside className="ows-control-stage">
        <div className="ows-section-label">01 / ASSETS IN YOUR WALLET</div>
        <div className="ows-holdings">
          <div><span>USDC</span><strong>{address?fmt(balances.usdc,6):'—'}</strong></div>
          <div><span>WMON</span><strong>{address?fmt(balances.wmon,6):'—'}</strong></div>
          <div><span>Gas · MON</span><strong>{address?fmt(balances.mon,5):'—'}</strong></div>
        </div>
        <div className="ows-section-label">02 / AUTOMATED SESSION</div>
        <div className="ows-exec-details">
          <span>Keeper 状态 <b>{keeper===true?'ONLINE':keeper===false?'OFFLINE':'CHECKING'}</b></span>
          <span>单笔 BUY <b>0.4 USDC</b></span><span>单笔 SELL <b>0.1 WMON</b></span>
          <span>单次授权上限 <b>0.8 USDC + 0.2 WMON</b></span>
          <span>六轮间隔 <b>10 sec</b></span>
        </div>
        {!address?<button className="ows-primary" onClick={connect} disabled={Boolean(busy)||!config}>连接钱包 <span>↗</span></button>
        :session.active&&!expired?<button className="ows-primary" disabled>{busy||'KEEPER RUNNING…'} <span>●</span></button>
        :!approved?<button className="ows-primary" onClick={grant} disabled={Boolean(busy)}>{busy||'授权本次演示额度'} <span>↗</span></button>
        :canStart?<button className="ows-primary" onClick={()=>sendAction('start')} disabled={keeper!==true}>START 60s AUTO TRADING <span>→</span></button>
        :<button className="ows-primary" disabled>{busy|| (session.active&&!expired?'KEEPER RUNNING…':expired?'SESSION EXPIRED':finished?'FINISHED':'WAITING')} <span>●</span></button>}
        {session.active&&!expired&&<button className="ows-cancel" onClick={()=>sendAction('cancel')} disabled={Boolean(busy)}>取消会话 · Stop</button>}
        <p className="ows-subtext">{session.active&&!expired?'会话已签署，Keeper 负责完成后续六笔交易。':!approved?'首次需要 ERC-20 小额授权，之后只签一次 Start。':'资金始终在钱包，交易结果直接返回钱包。'}
          {' '}只有测试网代币；Keeper 支付后续六笔交易的 gas。</p>
        {address&&approved&&!session.active&&!hasWmon&&<p className="ows-subtext">起始钱包可没有 WMON，第一笔 BUY 得到的 WMON 用于后续 SELL。</p>}
        {keeper===false&&<p className="ows-error">Keeper 未在线。请在项目服务器运行 keeper，网页才能自动交易。</p>}
        {error&&<p className="ows-error" role="alert">{error}</p>}
        <div className="ows-links">{config&&<a href={config.explorer} target="_blank" rel="noreferrer">会话合约 ↗</a>}
          {config&&<a href={'https://testnet.monadscan.com/address/'+config.model} target="_blank" rel="noreferrer">AI 模型 ↗</a>}</div>
      </aside>
    </div>
    <div className="ows-ledger">
      <div className="ows-ledger-title"><div><span>LIVE ONCHAIN TRANSACTIONS</span><h2>Execution log</h2></div>
        <span>六笔由 Keeper 自动签名广播，成功后出现链上交易哈希</span></div>
      <div className="ows-table-wrap"><table><thead><tr><th>ROUND</th><th>AI SIGNAL</th><th>SETTLEMENT</th><th>STATUS</th><th>MONADSCAN</th></tr></thead><tbody>{frames.map((f,i)=>{
        const item=done.find(x=>x.round===i)
        const action=item?.action??f.action
        const flow=!item?'—':action===0?'NO SWAP':action===1?'0.4 USDC → '+fmt(formatUnits(BigInt(item.amountOut),18),5)+' WMON'
          :'0.1 WMON → '+fmt(formatUnits(BigInt(item.amountOut),6),5)+' USDC'
        return <tr key={i} className={item?'ows-done-row':''}>
          <td>{String(i+1).padStart(2,'0')}</td><td><span className={'ows-signal ows-signal-'+action}>{labels[action]}</span></td>
          <td>{flow}</td><td>{item?'CONFIRMED':i<step?'CONFIRMED':'WAITING'}</td>
          <td>{item?<a href={'https://testnet.monadscan.com/tx/'+item.hash} target="_blank" rel="noreferrer">{trim(item.hash)} ↗</a>:'—'}</td>
        </tr>
      })}</tbody></table></div>
    </div>
    <footer className="ows-footer"><span>SESSION CONTRACT {trim(config?.contract)}</span><span>MONAD TESTNET 10143 · NON-CUSTODIAL</span></footer>
  </section>
}
