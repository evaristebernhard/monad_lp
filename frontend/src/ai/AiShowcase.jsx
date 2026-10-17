import {useCallback,useEffect,useRef,useState} from 'react'
import {createPublicClient,createWalletClient,custom,http,formatUnits,parseUnits,parseAbi} from 'viem'
import {monadTestnet} from 'viem/chains'
import {sessionAbi,createDecisionReader} from './chainData'
import './aiShowcase.css'

const api=createPublicClient({chain:monadTestnet,transport:http('/api/selection/testnet-rpc',{timeout:25000,retryCount:1})})
const erc20=parseAbi([
  'function balanceOf(address) view returns(uint256)',
  'function allowance(address,address) view returns(uint256)',
  'function approve(address,uint256) returns(bool)',
  'function deposit() payable'
])
const rounds=Array.from({length:6},(_,i)=>i)
const activity=createDecisionReader(api)
const labels=['HOLD','BUY','SELL']
const meaning=['等待下一次交易','1 USDC → MON','1 MON → USDC']
const trim=s=>s?s.slice(0,8)+'…'+s.slice(-5):'—'
const fmt=(n,d=4)=>Number(n).toLocaleString('en-US',{maximumFractionDigits:d})
const errorText=e=>e?.shortMessage||e?.message?.split('\n')[0]||'请求失败'
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
function Track(){return <div className="ows-market-track"><span>START</span><i/><span>AI SIGNAL</span><i/><span>BUY / SELL</span><i/><span>YOUR WALLET</span></div>}
export default function AiShowcase(){
  const [config,setConfig]=useState(null)
  const [address,setAddress]=useState('')
  const [session,setSession]=useState({id:0,startedAt:0,lastExecution:0,nextRound:0,active:false})
  const [balances,setBalances]=useState(null)
  const [allowances,setAllowances]=useState({usdc:0n,wmon:0n})
  const [history,setHistory]=useState([])
  const [busy,setBusy]=useState('')
  const [error,setError]=useState('')
  const [now,setNow]=useState(Date.now())
  const [market,setMarket]=useState(null)
  const [liveInput,setLiveInput]=useState(null)
  const [rpcError,setRpcError]=useState('')
  const [keeperStatus,setKeeperStatus]=useState(null)
  const [walletError,setWalletError]=useState('')
  const [logError,setLogError]=useState('')
  const [decisions,setDecisions]=useState([])
  const [runStartedAt,setRunStartedAt]=useState(0)
  const [runUntil,setRunUntil]=useState(0)
  const run=useRef({start:0,end:0})
  const [lastTx,setLastTx]=useState('')
  const [preview,setPreview]=useState(null)
  const liveRef=useRef(null)
  const accountRef=useRef(address)
  const walletEpoch=useRef(0)
  const sessionKey=useRef('')
  accountRef.current=address
  const refreshing=useRef(false)
  const disconnected=useRef(false)
  const approveUSDC=parseUnits(config?.maxUSDC||'3',6)
  const approveWMON=parseUnits(config?.maxWMON||'3',18)

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
    const changed=a=>{setAddress(a?.[0]||'');setHistory([]);setBalances(null);setError('')}
    const chainChanged=()=>{setError('钱包网络已更改，请检查 Monad Testnet')}
    p.on?.('accountsChanged',changed);p.on?.('chainChanged',chainChanged)
    return()=>{live=false;p.removeListener?.('accountsChanged',changed);p.removeListener?.('chainChanged',chainChanged)}
  },[config])

  useEffect(()=>{
    let active=true,inflight=false
    async function poll(){
      if(inflight)return
      inflight=true
      try{
        const response=await fetch('/api/ai/rpc-live',{cache:'no-store'})
        if(!response.ok)throw Error('RPC 采样服务未启动')
        const data=await response.json()
        if(!active)return
        if(data.error)throw Error(data.error)
        liveRef.current=data
        setLiveInput(data);setMarket(data.tick);setRpcError('')
        if(run.current.start){
          const ticks=data.ticks.filter(t=>t.sampledAt>=run.current.start&&t.sampledAt<run.current.end)
          setDecisions(old=>{
            const merged=new Map(old.map(t=>[t.observedAt,t]))
            ticks.forEach(t=>merged.set(t.observedAt,t))
            return [...merged.values()].sort((a,b)=>a.sampledAt-b.sampledAt)
          })
        }
      }catch(e){if(active)setRpcError(errorText(e))}
      finally{inflight=false}
    }
    poll();const timer=setInterval(poll,1000)
    return()=>{active=false;clearInterval(timer)}
  },[])

  useEffect(()=>{
    if(!config)return undefined
    let active=true
    async function pollKeeper(){
      try{
        const response=await fetch('/ai/keeper-status.json?ts='+Date.now(),{cache:'no-store'})
        if(!response.ok)throw Error('keeper status unavailable')
        const data=await response.json()
        if(active)setKeeperStatus(data)
      }catch{if(active)setKeeperStatus(null)}
    }
    pollKeeper()
    const timer=setInterval(pollKeeper,3000)
    return()=>{active=false;clearInterval(timer)}
  },[config])

  const refresh=useCallback(async()=>{
    if(!config||!address||refreshing.current)return
    refreshing.current=true
    const epoch=walletEpoch.current
    const current=()=>accountRef.current===address&&!disconnected.current&&walletEpoch.current===epoch
    try{
      const blockNumber=await api.getBlockNumber({cacheTime:0})
      const [id,mon,usdc,wmon,usdcAllowance,wmonAllowance]=await Promise.all([
        api.readContract({address:config.contract,abi:sessionAbi,functionName:'sessions',args:[address],blockNumber}),
        api.getBalance({address,blockNumber}),
        api.readContract({address:config.usdc,abi:erc20,functionName:'balanceOf',args:[address],blockNumber}),
        api.readContract({address:config.wmon,abi:erc20,functionName:'balanceOf',args:[address],blockNumber}),
        api.readContract({address:config.usdc,abi:erc20,functionName:'allowance',args:[address,config.contract],blockNumber}),
        api.readContract({address:config.wmon,abi:erc20,functionName:'allowance',args:[address,config.contract],blockNumber})
      ])
      if(!current())return
      const [sid,startedAt,lastExecution,nextRound,isActive]=id
      const next={id:Number(sid),startedAt:Number(startedAt),lastExecution:Number(lastExecution),
        nextRound:Number(nextRound),active:isActive}
      const key=address+':'+next.id
      if(sessionKey.current!==key){sessionKey.current=key;setHistory([]);setLogError('')}
      setSession(next)
      setBalances({mon:formatUnits(mon,18),usdc:formatUnits(usdc,6),wmon:formatUnits(wmon,18)})
      setAllowances({usdc:usdcAllowance,wmon:wmonAllowance})
      setWalletError('')
      if(config.executionRules?.enabled&&liveRef.current?.input){
        const [predicted,action,reason]=await api.readContract({address:config.contract,abi:sessionAbi,
          functionName:'previewAction',args:[address,liveRef.current.input],blockNumber})
        if(current())setPreview({predicted:Number(predicted),action:Number(action),reason:Number(reason)})
      }
      if(next.id>0){
        try{
          const events=await activity(config,address,next)
          if(current()){setHistory(events);setLogError('')}
        }catch(e){if(current())setLogError('决策事件读取失败：'+errorText(e))}
      }else{setHistory([]);setLogError('')}
    }catch(e){if(current())setWalletError('钱包数据读取失败：'+errorText(e))}
    finally{refreshing.current=false}
  },[address,config])

  useEffect(()=>{
    if(!config||!address)return
    disconnected.current=false
    setBalances(null);setHistory([]);setPreview(null);setAllowances({usdc:0n,wmon:0n})
    setSession({id:0,startedAt:0,lastExecution:0,nextRound:0,active:false})
    const timer=setInterval(refresh,4500)
    refresh()
    return()=>{disconnected.current=true;walletEpoch.current++;clearInterval(timer)}
  },[config,address,refresh])


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
      const wrappedBalance=await api.readContract({address:config.wmon,abi:erc20,functionName:'balanceOf',args:[address]})
      if(wrappedBalance<approveWMON){
        setBusy('准备 MON')
        const hash=await w.writeContract({address:config.wmon,abi:erc20,functionName:'deposit',value:approveWMON-wrappedBalance})
        const receipt=await api.waitForTransactionReceipt({hash,timeout:150000})
        if(receipt.status!=='success')throw Error('MON 准备失败')
      }
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
      setLastTx(hash)
      if(method==='start')setHistory([])
      return true
    }catch(e){setError(errorText(e))}
    finally{setBusy('')}
    return false
  }

  const approved=allowances.usdc>=approveUSDC&&allowances.wmon>=approveWMON
  const running=runUntil>now
  const remaining=running?Math.ceil((runUntil-now)/1000):runStartedAt?0:60
  const latest=decisions.at(-1)
  const current=latest||market
  const done=history.filter(h=>h.round<6)
  const executed=done.filter(h=>h.action!==0).at(-1)||done.at(-1)
  const viewAction=executed?.action??(approved?preview?.action:null)??null
  const bought=done.filter(h=>h.action===1).length
  const sold=done.filter(h=>h.action===2).length
  const expired=session.active&&now/1000>session.startedAt+110
  const keeperAge=keeperStatus?.checkedAt?Date.now()-new Date(keeperStatus.checkedAt).getTime():Infinity
  const keeperOnline=Boolean(keeperStatus?.online&&keeperStatus.contract?.toLowerCase()===config?.contract?.toLowerCase()&&keeperAge<15000)
  const canTrade=address&&approved&&balances&&Number(balances.usdc)>=Number(config.maxUSDC)&&!busy&&keeperOnline&&(!session.active||expired)
  async function start(){
    if(running||!canTrade)return
    const start=Date.now()
    run.current={start,end:start+60000}
    setNow(start);setRunStartedAt(start);setRunUntil(start+60000);setDecisions([])
    // Wallet execution runs separately; neither the signature nor a receipt blocks sampling.
    const ok=await sendAction('start')
    if(!ok){run.current={start:0,end:0};setRunStartedAt(0);setRunUntil(0);setDecisions([])}
  }
  function stop(){run.current.end=Date.now();setNow(run.current.end);setRunUntil(run.current.end)}
  return <section className="ows-page">
    <div className="ows-mast">
      <div className="ows-overline"><span className="ows-live-dot"/> MONAD AI <span className="ows-dim">/</span> TESTNET TRADING</div>
      <div className="ows-mast-row">
        <div><h1>ONE START.<em>BUY. SELL.</em></h1><p>实时行情驱动 · AI 自动交易 · 成交直接回到你的钱包。</p></div>
        <div className="ows-wallet"><span>{address?'TRADING WALLET':'TRADING WALLET · OPTIONAL'}</span><strong>{address?trim(address):'NOT CONNECTED'}</strong>
          <button onClick={connect} disabled={Boolean(busy)||!config}>{address?'检查钱包':'连接 MetaMask'} <span>↗</span></button></div>
      </div>
      <Track/>
    </div>
    <div className="ows-main">
      <div className="ows-decision-stage">
        <div className="ows-top-line"><span>12 → 8 → 3 · ONCHAIN AI</span><span>{rpcError?'● 行情连接中':market?'● 实时行情':'● 连接中'}</span></div>
        <div className={'ows-action ows-action-'+(viewAction??'idle')}>
          <div className="ows-action-kicker">{executed?'最新链上成交':approved&&preview?'下一笔执行':'准备开始'}</div>
          <strong>{viewAction!==null?labels[viewAction]:'READY'}</strong>
          <p>{executed&&executed.action!==0
            ?`${fmt(formatUnits(BigInt(executed.amountIn),executed.action===1?6:18),6)} ${executed.action===1?'USDC':'MON'} → ${fmt(formatUnits(BigInt(executed.amountOut),executed.action===1?18:6),6)} ${executed.action===1?'MON':'USDC'}`
            :viewAction!==null?meaning[viewAction]:'连接钱包 · 开始交易'}
            {executed&&<> · <a href={'https://testnet.monadscan.com/tx/'+executed.hash} target="_blank" rel="noreferrer">已确认 ↗</a></>}
          </p>
          <div className="ows-model-signal">AI 信号 <b>{current?labels[current.action]:'—'}</b></div>
        </div>
        <div className="ows-process">
          <div><span>买入成交</span><strong>{bought}<small> 笔</small></strong></div>
          <div><span>卖出成交</span><strong>{sold}<small> 笔</small></strong></div>
          <div><span>执行进度</span><strong>{done.length}<small> / 6</small></strong></div>
        </div>
        <div className="ows-progress">{rounds.map(i=>{
          const trade=done.find(t=>t.round===i)
          return <div key={i} className={'ows-progress-step '+(trade?'completed':session.active&&!expired&&i===session.nextRound?'now':'future')}>
            <span>ROUND {i+1}</span><strong>{trade?labels[trade.action]:'—'}</strong><small>{trade?'已确认':session.active&&!expired&&i===session.nextRound?'执行中':'待执行'}</small>
          </div>
        })}</div>
        <details className="ows-data-details"><summary>行情与模型数据</summary>
        <div className="ows-rpc" aria-label="RPC 链上数据">
          <div><span>最新 RPC 区块</span><strong>{market?'#'+market.block:'—'}</strong><small>{market?new Date(market.sampledAt).toLocaleTimeString():'正在连接 Monad'}</small></div>
          <div><span>WMON / USDC 池价</span><strong>{market?fmt(market.price,8)+' USDC':'—'}</strong><small>slot0 · 同一区块读取</small></div>
          <div><span>当前小时成交量</span><strong>{market?fmt(market.volume,4)+' USDC':'—'}</strong><small>真实 Swap 日志累计</small></div>
          <div><span>模型分数 · HOLD / BUY / SELL</span><strong>{current?current.scores.join(' / '):'—'}</strong><small>predict() · eth_call</small></div>
        </div>
        {liveInput&&<a href="/api/ai/rpc-live" target="_blank" rel="noreferrer">查看原始输入 ↗</a>}
        </details>
        {rpcError&&<p className="ows-error" role="alert">行情读取失败：{rpcError}</p>}
      </div>
      <aside className="ows-control-stage">
        <div className="ows-section-label">AUTO TRADING</div>
        <div className="ows-exec-details"><span>网络 <b>Monad Testnet</b></span><span>买入使用 <b>{config?.buyUSDC||'1'} USDC</b></span><span>卖出使用 <b>{config?.sellWMON||'1'} MON</b></span><span>交易轮次 <b>6 rounds / 10s</b></span><span>Keeper <b className={keeperOnline?'ows-online':'ows-offline'}>{keeperOnline?'ONLINE':'OFFLINE'}</b></span></div>
        <div className={'ows-run-status '+(running?'running':'')} role="status"><span>{running?'AI 1 分钟采样运行中':'AI 1 分钟采样窗口'}</span><strong>{running?`${remaining}s`:'60s'}</strong></div>
        <div className="ows-holdings">
          <div><span>USDC</span><strong>{balances?fmt(balances.usdc,6):'—'}</strong></div>
          <div><span title="WMON">MON</span><strong>{balances?fmt(balances.wmon,6):'—'}</strong></div>
          <div><span>Gas · MON</span><strong>{balances?fmt(balances.mon,5):'—'}</strong></div>
        </div>
        {!address?<button className="ows-primary" onClick={connect} disabled={Boolean(busy)}>{busy||'连接钱包开始'} <span>→</span></button>
          :!approved?<button className="ows-primary" onClick={grant} disabled={Boolean(busy)}>{busy||'准备并授权'} <span>→</span></button>
          :session.active&&!expired?<button className="ows-cancel" onClick={()=>sendAction('cancel')} disabled={Boolean(busy)}>{busy||'停止交易'}</button>
          :<button className="ows-primary" onClick={start} disabled={!canTrade||running}>{busy||(running?`运行中 · ${remaining}s`:'START AUTO TRADING')} <span>→</span></button>}
        {lastTx&&<a className="ows-subtext" href={'https://testnet.monadscan.com/tx/'+lastTx} target="_blank" rel="noreferrer">交易已广播 {trim(lastTx)} ↗</a>}
        {!keeperOnline&&<p className="ows-error" role="alert">Keeper 未在线，暂时不会自动执行 6 轮交易。</p>}
        {walletError&&<p className="ows-error" role="alert">{walletError}</p>}
        {error&&<p className="ows-error" role="alert">{error}</p>}
        <div className="ows-links">{config&&<a href={config.explorer} target="_blank" rel="noreferrer">交易会话合约 ↗</a>}
          {config&&<a href={'https://testnet.monadscan.com/address/'+config.model} target="_blank" rel="noreferrer">原 AI 模型 ↗</a>}</div>
      </aside>
    </div>
    <div className="ows-ledger"><div className="ows-ledger-title"><div><span>ONCHAIN EXECUTION</span><h2>交易记录</h2></div><span>{bought} 笔 BUY · {sold} 笔 SELL</span></div>
      <div className="ows-table-wrap"><table><thead><tr><th>轮次</th><th>动作</th><th>支付</th><th>收到</th><th>状态</th><th>交易</th></tr></thead>
        <tbody>{done.map(item=><tr key={item.round} className="ows-done-row"><td>{item.round+1}</td>
          <td><span className={'ows-signal ows-signal-'+item.action}>{labels[item.action]}</span></td>
          <td>{item.action===0?'—':fmt(formatUnits(BigInt(item.amountIn),item.action===1?6:18),6)+' '+(item.action===1?'USDC':'MON')}</td>
          <td>{item.action===0?'—':fmt(formatUnits(BigInt(item.amountOut),item.action===1?18:6),6)+' '+(item.action===1?'MON':'USDC')}</td>
          <td>已确认</td><td><a href={'https://testnet.monadscan.com/tx/'+item.hash} target="_blank" rel="noreferrer">{trim(item.hash)} ↗</a></td></tr>)}</tbody>
      </table></div>{!done.length&&<p className="ows-callout">成交后在这里查看数量与交易记录。</p>}
    </div>
    <details className="ows-ledger ows-samples"><summary>模型信号记录 · {decisions.length} 次</summary>
      <div className="ows-ledger-title"><div><span>ONE SECOND · ONE RPC SAMPLE</span><h2>Live decisions</h2></div><span>{decisions.length} 次实际采样 · 记录区块、输入与模型结果</span></div>
      <div className="ows-table-wrap"><table><thead><tr><th>#</th><th>TIME</th><th>RPC BLOCK</th><th>PRICE · USDC</th><th>AI SIGNAL</th><th>Q12 SCORES</th><th>INPUT HASH</th></tr></thead>
        <tbody>{decisions.map((item,i)=><tr key={item.observedAt} className="ows-done-row">
          <td>{String(i+1).padStart(2,'0')}</td><td>{new Date(item.sampledAt).toLocaleTimeString()}</td>
          <td>{item.block}</td><td>{fmt(item.price,8)}</td><td><span className={'ows-signal ows-signal-'+item.action}>{labels[item.action]}</span></td>
          <td>{item.scores.join(' / ')}</td><td title={item.input.join(', ')}>{trim(item.inputHash)}</td>
        </tr>)}</tbody></table></div>
      {!decisions.length&&<p className="ows-callout">点击 Start 开始记录。</p>}
    </details>
    {logError&&<p className="ows-error" role="alert">{logError}</p>}
    <footer className="ows-footer"><span>MARKET DATA · MONAD MAINNET RPC</span><span>ORIGINAL MODEL · TESTNET 10143</span></footer>
  </section>
}
