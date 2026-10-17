import React, { useEffect, useState, useRef, lazy, Suspense } from 'react'
import { createRoot } from 'react-dom/client'
import MonadListener from './MonadListener'
import MonadDemo from './MonadDemo'
import './styles.css'
const PoolSelection = lazy(() => import('./PoolSelection'))
const AiShowcase = lazy(() => import('./ai/AiShowcase'))

const api = (path, options = {}) => fetch(path, options).then((response) => {
  if (!response.ok) throw new Error(`API ${response.status}`)
  return response.json()
})

const formatMoney = (value, compact = true) => {
  if (value == null || Number.isNaN(Number(value))) return '—'
  const n = Number(value)
  if (compact) {
    if (Math.abs(n) >= 1e12) return `$${n.toExponential(2).replace('e+', 'e')}`
    if (Math.abs(n) >= 1e9) return `$${(n / 1e9).toFixed(2)}B`
    if (Math.abs(n) >= 1e6) return `$${(n / 1e6).toFixed(2)}M`
    if (Math.abs(n) >= 1e3) return `$${(n / 1e3).toFixed(1)}K`
  }
  return `$${n.toLocaleString('en-US', { maximumFractionDigits: 2 })}`
}

const formatPercent = (value) => {
  if (value == null || Number.isNaN(Number(value))) return '—'
  return `${(Number(value) * 100).toFixed(2)}%`
}

const formatMultiple = (value) => {
  if (value == null || Number.isNaN(Number(value))) return '—'
  const n = Number(value)
  if (n >= 100) return `${n.toFixed(0)}x`
  if (n >= 10) return `${n.toFixed(1)}x`
  return `${n.toFixed(2)}x`
}

const formatPrice = (value) => {
  if (value == null) return '—'
  const n = Number(value)
  if (n >= 1000) return `$${n.toLocaleString('en-US', { maximumFractionDigits: 0 })}`
  if (n >= 1) return `$${n.toFixed(2)}`
  return `$${n.toPrecision(5)}`
}

const formatVolatility = (value) => {
  if (value == null || Number.isNaN(Number(value))) return '—'
  return `${(Number(value) * 100).toFixed(1)}%`
}
const realizedVolatility = (pool, days) => pool?.[`realized_vol_${days}d`] ?? pool?.[`iv_${days}_annualized`]
const poolVolatility = (pool) => realizedVolatility(pool, 7) ?? realizedVolatility(pool, 1)
const poolVolatilitySource = (pool) => pool?.historical_pair_price_semantics === 'external_market_pair_proxy'
  ? 'External market proxy'
  : pool?.historical_pair_price_semantics === 'pool_local' ? 'Pool-local history' : 'Missing'

const tokenLabel = (pool, side) => pool?.[`token_${side}_symbol`] || shortAddress(pool?.[`token_${side}_address`] || '') || side.toUpperCase()
const tokenPrice = (pool, side) => {
  const latest = pool?.["token_" + side + "_price_usd"]
  const average = pool?.["token_" + side + "_avg_price_usd"]
  return tokenLabel(pool, side) + ' ' + formatPrice(latest ?? average) + (latest == null && average != null ? ' · avg' : '')
}
const pairMetric = (a, b, format, missingLabel = 'No history') => {
  const hasA = a != null && !Number.isNaN(Number(a))
  const hasB = b != null && !Number.isNaN(Number(b))
  if (!hasA && !hasB) return <span className="pair-metric-empty">{missingLabel}</span>
  return <div className="pair-prices"><span>A {hasA ? format(a) : '—'}</span><span>B {hasB ? format(b) : '—'}</span></div>
}

const shortAddress = (value = '') => value.length > 16 ? `${value.slice(0, 7)}…${value.slice(-6)}` : value
const labelChain = (value = '') => value.split('-')[0].replace(/\b\w/g, (letter) => letter.toUpperCase())
const WATCHLIST_KEY = 'lp-screener.watchlist.v1'
const explorerBases = {
  ethereum: 'https://etherscan.io/address/', arbitrum: 'https://arbiscan.io/address/', avalanche: 'https://snowtrace.io/address/',
  base: 'https://basescan.org/address/', blast: 'https://blastscan.io/address/', bsc: 'https://bscscan.com/address/',
  linea: 'https://lineascan.build/address/', mantle: 'https://mantlescan.xyz/address/', monad: 'https://monadscan.com/address/',
  optimism: 'https://optimistic.etherscan.io/address/', polygon: 'https://polygonscan.com/address/', scroll: 'https://scrollscan.com/address/',
  zksync: 'https://era.zksync.network/address/',
}
const explorerUrl = (pool) => {
  if (!pool?.pool_address) return null
  if (pool.chain === 'solana') return `https://solscan.io/account/${pool.pool_address}`
  if (!/^0x[0-9a-fA-F]{40}$/.test(pool.pool_address)) return null
  const base = explorerBases[pool.chain]
  return base ? `${base}${pool.pool_address}` : null
}
const formatAge = (seconds) => {
  if (seconds == null || !Number.isFinite(Number(seconds))) return '—'
  const value = Math.max(0, Number(seconds))
  if (value < 60) return `${Math.round(value)}s ago`
  if (value < 3600) return `${Math.floor(value / 60)}m ago`
  return `${Math.floor(value / 3600)}h ${Math.floor((value % 3600) / 60)}m ago`
}
const unixAge = (timestamp) => timestamp ? Math.max(0, Date.now() / 1000 - Number(timestamp)) : null
const chainPalette = ['#58d9ff', '#b8f05a', '#ffad66', '#c49bff', '#ff827b', '#70d6c4', '#f7df75']
const UI_MIN_LIQUIDITY_USD = 0.000000000001
const POOL_PAGE_SIZE = 25
const LIVE_RPC_STATES = new Set(['live', 'account_present'])
const isDisplayablePool = (pool) => Number(pool?.liquidity_usd) > 0 && LIVE_RPC_STATES.has(pool?.rpc_state_status)

