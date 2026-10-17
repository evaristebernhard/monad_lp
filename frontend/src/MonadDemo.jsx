import { useEffect, useState } from 'react'
import MonadTrade from './MonadTrade'
import {subscribeJson} from './streamJson.js'

const number = value => value == null ? '—' : Number(value).toLocaleString('en-US', { maximumSignificantDigits: 8 })
const percent = value => value == null ? '—' : (Number(value) * 100).toFixed(3) + '%'
const time = value => value ? new Date(Number(value) * 1000).toLocaleTimeString() : '—'

export default function MonadDemo() {
  const [data, setData] = useState(null)
  const [error, setError] = useState('')
  const [priceError,setPriceError]=useState('')
  const [now, setNow] = useState(Date.now())
  useEffect(() => {
    let cancelled = false, pending = false
    const controller = new AbortController()
    const load = async () => {
      if (pending) return
      pending = true
      try {
        const response = await fetch('/api/monad/demo', { signal: controller.signal })
        const payload = await response.json()
        if (!response.ok) throw new Error(payload.error || '20 pool API unavailable')
        if (!cancelled) {
          setData(current => ({ ...payload, pools: (payload.pools || []).map(pool => {
            const live=current?.pools.find(p=>p.pool_address===pool.pool_address)?.price
            return live?.as_of>pool.price?.as_of ? {...pool,price:live} : pool
          }) })); setError('')
        }
      } catch (e) { if (!cancelled) setError(e.message) }
      finally { pending = false }
    }
    load()
    void subscribeJson('/api/monad/prices-stream',{signal:controller.signal,onValue:payload=>{
      if(payload.error)throw Error(payload.error)
      if(!cancelled)setPriceError('')
      if(!cancelled)setData(current=>current ? {...current,pools:current.pools.map(pool=>{
        const price=(payload.prices||[]).find(p=>p.pool===pool.pool_address)
        return price && Number(price.state_block||0)>=Number(pool.price?.state_block||0) && Number(price.as_of)>=Number(pool.price?.as_of||0) ? {...pool,price} : pool
      })} : current)
    },onError:e=>{if(!cancelled)setPriceError(e.message)}})
    const timer = setInterval(load, 5000)
    const clock = setInterval(() => setNow(Date.now()), 1000)
    window.addEventListener('monad-demo-refresh', load)
    return () => { cancelled = true; controller.abort(); clearInterval(timer); clearInterval(clock); window.removeEventListener('monad-demo-refresh', load) }
  }, [])
  const pools = data?.pools || []
  const fresh = pools.filter(p => p.price?.status === 'ok' && now / 1000 - Number(p.price.as_of) <= 30).length
  const cell = (metric, format = number) => <td className="numeric mono-cell" title={`${metric?.source || ''} · ${time(metric?.as_of)}`}>
    {format(metric?.value)}{metric?.status !== 'ok' && <small style={{ display: 'block', color: '#e5b86b' }}>{metric?.status || 'waiting'}</small>}
  </td>
  return <section className="solana-listener-page">
    <div className="page-heading"><div><h1>20 pool</h1><p>价格实时推送 · 成交量 / TVL 30s</p></div></div>
    <MonadTrade />
    {(error||priceError) && <div className="solana-error">{error||priceError}</div>}
    <div className="metric-strip"><div><strong>{pools.length}</strong><p>Fixed pools</p></div><div><strong>{fresh} / {pools.length}</strong><p>RPC prices within 30 seconds</p></div></div>
    <div style={{ overflowX: 'auto' }}><table className="solana-table"><thead><tr><th>Pool</th><th>Protocol</th><th>Price · right / left</th><th>Daily volatility</th><th>Fee APR</th><th>Volume · 24h USD</th><th>RPC observation</th></tr></thead>
      <tbody>{pools.map(pool => <tr key={pool.pool_address}>
        <td title={pool.pool_address}><strong>{pool.pair}</strong><small style={{ display: 'block' }}>{pool.pool_address.slice(0, 8)}…{pool.pool_address.slice(-6)}</small></td><td>{pool.protocol}</td>
        {cell(pool.price)}{cell(pool.vol, percent)}{cell(pool.fee_apr, percent)}{cell(pool.volume)}
        <td>{time(pool.price?.as_of)}<small style={{ display: 'block' }}>{pool.price?.as_of ? Math.max(0, Math.round(now / 1000 - Number(pool.price.as_of))) + 's ago' : 'waiting for RPC'}</small>{pool.price?.state_block&&<small style={{display:'block'}}>#{pool.price.state_block}</small>}</td>
      </tr>)}</tbody></table></div>
  </section>
}