function Icon({ name, size = 16 }) {
  const common = { width: size, height: size, viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', strokeWidth: 1.7, strokeLinecap: 'round', strokeLinejoin: 'round', 'aria-hidden': true }
  const paths = {
    grid: <><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/></>,
    search: <><circle cx="10.8" cy="10.8" r="6.7"/><path d="m16 16 4.3 4.3"/></>,
    sliders: <><path d="M4 6h7M16 6h4M4 12h3M12 12h8M4 18h9M18 18h2"/><circle cx="13.5" cy="6" r="2.2"/><circle cx="9.5" cy="12" r="2.2"/><circle cx="15.5" cy="18" r="2.2"/></>,
    refresh: <><path d="M20 11a8 8 0 0 0-14.9-3L3 11"/><path d="M3 5v6h6"/><path d="M4 13a8 8 0 0 0 14.9 3L21 13"/><path d="M21 19v-6h-6"/></>,
    arrow: <><path d="M5 12h14"/><path d="m13 6 6 6-6 6"/></>,
    close: <><path d="m6 6 12 12M18 6 6 18"/></>,
    external: <><path d="M14 4h6v6"/><path d="M20 4 11 13"/><path d="M18 13v5a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h5"/></>,
    chart: <><path d="M4 19V5M4 19h16"/><path d="m7 15 3-4 3 2 5-7"/></>,
    database: <><ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v7c0 1.7 3.6 3 8 3s8-1.3 8-3V5"/><path d="M4 12v7c0 1.7 3.6 3 8 3s8-1.3 8-3v-7"/></>,
    bookmark: <path d="M6 4a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v18l-6-3-6 3V4Z"/>,
    pulse: <><path d="M3 12h4l2.3-6 4.1 12 2.2-6H21"/><path d="M12 3v2M12 19v2"/></>,
  }
  return <svg {...common}>{paths[name] || paths.grid}</svg>
}

function ActivityMeter({ ratio }) {
  const width = Math.max(8, Math.min(100, Math.log10(Math.max(1, Number(ratio || 0))) * 42 + 12))
  return <div className="activity-meter" title="24h volume / TVL"><span style={{ width: `${width}%` }}/></div>
}

function ChainDot({ chain }) {
  const index = [...String(chain || '')].reduce((sum, char) => sum + char.charCodeAt(0), 0) % chainPalette.length
  return <span className="chain-dot" style={{ background: chainPalette[index], boxShadow: `0 0 8px ${chainPalette[index]}55` }} />
}

function App() {
  const [summary, setSummary] = useState(null)
  const [meta, setMeta] = useState({ protocols: [], pairs: [], chains: [] })
  const [health, setHealth] = useState(null)
  const [pools, setPools] = useState([])
  const [total, setTotal] = useState(0)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [selected, setSelected] = useState(null)
  const chain = 'monad'
  const [query, setQuery] = useState('')
  const [protocol, setProtocol] = useState('all')
  const [pair, setPair] = useState('all')
  const [sort, setSort] = useState('volume_24h_usd')
  const [order, setOrder] = useState('desc')
  const [page, setPage] = useState(1)
  const [lastUpdated, setLastUpdated] = useState(new Date())
  const [pairCategory, setPairCategory] = useState('all')
  const [poolType, setPoolType] = useState('all')
  const [minLiquidity, setMinLiquidity] = useState('')
  const [minVolume, setMinVolume] = useState('')
  const [minFeeBps, setMinFeeBps] = useState('')
  const [minAprPercent, setMinAprPercent] = useState('')
  const [aprStatus, setAprStatus] = useState('all')
  const [rpcStatus, setRpcStatus] = useState('live')
  const [filtersOpen, setFiltersOpen] = useState(false)
  const [rpcState, setRpcState] = useState(null)
  const [rpcRefresh, setRpcRefresh] = useState(null)
  const [watchlist, setWatchlist] = useState(() => {
    try { return JSON.parse(window.localStorage.getItem(WATCHLIST_KEY) || '[]') } catch { return [] }
  })
  const [watchlistOnly, setWatchlistOnly] = useState(false)
  const [view, setView] = useState(() => {
    const page = window.location.hash.slice(1)
    return ['ai', 'selection', 'demo', 'monad', 'market'].includes(page) ? page : 'demo'
  })
  useEffect(() => {window.history.replaceState(null, '', `#${view}`)}, [view])
  useEffect(() => {
    const onHashChange = () => {
      const page = window.location.hash.slice(1)
      if (['ai', 'selection', 'demo', 'monad', 'market'].includes(page)) setView(page)
    }
    window.addEventListener('hashchange', onHashChange)
    return () => window.removeEventListener('hashchange', onHashChange)
  }, [])
  const [toast, setToast] = useState('')
  const poolRequest = useRef({ id: 0, controller: null, inFlight: false })
  const runtimeRequest = useRef({ controller: null, inFlight: false })

  const loadRuntime = async () => {
    if (runtimeRequest.current.inFlight) return
    const controller = new AbortController()
    runtimeRequest.current = { controller, inFlight: true }
    try {
      const [healthData, stateData, refreshData] = await Promise.all([
        api('/api/health', { signal: controller.signal }).catch(() => ({ source: 'offline' })),
        api('/api/rpc-state/summary', { signal: controller.signal }).catch(() => null),
        api('/api/rpc-refresh', { signal: controller.signal }).catch(() => null),
      ])
      if (controller.signal.aborted) return
      setHealth(healthData)
      setRpcState(stateData)
      setRpcRefresh(refreshData)
    } finally {
      if (runtimeRequest.current.controller === controller) runtimeRequest.current = { controller: null, inFlight: false }
    }
  }

  const loadPools = async ({ silent = false } = {}) => {
    if (watchlistOnly && !watchlist.length) {
      poolRequest.current.controller?.abort()
      poolRequest.current = { id: poolRequest.current.id + 1, controller: null, inFlight: false }
      setPools([])
      setTotal(0)
      setLastUpdated(new Date())
      setLoading(false)
      return
    }
    if (poolRequest.current.inFlight) {
      if (silent) return
      poolRequest.current.controller?.abort()
    }
    const requestId = poolRequest.current.id + 1
    const controller = new AbortController()
    poolRequest.current = { id: requestId, controller, inFlight: true }
    if (!silent) setLoading(true)
    setError('')
    try {
      const params = new URLSearchParams({ chain, q: query, protocol, pair, sort, order, page: String(page), page_size: String(POOL_PAGE_SIZE) })
      if (watchlistOnly) params.set('addresses', watchlist.join(','))
      if (pairCategory !== 'all') params.set('pair_category', pairCategory)
      if (poolType !== 'all') params.set('pool_type', poolType)
      // Keep the catalog in SQLite, but show only executable flow candidates.
      // The tiny positive floor excludes NULL/zero-liquidity rows without
      // changing the user's requested minimum TVL when it is higher.
      params.set('min_liquidity_usd', String(Math.max(UI_MIN_LIQUIDITY_USD, Number(minLiquidity) || 0)))
      if (minVolume) params.set('min_volume_24h_usd', minVolume)
      if (minFeeBps) params.set('min_fee_bps', minFeeBps)
      if (minAprPercent) params.set('min_lp_fee_apr', String(Number(minAprPercent) / 100))
      if (aprStatus !== 'all') params.set('fee_apr_status', aprStatus)
      // Non-live pools remain queryable in SQLite but never enter the flow UI.
      params.set('rpc_status', 'live')
      const [poolData, summaryData, metaData] = await Promise.all([
        api(`/api/pools?${params}`, { signal: controller.signal }),
        summary ? Promise.resolve(summary) : api('/api/summary?chain=monad', { signal: controller.signal }),
        meta.protocols.length ? Promise.resolve(meta) : api('/api/meta?chain=monad', { signal: controller.signal }),
      ])
      if (controller.signal.aborted || poolRequest.current.id !== requestId) return
      const visiblePools = (poolData.items || []).filter((pool) => pool.chain === chain && pool.pool_price_status !== 'unavailable' && isDisplayablePool(pool))
      setPools(visiblePools)
      setTotal(Math.max(0, Number(poolData.total || 0) - ((poolData.items || []).length - visiblePools.length)))
      setSummary(summaryData)
      setMeta(metaData)
      setLastUpdated(new Date())
    } catch (err) {
      if (err?.name !== 'AbortError' && !controller.signal.aborted && poolRequest.current.id === requestId) {
        setError('无法连接本地数据服务，请先启动 API server')
      }
    } finally {
      if (poolRequest.current.id === requestId) {
        poolRequest.current = { id: requestId, controller: null, inFlight: false }
        if (!silent) setLoading(false)
      }
    }
  }

  useEffect(() => {
    if (view !== 'market') return undefined
    const timer = window.setTimeout(() => loadPools(), query ? 250 : 0)
    return () => {
      window.clearTimeout(timer)
      poolRequest.current.controller?.abort()
    }
  }, [view, chain, query, protocol, pair, sort, order, page, pairCategory, poolType, minLiquidity, minVolume, minFeeBps, minAprPercent, aprStatus, rpcStatus, watchlistOnly, watchlist])
  useEffect(() => { setPage(1) }, [chain, query, protocol, pair, sort, order, pairCategory, poolType, minLiquidity, minVolume, minFeeBps, minAprPercent, aprStatus, rpcStatus])
  useEffect(() => {
    if (view !== 'market') return undefined
    loadRuntime()
    const timer = window.setInterval(() => {
      loadPools({ silent: true })
      loadRuntime()
    }, 30000)
    return () => {
      window.clearInterval(timer)
      runtimeRequest.current.controller?.abort()
    }
  }, [view])
  useEffect(() => {
    api('/api/watchlist').then(async (data) => {
      const remote = Array.isArray(data.addresses) ? data.addresses : []
      if (remote.length) {
        setWatchlist(remote)
        window.localStorage.setItem(WATCHLIST_KEY, JSON.stringify(remote))
        return
      }
      const local = watchlist
      if (local.length) {
        await Promise.all(local.map((pool_address) => api('/api/watchlist', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ pool_address }) }).catch(() => null)))
      }
    }).catch(() => {})
  }, [])
  const openPool = async (pool) => {
    try {
      const detail = await api(`/api/pools/${encodeURIComponent(pool.pool_address)}`)
      setSelected(detail)
    } catch {
      setSelected(pool)
    }
  }

  const toggleWatchlist = async (pool) => {
    const address = String(pool?.pool_address || '')
    if (!address) return
    const exists = watchlist.some((item) => item.toLowerCase() === address.toLowerCase())
    const next = exists ? watchlist.filter((item) => item.toLowerCase() !== address.toLowerCase()) : [...watchlist, address]
    try {
      await api(exists ? `/api/watchlist/${encodeURIComponent(address)}` : '/api/watchlist', {
        method: exists ? 'DELETE' : 'POST',
        ...(exists ? {} : { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ pool_address: address }) }),
      })
      setToast(exists ? 'Removed from local watchlist' : 'Saved to local watchlist')
    } catch {
      setToast('API unavailable · saved in this browser only')
    }
    setWatchlist(next)
    window.localStorage.setItem(WATCHLIST_KEY, JSON.stringify(next))
    window.setTimeout(() => setToast(''), 2200)
  }

  const clearWatchlist = async () => {
    try { await api('/api/watchlist', { method: 'DELETE' }) } catch { /* local fallback below */ }
    setWatchlist([])
    window.localStorage.removeItem(WATCHLIST_KEY)
    setToast('Watchlist cleared')
    window.setTimeout(() => setToast(''), 2200)
  }

  const exportWatchlist = () => {
    const blob = new Blob([JSON.stringify({ exported_at: new Date().toISOString(), pool_addresses: watchlist }, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const link = document.createElement('a'); link.href = url; link.download = 'lp-watchlist.json'; link.click(); URL.revokeObjectURL(url)
  }

  const isWatchlisted = (pool) => watchlist.some((item) => item.toLowerCase() === String(pool?.pool_address || '').toLowerCase())

  const setSortValue = (value) => {
    if (sort === value) setOrder(order === 'desc' ? 'asc' : 'desc')
    else { setSort(value); setOrder('desc') }
  }

  const pages = Math.max(1, Math.ceil(total / POOL_PAGE_SIZE))
  const activeFilters = [pairCategory !== 'all', poolType !== 'all', Boolean(minLiquidity), Boolean(minVolume), Boolean(minFeeBps), Boolean(minAprPercent), aprStatus !== 'all'].filter(Boolean).length
  const stateAge = unixAge(rpcState?.checked_at)
  const monadRpcState = rpcState?.chains?.find((item) => item.chain === chain)
  const refreshAge = unixAge(rpcRefresh?.cycle_finished_at ?? rpcState?.checked_at)
  const refreshStale = refreshAge == null || refreshAge > Number(rpcRefresh?.interval_secs || 900) * 2

  return <div className="app-shell">
    <aside className="sidebar">
      <div className="brand"><span className="brand-mark">LP</span><span>SCREENER</span></div>
      <nav className="side-nav">
        <div className="nav-label">Workspace</div>
        <button className={`nav-item ${view === 'market' && !watchlistOnly ? 'active' : ''}`} onClick={() => { setView('market'); setWatchlistOnly(false); setPage(1) }}><Icon name="grid"/> Pool market</button>
        <button className={`nav-item ${view === 'market' && watchlistOnly ? 'active' : ''}`} onClick={() => { setView('market'); setWatchlistOnly(true); setPage(1) }}><Icon name="bookmark"/> Watchlist <span className="nav-count">{watchlist.length}</span></button>
        <button className={`nav-item ${view === 'monad' ? 'active' : ''}`} onClick={() => { setView('monad'); setWatchlistOnly(false) }}><Icon name="pulse"/> Wallet listener</button>
        <button className={`nav-item ${view === 'demo' ? 'active' : ''}`} onClick={() => { setView('demo'); setWatchlistOnly(false) }}><Icon name="pulse"/> 20 pool</button>
        <button className={`nav-item ${view === 'selection' ? 'active' : ''}`} onClick={() => { setView('selection'); setWatchlistOnly(false) }}><Icon name="pulse"/> 最优报价</button>
        <button className={`nav-item ${view === 'ai' ? 'active' : ''}`} onClick={() => { setView('ai'); setWatchlistOnly(false) }}><Icon name="chart"/> AI 交易</button>
      </nav>
      <div className="sidebar-footer">
        <div className="source-status"><span className="status-dot"/> {health?.source === 'sqlite' ? 'SQLite primary' : 'Data service starting'}</div>
        <div className="source-caption">{summary?.pools?.toLocaleString() || '—'} pools indexed<br/>Monad · {health?.source || 'checking'}</div>
      </div>
    </aside>

    <main className="main-content">
      <header className="topbar">
        <div className="breadcrumb"><span>{view === 'monad' ? 'Chain monitoring' : view === 'ai' ? 'Onchain AI' : 'Markets'}</span><span className="slash">/</span><strong>{view === 'monad' ? 'Wallet listener' : view === 'demo' ? '20 pool' : view === 'selection' ? '最优报价' : view === 'ai' ? 'AI 交易' : 'LP pools'}</strong></div>
       <div className="top-actions">{!['monad', 'demo', 'selection', 'ai'].includes(view) && <><span className={`live-dot ${refreshStale ? 'stale' : ''}`}/><span>Local SQLite snapshot</span></>}<button className="icon-button" onClick={() => { if (view === 'monad') window.dispatchEvent(new Event('monad-refresh')); else if (view === 'demo') window.dispatchEvent(new Event('monad-demo-refresh')); else if (view === 'ai') window.location.reload(); else { loadPools(); loadRuntime() } }} title="Refresh"><Icon name="refresh"/></button></div>
     </header>

      {!['monad', 'demo', 'selection', 'ai'].includes(view) && <section className="page-heading">
        <div>
          <h1>{watchlistOnly ? 'Watchlist' : 'Pool market'}</h1>
          <p>{watchlistOnly ? 'Your saved pools, persisted through the local Rust API.' : 'Scan liquidity, flow and fee opportunity across Monad pools.'}</p>
        </div>
        {watchlistOnly && <div className="heading-actions"><button className="secondary-button compact" onClick={exportWatchlist}>Export JSON</button><button className="secondary-button compact" onClick={clearWatchlist} disabled={!watchlist.length}>Remove all</button></div>}
        <div className="snapshot-time">UI sync {lastUpdated.toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit' })}</div>
      </section>}

      {!['monad', 'demo', 'selection', 'ai'].includes(view) && <RefreshBanner report={rpcRefresh} age={refreshAge} stateAge={stateAge} stale={refreshStale}/>}

      {view === 'monad' ? <MonadListener/> : view === 'demo' ? <MonadDemo/> : view === 'selection' ? <Suspense fallback={<p>加载选池策略…</p>}><PoolSelection/></Suspense> : view === 'ai' ? <Suspense fallback={<p>加载演示…</p>}><AiShowcase/></Suspense> : <>
      <section className="metric-strip">
        <Metric label="Indexed pools" value={summary?.pools?.toLocaleString() || '—'} detail="Monad" />
        <Metric label="Total liquidity" value={formatMoney(summary?.liquidity)} detail="Across Monad venues" />
        <Metric label="Volume · 24h" value={formatMoney(summary?.volume)} detail="Rolling market snapshot" />
        <Metric label="RPC state" value={rpcState ? `${Number(monadRpcState?.live || 0).toLocaleString()} live` : '—'} detail={`${Number(monadRpcState?.total || 0).toLocaleString()} checked Monad pools`} accent />
      </section>

      <section className="market-panel">
        <div className="market-tabs">
          <div className="chain-tab selected"><ChainDot chain={chain}/> Monad</div>
        </div>
        <div className="toolbar">
          <label className="search-box"><Icon name="search"/><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search token, pair or pool address"/><kbd>/</kbd></label>
          <select value={protocol} onChange={(event) => setProtocol(event.target.value)}><option value="all">All DEXs</option>{meta.protocols.map((item) => <option key={item} value={item}>{item}</option>)}</select>
          <select value={pair} onChange={(event) => setPair(event.target.value)}><option value="all">All pairs</option>{meta.pairs.slice(0, 80).map((item) => <option key={item} value={item}>{item}</option>)}</select>
          <button className={`filter-button ${filtersOpen ? 'selected' : ''}`} onClick={() => setFiltersOpen((value) => !value)}><Icon name="sliders"/> Filters <span>{activeFilters}</span></button>
        </div>
        {filtersOpen && <div className="filter-drawer">
          <div className="filter-section"><strong>Market</strong><div className="filter-fields">
            <label><span>Pair category</span><select value={pairCategory} onChange={(event) => setPairCategory(event.target.value)}><option value="all">All categories</option><option value="stable_quote">Stable quoted</option><option value="stable_stable">Stable / stable</option><option value="major_pair">Major pair</option><option value="other">Other</option></select></label>
            <label><span>Minimum TVL · USD</span><input inputMode="decimal" value={minLiquidity} onChange={(event) => setMinLiquidity(event.target.value.replace(/[^\d.]/g, ''))} placeholder="100000" /></label>
            <label><span>Minimum volume · 24h</span><input inputMode="decimal" value={minVolume} onChange={(event) => setMinVolume(event.target.value.replace(/[^\d.]/g, ''))} placeholder="100000" /></label>
          </div></div>
          <div className="filter-section"><strong>Fees</strong><div className="filter-fields">
            <label><span>Minimum fee · bps</span><input inputMode="decimal" value={minFeeBps} onChange={(event) => setMinFeeBps(event.target.value.replace(/[^\d.]/g, ''))} placeholder="1" /></label>
            <label><span>Minimum fee APR · %</span><input inputMode="decimal" value={minAprPercent} onChange={(event) => setMinAprPercent(event.target.value.replace(/[^\d.]/g, ''))} placeholder="5" /></label>
            <label><span>APR data</span><select value={aprStatus} onChange={(event) => setAprStatus(event.target.value)}><option value="all">All pools</option><option value="available">APR available</option><option value="missing">APR missing</option></select></label>
          </div></div>
          <div className="filter-section"><strong>Protocol</strong><div className="filter-fields">
            <label><span>Pool type</span><select value={poolType} onChange={(event) => setPoolType(event.target.value)}><option value="all">All pool types</option><option value="constant_product_v2">Constant product</option><option value="solidly_v2">Solidly V2</option><option value="concentrated_v3">Concentrated V3</option><option value="stable_swap">Stable swap</option><option value="weighted">Weighted</option><option value="liquidity_book">Liquidity Book</option><option value="uniswap_v4">Uniswap V4</option><option value="unsupported_protocol">Unsupported decoder</option></select></label>
            <label><span>RPC state</span><select value="live" disabled><option value="live">Live / present only</option></select></label>
          </div></div>
          <button className="clear-filter" onClick={() => { setPairCategory('all'); setPoolType('all'); setMinLiquidity(''); setMinVolume(''); setMinFeeBps(''); setMinAprPercent(''); setAprStatus('all'); setRpcStatus('live') }}>Clear all</button>
        </div>}
        <div className="table-meta"><span><strong>{total.toLocaleString()}</strong> pools found</span><span className="meta-right">Page {page} of {pages}</span></div>
        {error ? <div className="error-state">{error}<button onClick={loadPools}>Retry</button></div> : <div className="pool-table-wrap">
          <table className="pool-table">
            <thead><tr>
              <th className="star-col">☆</th><th>Pool</th><th className="numeric">A / B price</th><th className="numeric">Avg · 1d A / B</th><th className="numeric">Avg · 7d A / B</th><th className="numeric">24h / TVL</th><th>Activity</th>
              <th className="numeric" title="Token A/B dailyized volatility from local history; backend widens the window when recent samples are sparse">Token daily vol A / B</th>
              <SortableHeader label="Liquidity" value="liquidity_usd" sort={sort} order={order} onSort={setSortValue}/>
              <SortableHeader label="Volume 24h" value="volume_24h_usd" sort={sort} order={order} onSort={setSortValue}/>
              <SortableHeader label="Volume 7d" value="volume_7d_usd" sort={sort} order={order} onSort={setSortValue}/>
              <SortableHeader label="Fee APR" value="fee_apr" sort={sort} order={order} onSort={setSortValue}/>
              <SortableHeader label="7d Fee APR" value="7d_fee_apr" sort={sort} order={order} onSort={setSortValue}/>
              <th className="numeric" title="Daily realized pair volatility; the source is shown in each pool detail">Daily vol</th>
              <th className="arrow-col" />
            </tr></thead>
            <tbody>{loading ? <SkeletonRows/> : pools.map((pool, index) => <PoolRow key={pool.pool_address} pool={pool} index={index} onOpen={openPool} watched={isWatchlisted(pool)} onToggle={toggleWatchlist}/>)}</tbody>
          </table>
          {!loading && pools.length === 0 && <div className="empty-state">No pools match this filter.</div>}
        </div>}
        <div className="pagination"><button disabled={page <= 1} onClick={() => setPage((value) => value - 1)}>Previous</button><span>{page} / {pages}</span><button disabled={page >= pages} onClick={() => setPage((value) => value + 1)}>Next</button></div>
      </section>

      </>}
    </main>
    {selected && <PoolDrawer pool={selected} watched={isWatchlisted(selected)} onToggle={toggleWatchlist} onOpenExplorer={() => { const url = explorerUrl(selected); if (url) window.open(url, '_blank', 'noopener,noreferrer') }} onClose={() => setSelected(null)}/>}
    {toast && <div className="toast" role="status">{toast}</div>}
  </div>
}

function RefreshBanner({ report, age, stateAge, stale }) {
  const liveWithoutReport = report?.available === false && stateAge != null && stateAge < 900
  const running = (report?.status === 'ok' && !stale) || liveWithoutReport
  const chip = report?.available === false ? 'SQLite state snapshot' : 'SQLite state refreshed'
  return <section className={`refresh-banner ${stale && !liveWithoutReport ? 'is-stale' : ''}`}>
    <div className="refresh-pulse"><span className="pulse-ring"/><span className="pulse-core"/></div>
    <div className="refresh-copy"><span className="eyebrow">Live state loop</span><strong>{running ? 'RPC refresh is on cadence' : 'RPC refresh needs attention'}</strong><span>{report?.selected ? `${Number(report.selected).toLocaleString()} pools in the last cycle` : liveWithoutReport ? 'Batches are landing in the live state table' : 'No completed cycle recorded yet'}</span></div>
    <div className="refresh-stat"><span>Last cycle</span><strong>{formatAge(age)}</strong></div>
    <div className="refresh-stat"><span>Throughput</span><strong>{report?.pools_per_second ? `${Number(report.pools_per_second).toFixed(1)} pools/s` : '—'}</strong></div>
    <div className={`refresh-chip ${report?.available === false ? 'warn' : 'ok'}`}><span/>{chip}</div>
  </section>
}

function Metric({ label, value, detail, accent }) { return <div className="metric"><div className="metric-label">{label}</div><div className={`metric-value ${accent ? 'accent' : ''}`}>{value}</div><div className="metric-detail">{detail}</div></div> }

function SortableHeader({ label, value, sort, order, onSort }) { return <th><button className={`sort-header ${sort === value ? 'sort-active' : ''}`} onClick={() => onSort(value)}>{label}<span>{sort === value ? (order === 'desc' ? '↓' : '↑') : '↕'}</span></button></th> }

function PoolRow({ pool, index, onOpen, watched, onToggle }) {
  const feeApr = pool.fee_apr ?? pool.effective_lp_fee_apr ?? pool.lp_fee_apr_estimate
  const sevenDayFeeApr = pool['7d_fee_apr']
  const aprPositive = Number(feeApr || 0) > 0.1
  return <tr className="pool-row" onClick={() => onOpen(pool)}>
    <td className="star-col"><button className={`star-button ${watched ? 'saved' : ''}`} aria-label={watched ? 'Remove from watchlist' : 'Add to watchlist'} aria-pressed={watched} onClick={(event) => { event.stopPropagation(); onToggle(pool) }}>{watched ? '★' : '☆'}</button></td>
    <td><div className="pool-name"><span className="pair-icon"><ChainDot chain={pool.chain}/></span><div><strong>{pool.pair_display || pool.pair}</strong><div className="pool-sub"><span>{pool.protocol}</span><span className="address">{shortAddress(pool.pool_address)}</span></div></div></div></td>
    <td className="numeric price-cell"><div className="pair-prices"><span>{tokenPrice(pool, 'a')}</span><span>{tokenPrice(pool, 'b')}</span></div></td>
    <td className="numeric avg-cell">{pairMetric(pool.token_a_avg_price_1d_usd, pool.token_b_avg_price_1d_usd, formatPrice, 'No 1d data')}</td>
    <td className="numeric avg-cell">{pairMetric(pool.token_a_avg_price_7d_usd, pool.token_b_avg_price_7d_usd, formatPrice, 'No 7d data')}</td>
    <td className="numeric avg-cell">{pairMetric(pool.token_a_daily_vol, pool.token_b_daily_vol, formatVolatility)}</td>
    <td className="numeric flow-cell">{formatMultiple(pool.volume_24h_to_tvl)}</td>
    <td><ActivityMeter ratio={pool.volume_24h_to_tvl}/></td>
    <td className="numeric metric-cell">{formatMoney(pool.liquidity_usd)}</td>
    <td className="numeric metric-cell">{formatMoney(pool.volume_24h_usd)}</td>
    <td className="numeric metric-cell">{formatMoney(pool.volume_7d_usd)}</td>
    <td className={`numeric apr-cell ${aprPositive ? 'positive' : ''}`}>{formatPercent(feeApr)}</td>
    <td className={`numeric apr-cell ${Number(sevenDayFeeApr || 0) > 0.1 ? 'positive' : ''}`}>{formatPercent(sevenDayFeeApr)}</td>
    <td className="numeric iv-cell" title={`7d realized pair volatility · ${poolVolatilitySource(pool)}`}>{formatVolatility(poolVolatility(pool))}</td>
    <td className="arrow-col"><Icon name="arrow" size={15}/></td>
  </tr>
}

function SkeletonRows() { return Array.from({ length: 8 }, (_, index) => <tr key={index} className="skeleton-row"><td colSpan="15"><span /></td></tr>) }

function PoolDrawer({ pool, watched, onToggle, onOpenExplorer, onClose }) {
  const hedgeAvailable = pool.option_status === 'complete'
  const fee = Number((pool.effective_fee_bps ?? pool.fee_bps) || 0)
  const feeApr = pool.fee_apr ?? pool.effective_lp_fee_apr ?? pool.lp_fee_apr_estimate
  const sevenDayFeeApr = pool['7d_fee_apr']
  const rangeLow = pool.range_low || Number(pool.price_usd || 0) * 0.8
  const rangeHigh = pool.range_high || Number(pool.price_usd || 0) * 1.2
  return <div className="drawer-backdrop" onClick={onClose}><aside className="drawer" onClick={(event) => event.stopPropagation()}>
    <div className="drawer-header"><div className="drawer-kicker"><ChainDot chain={pool.chain}/>{pool.chain} <span>·</span> {pool.protocol}</div><button className="icon-button" onClick={onClose}><Icon name="close"/></button></div>
    <div className="drawer-title"><div><h2>{pool.pair_display || pool.pair}</h2><span>{pool.pool_address}</span></div><button className="open-link"><Icon name="external" size={14}/></button></div>
    <div className="drawer-price"><div className="drawer-token-prices"><strong>{tokenPrice(pool, 'a')}</strong><strong>{tokenPrice(pool, 'b')}</strong></div><span className="flow-up">{formatMultiple(pool.volume_24h_to_tvl)} flow / TVL</span></div>
    <div className="activity-panel"><div className="activity-panel-head"><span>Snapshot activity</span><span>24h volume / liquidity</span></div><div className="activity-track"><span style={{ width: `${Math.max(5, Math.min(100, Math.log10(Math.max(1, Number(pool.volume_24h_to_tvl || 0))) * 42 + 12))}%` }}/></div><div className="activity-scale"><span>quiet</span><span>{formatMoney(pool.volume_24h_usd)} flow</span><span>active</span></div></div>
    <div className="drawer-stats"><Stat label="Liquidity" value={formatMoney(pool.liquidity_usd)}/><Stat label="Volume · 24h" value={formatMoney(pool.volume_24h_usd)}/><Stat label="Volume · 7d" value={formatMoney(pool.volume_7d_usd)}/><Stat label="A avg · 1d" value={formatPrice(pool.token_a_avg_price_1d_usd)}/><Stat label="B avg · 1d" value={formatPrice(pool.token_b_avg_price_1d_usd)}/><Stat label="A avg · 7d" value={formatPrice(pool.token_a_avg_price_7d_usd)}/><Stat label="B avg · 7d" value={formatPrice(pool.token_b_avg_price_7d_usd)}/><Stat label="A avg · 30d" value={formatPrice(pool.token_a_avg_price_usd)}/><Stat label="B avg · 30d" value={formatPrice(pool.token_b_avg_price_usd)}/><Stat label="A daily vol" value={formatVolatility(pool.token_a_daily_vol)}/><Stat label="B daily vol" value={formatVolatility(pool.token_b_daily_vol)}/><Stat label="Fee APR" value={formatPercent(feeApr)} highlight/><Stat label="7d Fee APR" value={formatPercent(sevenDayFeeApr)} highlight/><Stat label="Realized vol" value={formatVolatility(poolVolatility(pool))} highlight/><Stat label="Vol source" value={poolVolatilitySource(pool)}/><Stat label="Pool fee" value={fee ? `${fee.toFixed(2)} bps` : 'Not available'}/></div>
    <div className="drawer-section"><div className="section-title"><span>LP range</span><span className="muted">Modeled ±20%</span></div><div className="range-visual"><div className="range-line"><span className="range-active"/><i className="range-marker"/></div><div className="range-values"><span>{formatPrice(rangeLow)}</span><strong>{formatPrice(pool.price_usd)}</strong><span>{formatPrice(rangeHigh)}</span></div></div></div>
    <div className="drawer-section hedge-card"><div className="section-title"><span>Option hedge</span><span className={`status-pill ${hedgeAvailable ? 'green' : 'red'}`}>{hedgeAvailable ? 'Available' : 'No direct option'}</span></div><p>{hedgeAvailable ? `Deribit quote matched. Premium ${formatMoney(pool.option_premium_usd, false)} · replication error ${formatMoney(pool.replication_error_usd, false)}.` : 'No direct Deribit option for this asset, so no synthetic premium is shown.'}</p>{pool.net_pnl_lower_usd != null && <div className="net-range"><span>Modeled 7D net</span><strong>{formatMoney(pool.net_pnl_lower_usd, false)} — {formatMoney(pool.net_pnl_upper_usd, false)}</strong></div>}</div>
    <div className="drawer-footer"><button className={`secondary-button ${watched ? 'saved' : ''}`} onClick={() => onToggle(pool)}><Icon name="bookmark" size={14}/>{watched ? 'Saved' : 'Add to watchlist'}</button><button className="primary-button" disabled={!explorerUrl(pool)} onClick={onOpenExplorer}>{explorerUrl(pool) ? 'View on explorer' : 'No contract explorer'} <Icon name="external" size={14}/></button></div>
  </aside></div>
}

function Stat({ label, value, highlight }) { return <div className="stat"><span>{label}</span><strong className={highlight ? 'highlight' : ''}>{value}</strong></div> }

createRoot(document.getElementById('root')).render(<App />)
