import { useCallback, useEffect, useMemo, useState, useRef } from 'react'

const TOKEN_KEY = 'lp-screener.monad-listener-token'
const INPUT_KEY = 'lp-screener.monad-listener-input'
const NAME_KEY = 'lp-screener.monad-listener-name'
const ADDRESS_KEY = 'lp-screener.monad-listener-address'
const DEFAULT_DECODED_POOL_TYPES = new Set(['constant_product_v2', 'solidly_v2', 'concentrated_v3', 'uniswap_v4'])

const request = (path, options) => fetch(path, options).then(async (response) => {
  const payload = await response.json().catch(() => ({}))
  if (!response.ok) throw new Error(payload.error || ('API ' + response.status))
  return payload
}).catch((error) => {
  if (error?.message === 'Failed to fetch') {
    throw new Error('Monad API unavailable; check lp-api and local-rpc')
  }
  throw error
})

const shortAddress = (value) => {
  if (typeof value !== 'string' || !value) return '—'
  return value.length > 16 ? value.slice(0, 7) + '…' + value.slice(-6) : value
}

const formatMoney = (value) => {
  const number = Number(value)
  if (value == null || !Number.isFinite(number)) return '—'
  if (Math.abs(number) >= 1e9) return '$' + (number / 1e9).toFixed(2) + 'B'
  if (Math.abs(number) >= 1e6) return '$' + (number / 1e6).toFixed(2) + 'M'
  if (Math.abs(number) >= 1e3) return '$' + (number / 1e3).toFixed(1) + 'K'
  return '$' + number.toLocaleString('en-US', { maximumSignificantDigits: 6 })
}

const formatRatio = (value) => {
  const number = Number(value)
  if (value == null || !Number.isFinite(number)) return '—'
  return number.toLocaleString('en-US', { maximumSignificantDigits: 8 })
}

const formatAmount = (value) => {
  const number = Number(value)
  if (value == null || !Number.isFinite(number)) return '—'
  return number.toLocaleString('en-US', { maximumSignificantDigits: 8 })
}

const positiveNumber = (value) => {
  const number = Number(value)
  return value != null && Number.isFinite(number) && number > 0 ? number : null
}

const explorerTx = (signature) => signature ? 'https://monadvision.com/tx/' + signature : null
const explorerPool = (address) => address?.length === 42 ? 'https://monadvision.com/address/' + address : null

const poolAssetLabel = (pool, mint) => {
  if (!pool || !mint) return shortAddress(mint)
  if (pool.token_a_address?.toLowerCase() === mint.toLowerCase()) return pool.token_a_symbol || shortAddress(mint)
  if (pool.token_b_address?.toLowerCase() === mint.toLowerCase()) return pool.token_b_symbol || shortAddress(mint)
  return shortAddress(mint)
}

const formatTime = (value) => {
  if (!value) return '—'
  const date = new Date(Number(value) > 1e11 ? Number(value) : Number(value) * 1000)
  return Number.isNaN(date.getTime()) ? '—' : date.toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

const formatSourceTime = (value) => {
  if (value == null) return '—'
  const number = Number(value)
  const date = Number.isFinite(number) ? new Date(number > 1e11 ? number : number * 1000) : new Date(value)
  return Number.isNaN(date.getTime()) ? '—' : date.toLocaleString()
}
const percent = (value) => value == null ? '—' : (Number(value) * 100).toFixed(2) + '%'
const metricStatus = (metric) => metric?.status === 'stale' ? 'Stale source' : metric?.status === 'requires_24_consecutive_hourly_pool_prices' ? 'Needs 24 consecutive hourly samples' : metric?.value == null ? 'Data unavailable' : formatSourceTime(metric.as_of)

const formatAge = (value) => {
  if (!value) return '—'
  const milliseconds = Date.now() - (Number(value) > 1e11 ? Number(value) : Number(value) * 1000)
  const seconds = Math.max(0, Math.floor(milliseconds / 1000))
  if (seconds < 60) return seconds + 's ago'
  if (seconds < 3600) return Math.floor(seconds / 60) + 'm ago'
  return Math.floor(seconds / 3600) + 'h ago'
}

const normalizeProviderCandles = (payload) => (Array.isArray(payload?.data) ? payload.data : [])
  .map((candle) => ({
    ...candle,
    timestamp: (() => {
      const value = Number(candle.timestamp ?? candle.time)
      return value > 1e11 ? value : value * 1000
    })(),
    open: Number(candle.open),
    high: Number(candle.high),
    low: Number(candle.low),
    close: Number(candle.close),
    volume_usd: Number(candle.volume_usd ?? candle.volumeUsd ?? 0),
  }))
  .filter((candle) => Number.isFinite(candle.timestamp) && Number.isFinite(candle.close))
  .sort((a, b) => Number(a.timestamp) - Number(b.timestamp))

const candleIntervalMs = (resolution) => ({ '1m': 60_000, '5m': 300_000, '15m': 900_000, '1h': 3_600_000 }[resolution] || 60_000)

const aggregateEventCandles = (events = [], resolution = '5m') => {
  const interval = candleIntervalMs(resolution)
  const candles = new Map()
  const ordered = events.map((event) => {
    const trade = tradeOf(event)
    const rawTime = Number(event.observed_at_ms)
    return {
      trade,
      time: rawTime > 1e11 ? rawTime : rawTime * 1000,
      price: positiveNumber(trade.swap_price_usd),
      volume: Number(trade.notional_usd) || 0,
    }
  }).filter((row) => row.price && Number.isFinite(row.time)).sort((a, b) => a.time - b.time)
  for (const row of ordered) {
    const timestamp = Math.floor(row.time / interval) * interval
    const candle = candles.get(timestamp)
    if (candle) {
      candle.high = Math.max(candle.high, row.price)
      candle.low = Math.min(candle.low, row.price)
      candle.close = row.price
      candle.volume_usd += row.volume
      candle.trades += 1
    } else {
      candles.set(timestamp, {
        timestamp, open: row.price, high: row.price, low: row.price, close: row.price,
        volume_usd: row.volume, trades: 1,
      })
    }
  }
  return [...candles.values()]
}

const combineCandles = (providerCandles = [], localCandles = [], resolution = '1m') => {
  const interval = candleIntervalMs(resolution)
  const byTime = new Map()
  for (const candle of localCandles) {
    const timestamp = Math.floor(Number(candle.timestamp) / interval) * interval
    byTime.set(timestamp, { ...candle, timestamp })
  }
  for (const candle of providerCandles) {
    const timestamp = Math.floor(Number(candle.timestamp) / interval) * interval
    const local = byTime.get(timestamp)
    byTime.set(timestamp, {
      ...local,
      ...candle,
      timestamp,
      volume_usd: Number(candle.volume_usd) || Number(local?.volume_usd) || 0,
    })
  }
  return [...byTime.values()].sort((a, b) => a.timestamp - b.timestamp)
}

const tradeOf = (event) => event?.enrichment?.transaction?.trade || {}

const snapshotPrice = (pool, snapshot, targetMint) => {
  const decoded = snapshot?.snapshot?.decoded_price
  if (!decoded) return null
  const target = targetMint?.toLowerCase()
  const validPrice = positiveNumber
  if (target && decoded.token0_address?.toLowerCase() === target) return validPrice(decoded.token0_price_usd)
  if (target && decoded.token1_address?.toLowerCase() === target) return validPrice(decoded.token1_price_usd)
  if (target) return null
  const tokenAStable = /USD/i.test(pool?.token_a_symbol || '')
  const tokenBStable = /USD/i.test(pool?.token_b_symbol || '')
  const value = tokenAStable
    ? decoded.token1_price_usd
    : tokenBStable
      ? decoded.token0_price_usd
      : decoded.token0_price_usd ?? decoded.token1_price_usd
  return Number.isFinite(Number(value)) ? Number(value) : null
}

function Metric({ label, value, detail, accent = false }) {
  return <div className="solana-metric">
    <span>{label}</span>
    <strong className={accent ? 'accent' : ''}>{value}</strong>
    {detail ? <small>{detail}</small> : null}
  </div>
}

function StatusMark({ state = 'ok' }) {
  return <span className={'solana-status-mark ' + state} aria-hidden="true" />
}

function PoolRow({ pool, snapshot, trade, selected, onSelect, targetMint }) {
  const explorer = explorerPool(pool.pool_address)
  const updatedAt = snapshot?.refreshed_at_ms
  const rpcPrice = snapshotPrice(pool, snapshot, targetMint)
  const executionPrice = positiveNumber(trade?.swap_price_usd)
  const price = rpcPrice ?? executionPrice
  const priceSource = rpcPrice != null ? 'Current RPC pool price' : executionPrice != null ? 'Latest decoded swap execution price' : 'Waiting for a target-token USD price from RPC'
  return <tr>
    <td>
      <button type="button" className={'solana-pool-select ' + (selected ? 'selected' : '')} onClick={() => onSelect(pool)}>
        <strong>{pool.pair || (pool.token_a_symbol || 'Token A') + ' / ' + (pool.token_b_symbol || 'Token B')}</strong>
        <span>{shortAddress(pool.pool_address)} · rank {pool.protocol_rank || '—'}<br/>Catalog: {formatSourceTime(pool.updated_at)}<br/>Last trade: {formatTime(trade?.as_of)}</span>
      </button>
    </td>
    <td className="mono-cell">{pool.pool_type || '—'}</td>
    <td className="numeric mono-cell">{formatMoney(pool.monad_tvl?.value ?? pool.liquidity_usd)}</td>
    <td className="numeric mono-cell">{formatMoney(pool.monad_volume?.value ?? pool.volume_24h_usd)}</td>
    <td className="numeric mono-cell" title={priceSource}>{formatMoney(price)}{executionPrice == null && rpcPrice != null ? <small className="solana-source-mark">RPC</small> : null}</td>
    <td className="numeric mono-cell" title={pool.monad_price?.status || "Current quote amount per target token"}>{formatRatio(pool.monad_price?.value)}</td>
    <td className="numeric mono-cell" title={pool.monad_vol?.status}>{pool.monad_vol?.value != null ? (pool.monad_vol.value * 100).toFixed(2) + '%' : '—'}</td>
    <td className="numeric mono-cell" title={pool.monad_fee_apr?.status}>{pool.monad_fee_apr?.value != null ? (pool.monad_fee_apr.value * 100).toFixed(2) + '%' : '—'}</td>
    <td className="numeric mono-cell" title={snapshot ? 'Latest listener snapshot' : 'No listener snapshot'}>{updatedAt ? formatAge(updatedAt) : '—'}</td>
    <td className="solana-row-action">{explorer && <a href={explorer} target="_blank" rel="noreferrer">Open ↗</a>}</td>
  </tr>
}

function ProtocolGroup({ protocol, pools, snapshots = new Map(), trades = new Map(), selectedPool, onSelect, targetMint }) {
  const liquidity = pools.reduce((sum, pool) => sum + (Number(pool.liquidity_usd) || 0), 0)
  return <section className="solana-protocol">
    <div className="solana-protocol-head">
      <div>
        <span className="solana-protocol-dot" />
        <strong>{protocol || 'Unknown protocol'}</strong>
        <span className="solana-protocol-count">{pools.length} subscribed</span>
      </div>
      <span className="mono-cell">TVL {formatMoney(liquidity)}</span>
    </div>
    <div className="solana-table-wrap">
      <table className="solana-table">
        <thead><tr><th>Pool</th><th>Type</th><th className="numeric">Liquidity</th><th className="numeric">Volume · 24h</th><th className="numeric">Last / state USD</th><th className="numeric">Quote / target</th><th className="numeric">Daily vol</th><th className="numeric">Fee APR</th><th className="numeric">State time</th><th /></tr></thead>
        <tbody>{pools.map((pool) => <PoolRow key={pool.pool_address} pool={pool} snapshot={snapshots.get(pool.pool_address?.toLowerCase())} trade={trades.get(pool.pool_address?.toLowerCase())} selected={selectedPool?.pool_address?.toLowerCase() === pool.pool_address?.toLowerCase()} onSelect={onSelect} targetMint={targetMint} />)}</tbody>
      </table>
    </div>
  </section>
}

function PriceChart({ candles = [], resolution = '1m', volumeUnavailable = false, volumeIsLocal = false, priceUnit = 'quote_per_target' }) {
  const [hoverIndex, setHoverIndex] = useState(null)
  const points = candles.filter((c) => Number.isFinite(Number(c.close)))
  if (!points.length) return <div className="solana-chart-empty">No price history</div>
  const width = 760
  const height = 260
  const plotLeft = 76
  const plotRight = 748
  const plotTop = 14
  const plotBottom = 184
  const volumeTop = 207
  const volumeHeight = 31
  const values = points.flatMap((point) => [Number(point.open), Number(point.high), Number(point.low), Number(point.close)]).filter(Number.isFinite)
  const dataMin = Math.min(...values)
  const dataMax = Math.max(...values)
  const spread = dataMax - dataMin
  const padValue = spread ? spread * .08 : Math.max(Math.abs(dataMax) * .01, 1e-9)
  const min = Math.max(0, dataMin - padValue)
  const max = dataMax + padValue
  const span = max - min || 1
  const maxVolume = Math.max(...points.map((point) => Number(point.volume_usd) || 0), 1)
  const firstTime = Number(points[0].timestamp) || 0
  const lastTime = Number(points[points.length - 1].timestamp) || firstTime
  const timeSpan = lastTime - firstTime
  const xAt = (point) => points.length === 1 || !timeSpan
    ? (plotLeft + plotRight) / 2
    : plotLeft + ((Number(point.timestamp) - firstTime) / timeSpan) * (plotRight - plotLeft)
  const yAt = (value) => plotBottom - ((Number(value) - min) / span) * (plotBottom - plotTop)
  const priceTicks = Array.from({ length: 5 }, (_, index) => max - (span * index) / 4)
  const candleData = points.map((point, index) => {
    const close = Number(point.close)
    const sourceOpen = Number(point.open)
    const sourceHigh = Number(point.high)
    const sourceLow = Number(point.low)
    const open = Number.isFinite(sourceOpen) ? sourceOpen : close
    const high = Number.isFinite(sourceHigh) ? sourceHigh : Math.max(open, close)
    const low = Number.isFinite(sourceLow) ? sourceLow : Math.min(open, close)
    const x = xAt(point)
    const volume = Number(point.volume_usd) || 0
    const candleWidth = Math.max(2, Math.min(12, (plotRight - plotLeft) / Math.max(points.length, 2) * .62))
    return {
      x, open, close, high, low, volume, candleWidth, timestamp: point.timestamp,
      top: Math.min(yAt(open), yAt(close)),
      bodyHeight: Math.max(2, Math.abs(yAt(open) - yAt(close))),
    }
  })
  const dataGaps = points.slice(1).map((point, index) => {
    const elapsed = Number(point.timestamp) - Number(points[index].timestamp)
    if (elapsed < Math.max(30 * 60_000, candleIntervalMs(resolution) * 3)) return null
    const left = candleData[index].x + candleData[index].candleWidth / 2
    const right = candleData[index + 1].x - candleData[index + 1].candleWidth / 2
    if (right - left < 30) return null
    const minutes = Math.round(elapsed / 60_000)
    const label = minutes >= 60
      ? Math.floor(minutes / 60) + 'h' + (minutes % 60 ? ' ' + (minutes % 60) + 'm' : '') + ' gap'
      : minutes + 'm gap'
    return { left, right, label }
  }).filter(Boolean)
  const latest = candleData[candleData.length - 1]
  const active = candleData[hoverIndex ?? candleData.length - 1] || latest
  const timeLabel = (timestamp) => timestamp
    ? new Date(Number(timestamp)).toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit' })
    : '—'
  const totalVolume = points.reduce((sum, point) => sum + (Number(point.volume_usd) || 0), 0)
  const volumeLabel = (value) => volumeUnavailable && !(Number(value) > 0) ? '—' : formatMoney(value)
  return <div className="solana-chart-wrap">
    <div className="solana-chart-reading">
      <span className="solana-chart-last">LAST <strong>{formatRatio(latest.close)}</strong></span>
      <span>{timeLabel(active.timestamp)}</span>
      <span>O <strong>{formatRatio(active.open)}</strong></span>
      <span>H <strong>{formatRatio(active.high)}</strong></span>
      <span>L <strong>{formatRatio(active.low)}</strong></span>
      <span>C <strong>{formatRatio(active.close)}</strong></span>
      <span>VOL <strong>{volumeLabel(active.volume)}</strong></span>
      <span className="solana-chart-total">Σ {volumeLabel(totalVolume)}</span>
    </div>
    <svg className="solana-chart" viewBox={'0 0 ' + width + ' ' + height} role="img" aria-label="Pool price candlesticks with OHLC and volume">
      {priceTicks.map((tick, index) => {
        const y = yAt(tick)
        return <g key={index}>
          <line x1={plotLeft} x2={plotRight} y1={y} y2={y} stroke="#26353d" strokeWidth="1" />
          <text x={plotLeft - 8} y={y + 3} textAnchor="end" className="solana-chart-axis">{formatRatio(tick)}</text>
        </g>
      })}
      {dataGaps.map((gap) => <g key={gap.left} pointerEvents="none">
        <rect x={gap.left} y={plotTop} width={gap.right - gap.left} height={plotBottom - plotTop} fill="rgba(222, 175, 94, .035)" stroke="rgba(222, 175, 94, .18)" strokeDasharray="3 5" />
        <text x={(gap.left + gap.right) / 2} y={(plotTop + plotBottom) / 2} textAnchor="middle" className="solana-chart-gap-label">No data · {gap.label}</text>
      </g>)}
      <line x1={plotLeft} x2={plotRight} y1={volumeTop - 8} y2={volumeTop - 8} stroke="#34434b" strokeWidth="1" />
      {candleData.map((candle, index) => <g key={points[index].timestamp || index} onMouseEnter={() => setHoverIndex(index)} onMouseLeave={() => setHoverIndex(null)}>
        {index === (hoverIndex ?? candleData.length - 1) && <line x1={candle.x} x2={candle.x} y1={plotTop} y2={volumeTop + volumeHeight} stroke="#8ba4a8" strokeDasharray="3 4" opacity=".55" />}
        <line x1={candle.x} x2={candle.x} y1={yAt(candle.high)} y2={yAt(candle.low)} stroke={candle.close >= candle.open ? '#b8f05a' : '#ff827b'} strokeWidth="1.2" vectorEffect="non-scaling-stroke" />
        <rect x={candle.x - candle.candleWidth / 2} y={candle.top} width={candle.candleWidth} height={candle.bodyHeight} fill={candle.close >= candle.open ? '#b8f05a' : '#ff827b'} opacity=".9" />
        <rect x={candle.x - candle.candleWidth / 2} y={volumeTop + volumeHeight - (candle.volume / maxVolume) * volumeHeight} width={candle.candleWidth} height={Math.max(1, (candle.volume / maxVolume) * volumeHeight)} fill={candle.close >= candle.open ? '#b8f05a' : '#ff827b'} opacity=".3" />
        <title>{timeLabel(candle.timestamp)} · O {formatRatio(candle.open)} · H {formatRatio(candle.high)} · L {formatRatio(candle.low)} · C {formatRatio(candle.close)} · VOL {volumeLabel(candle.volume)}</title>
      </g>)}
      {[0, Math.floor((candleData.length - 1) / 2), candleData.length - 1].filter((index, i, list) => list.indexOf(index) === i).map((index) => <text key={index} x={candleData[index].x} y={height - 3} textAnchor={index === 0 ? 'start' : index === candleData.length - 1 ? 'end' : 'middle'} className="solana-chart-axis">{timeLabel(candleData[index].timestamp)}</text>)}
    </svg>
    <div className="solana-chart-scale"><span>{points.length} candles</span><span>{priceUnit === 'USD' ? 'Price · USD' : 'Price · quote / target'}</span><span>{volumeUnavailable ? 'Volume unavailable' : volumeIsLocal ? 'Volume · USD · local' : 'Volume · USD'}</span></div>
  </div>
}

function PoolDetail({ pool, history, providerCandles, marketMeta = {}, poolEvents = [], candleError = '', providerLoading, loading, poolByAddress, snapshot, targetMint, candleResolution, onCandleResolutionChange }) {
  const [showChart, setShowChart] = useState(true)
  if (!pool) return <div className="solana-detail-empty"><strong>Select a pool</strong></div>
  const summary = history?.summary || {}
  const targetEvents = poolEvents.filter((event) => {
    const eventTarget = tradeOf(event).target_mint
    return !eventTarget || !targetMint || eventTarget.toLowerCase() === targetMint.toLowerCase()
  })
  const eventTrades = targetEvents.map((event) => ({
    ...tradeOf(event),
    signature: event.signature,
    observed_at_ms: event.observed_at_ms,
  })).filter((trade) => trade.side === 'buy' || trade.side === 'sell')
    .sort((a, b) => Number(b.observed_at_ms) - Number(a.observed_at_ms))
  const latest = history?.latest_trade?.trade || history?.latest_trade || eventTrades[0] || {}
  const rpcPrice = snapshotPrice(pool, snapshot, targetMint)
  const marketChart = providerCandles.length > 0
  // External candles are USD; local candles are quote per target. Never merge them.
  const candles = marketChart ? providerCandles : (history?.candles || [])
  const latestCandle = candles[candles.length - 1]
  const currentPrice = positiveNumber(latest.swap_price_usd) ?? positiveNumber(latestCandle?.close) ?? rpcPrice
  const firstCandle = candles[0]
  const candleChange = positiveNumber(firstCandle?.close) && positiveNumber(latestCandle?.close)
    ? ((Number(latestCandle.close) - Number(firstCandle.close)) / Number(firstCandle.close)) * 100
    : null
  const chartVolume = candles.reduce((sum, candle) => sum + (Number(candle.volume_usd) || 0), 0)
  const eventVolume = eventTrades.reduce((sum, trade) => sum + (Number(trade.notional_usd) || 0), 0)
  const volumeValue = Number(summary.volume_usd) || eventVolume
  const volumeUnavailable = Boolean((candleError && !volumeValue) || (summary.trades > 0 && summary.unknown_usd_trades === summary.trades))
  const historyTrades = (history?.trades || []).slice().reverse()
  const trades = (historyTrades.length ? historyTrades : eventTrades).slice(0, 8)
  return <section className="solana-detail-panel">
    <div className="solana-detail-head">
      <div><span>{pool.protocol || 'Monad'}</span><h3>{pool.pair || pool.pool_address}</h3><small>{pool.pool_type === 'uniswap_v4' ? 'Pool ID ' : ''}{shortAddress(pool.pool_address)}{loading ? ' · loading' : ''}</small></div>
      {explorerPool(pool.pool_address) && <a className="solana-detail-link" href={explorerPool(pool.pool_address)} target="_blank" rel="noreferrer">Open pool ↗</a>}
    </div>
    <div className="solana-detail-metrics">
      <Metric label="Price ratio" value={formatRatio(pool.monad_price?.value)} detail={'RPC · ' + metricStatus(pool.monad_price)} accent />
      <Metric label="Chart change" value={summary.change_pct != null ? (summary.change_pct >= 0 ? '+' : '') + Number(summary.change_pct).toFixed(2) + '%' : candleChange != null ? (candleChange >= 0 ? '+' : '') + candleChange.toFixed(2) + '%' : '—'} />
      <Metric label="Observed volume" value={volumeUnavailable ? '—' : formatMoney(volumeValue)} detail={volumeUnavailable ? 'USD valuation unavailable' : candleError ? 'Local trades only' : summary.trades ? summary.trades + ' observed trades' + (summary.unknown_usd_trades ? ' · ' + summary.unknown_usd_trades + ' unvalued' : '') : ''} />
      <Metric label="Hourly history" value={history?.hourly_prices?.length ?? 0} detail="Separate source: hourly pool prices" />
    </div>
    <div className="solana-detail-metrics">
      <Metric label="Daily volatility" value={percent(pool.monad_vol?.value)} detail={metricStatus(pool.monad_vol)} />
      <Metric label="Fee APR" value={percent(pool.monad_fee_apr?.value)} detail={metricStatus(pool.monad_fee_apr)} />
      <Metric label="TVL" value={formatMoney(pool.monad_tvl?.value ?? pool.liquidity_usd)} detail={pool.monad_tvl ? metricStatus(pool.monad_tvl) : 'Catalog · ' + formatSourceTime(pool.updated_at)} />
      <Metric label="Volume · 24h" value={formatMoney(pool.monad_volume?.value ?? pool.volume_24h_usd)} detail={pool.monad_volume ? (pool.monad_volume.source === 'geckoterminal_pool_snapshot' ? 'GeckoTerminal · ' : 'Catalog · ') + metricStatus(pool.monad_volume) : formatSourceTime(pool.updated_at)} />
    </div>
    <div className={'solana-detail-grid ' + (trades.length ? 'with-trades' : 'no-trades')}>
      <div className="solana-detail-chart"><div className="solana-detail-label">{marketChart ? 'Price · USD · GeckoTerminal' + (marketMeta.stale ? ' · cached' : '') : 'Quote / target · local trades'} {candleError && <span className="solana-chart-source" title={candleError}>{volumeValue ? 'Local data only' : 'Provider unavailable'}</span>}<span className="solana-chart-loading">{providerLoading ? 'loading' : ''}</span><div className="solana-chart-controls" role="group" aria-label="Candle interval">{[['1m', '1m'], ['5m', '5m'], ['15m', '15m'], ['1h', '1h']].map(([label, value]) => <button key={value} type="button" className={candleResolution === value ? 'active' : ''} aria-pressed={candleResolution === value} onClick={() => onCandleResolutionChange(value)}>{label}</button>)}<button type="button" className="solana-chart-toggle" onClick={() => setShowChart((visible) => !visible)}>{showChart ? 'Hide' : 'Show'}</button></div></div>{showChart ? <PriceChart candles={candles} resolution={candleResolution} volumeUnavailable={marketChart ? false : volumeUnavailable} volumeIsLocal={!marketChart} priceUnit={marketChart ? 'USD' : 'quote_per_target'} /> : null}</div>
      {trades.length > 0 && <div className="solana-detail-trades"><div className="solana-detail-label">Recent trades</div>
        {trades.map((trade) => <div className="solana-trade-line" key={trade.signature + '-' + (trade.log_index || '') + '-' + trade.observed_at_ms}>
          <span className={'solana-side ' + trade.side}>{trade.side}</span>
          <div><strong>{formatAmount(trade.target_amount_ui)} {poolAssetLabel(pool, trade.target_mint)}</strong><small>{trade.side === 'buy' ? 'bought for' : 'sold for'} {formatAmount(trade.quote_amount_ui)} {poolAssetLabel(pool, trade.quote_mint)} · {formatMoney(trade.notional_usd)} · {formatAge(trade.observed_at_ms)}</small></div>
          {explorerTx(trade.signature) && <a href={explorerTx(trade.signature)} target="_blank" rel="noreferrer">tx ↗</a>}
      </div>)}
      </div>}
    </div>
  </section>
}

function AlertRow({ event, pool }) {
  const trade = tradeOf(event)
  const side = trade.side === 'sell' ? 'sell' : trade.side === 'buy' ? 'buy' : 'unknown'
  const kind = event.enrichment?.transaction?.event_kind
  const headline = side === 'unknown' ? (kind === 'remove_liquidity' ? 'Whale LP removal' : 'Whale LP add') : 'Whale ' + side
  const verb = side === 'sell' ? 'sold' : 'bought'
  return <article className="solana-alert-row">
    <div className="solana-alert-icon">↗</div>
    <div className="solana-alert-copy">
      <div><strong>{headline}</strong><span>{formatAge(event.observed_at_ms)}</span></div>
      <p>{side === 'unknown' ? 'Liquidity change · ' + formatMoney(trade.notional_usd) : verb + ' ' + formatAmount(trade.target_amount_ui) + ' ' + poolAssetLabel(pool, trade.target_mint) + ' for ' + formatMoney(trade.notional_usd)}</p>
      <small>{event.watched_protocol || 'Monad'} · {shortAddress(event.watched_pool)} · price {formatMoney(trade.swap_price_usd)} · {shortAddress(trade.trader)}</small>
    </div>
    <a href={'https://monadvision.com/tx/' + event.signature} target="_blank" rel="noreferrer" aria-label="Open transaction">↗</a>
  </article>
}

function EventRow({ event, pool, snapshot }) {
  const trade = tradeOf(event)
  const transaction = event?.enrichment?.transaction || {}
  const isDecoding = event?.enrichment?.pending === true
  const eventKind = transaction.event_kind
  const side = trade.side === 'sell' ? 'sell' : trade.side === 'buy' ? 'buy' : 'unknown'
  const labels = {
    add_liquidity: ['LP add', 'Liquidity added; amounts unavailable'],
    remove_liquidity: ['LP removal', 'Liquidity removed; amounts unavailable'],
    pool_state_change: ['Pool balance change', 'Pool token accounts changed; no user swap balance pair was found'],
    failed_pool_program: ['Failed pool / LP call', 'Transaction reverted before token movement was committed'],
    pool_program_event: ['Pool event', 'No token balance delta; the program instruction was not identified'],
    pool_maintenance: ['Pool maintenance', 'Protocol maintenance instruction detected; no user token transfer'],
    protocol_event_unresolved: ['Protocol event not decoded', 'Known pool program matched, but its instruction name was not decoded'],
    failed_program_event: ['Failed protocol event', 'Known pool program reverted before token balances settled'],
    swap: event?.err
      ? ['Failed swap', 'Swap transaction failed or rolled back before balances settled']
      : transaction.protocol_instruction
        ? ['Swap decoded · amount unresolved', 'The protocol instruction was decoded, but no single owner had an opposite token/quote balance pair']
        : ['Swap amount unresolved', 'Swap instruction found, but no settled owner balance pair was decoded'],
  }
  const [unknownLabel, unknownDetail] = labels[eventKind] || ['Pool event', trade.reason || 'No settled owner balance pair was decoded']
  const instructionDetail = transaction.protocol_instruction ? ' · instruction ' + transaction.protocol_instruction : ''
  const liquidity = transaction.liquidity
  const protocolDetails = transaction.protocol_details || {}
  const sdkLiquidityAmounts = ['add_liquidity', 'remove_liquidity'].includes(eventKind)
    && Array.isArray(protocolDetails.amounts)
    && pool?.token_a_address
    && pool?.token_b_address
    ? protocolDetails.amounts
      .map((raw, index) => {
        const mint = index === 0 ? pool.token_a_address : pool.token_b_address
        const decimals = transaction.token_changes?.find((change) => change.mint?.toLowerCase() === mint.toLowerCase())?.decimals
        const amount = Number(raw) / (10 ** Number(decimals))
        return Number.isFinite(amount) && amount > 0 ? { mint, amount } : null
      })
      .filter(Boolean)
    : []
  const sdkLiquidityDetail = sdkLiquidityAmounts.length
    ? (eventKind === 'remove_liquidity' ? 'Withdrew ' : 'Deposited ')
      + sdkLiquidityAmounts.map(({ mint, amount }) => formatAmount(amount) + ' ' + poolAssetLabel(pool, mint)).join(' + ')
    : null
  const liquidityUsdAmounts = liquidity?.detected
    ? [
      { mint: liquidity.target_mint, amount: liquidity.target_amount_ui },
      ...(liquidity.quote_mint && liquidity.quote_amount_ui != null
        ? [{ mint: liquidity.quote_mint, amount: liquidity.quote_amount_ui }]
        : []),
    ]
    : sdkLiquidityAmounts
  const sdkLiquidityUsdParts = liquidityUsdAmounts.map(({ mint, amount }) => {
    const price = snapshotPrice(pool, snapshot, mint)
    return price != null ? amount * price : null
  })
  const sdkLiquidityUsd = liquidityUsdAmounts.length && sdkLiquidityUsdParts.every((value) => value != null)
    ? sdkLiquidityUsdParts.reduce((sum, value) => sum + value, 0)
    : null
  const liquidityDetail = liquidity?.detected && liquidity.target_amount_ui != null && (eventKind === 'add_liquidity' || eventKind === 'remove_liquidity')
    ? (liquidity.direction === 'deposit' ? 'Deposited ' : 'Withdrew ') + formatAmount(liquidity.target_amount_ui) + ' ' + poolAssetLabel(pool, liquidity.target_mint) + (liquidity.quote_amount_ui != null ? ' + ' + formatAmount(liquidity.quote_amount_ui) + ' ' + poolAssetLabel(pool, liquidity.quote_mint) : '')
    : null
  const shownLiquidityDetail = liquidityDetail || sdkLiquidityDetail
  const liquidityHeadline = liquidity?.detected && liquidity.target_amount_ui != null && (eventKind === 'add_liquidity' || eventKind === 'remove_liquidity')
    ? (liquidity.direction === 'deposit' ? 'LP add · ' : 'LP removal · ') + (sdkLiquidityUsd != null ? formatMoney(sdkLiquidityUsd) : formatAmount(liquidity.target_amount_ui) + ' ' + poolAssetLabel(pool, liquidity.target_mint))
    : null
  const sdkLiquidityHeadline = sdkLiquidityDetail
    ? (eventKind === 'remove_liquidity' ? 'LP removal' : 'LP add') + (sdkLiquidityUsd != null ? ' · ' + formatMoney(sdkLiquidityUsd) : '')
    : null
  // Incomplete protocol matches are useful in backend diagnostics, but they
  // read as product failures in the activity feed. Only show a swap once both
  // sides are attributed, or an LP action once token amounts were decoded.
  if (!isDecoding && side === 'unknown' && !shownLiquidityDetail && !transaction.liquidity?.detected) return null
  const decodedDetail = isDecoding
    ? 'Fetching transaction and decoding…'
    : shownLiquidityDetail
    ? shownLiquidityDetail + (sdkLiquidityUsd != null ? ' · ≈ ' + formatMoney(sdkLiquidityUsd) : '')
    : ['add_liquidity', 'remove_liquidity'].includes(eventKind)
      ? unknownDetail
      : transaction.protocol_explanation || unknownDetail
  const rpcPrice = snapshotPrice(pool, snapshot, trade.target_mint)
  const executionPrice = positiveNumber(trade.swap_price_usd)
  const currentPrice = executionPrice ?? rpcPrice
  const priceDetail = Number.isFinite(currentPrice) && currentPrice > 0
    ? ' · ' + (executionPrice != null ? 'price ' : 'current price ') + formatMoney(currentPrice)
    : ''
  const poolLabel = pool?.pair || shortAddress(event.watched_pool)
  const protocolLabel = event.watched_protocol || pool?.protocol || 'Monad'
  return <article className="solana-event-row">
    <span className={'solana-side ' + side}>{side === 'unknown' ? '—' : side}</span>
    <div>
      <strong>{isDecoding ? 'Decoding transaction…' : trade.side === 'unknown' ? (liquidityHeadline || sdkLiquidityHeadline || unknownLabel) : (trade.action || trade.side) + ' ' + formatAmount(trade.target_amount_ui) + ' ' + poolAssetLabel(pool, trade.target_mint)}</strong>
      <span>{trade.side === 'unknown' ? (transaction.liquidity?.delta_raw != null ? 'Liquidity delta ' + transaction.liquidity.delta_raw : decodedDetail) : 'for ' + formatAmount(trade.quote_amount_ui) + ' ' + poolAssetLabel(pool, trade.quote_mint) + ' · ' + formatMoney(trade.notional_usd)}</span>
      <small className="solana-event-meta">{event.whale_status === 'unknown_usd_value' ? 'USD valuation unavailable · ' : ''}{protocolLabel} · {poolLabel}{instructionDetail}{priceDetail} · {formatTime(event.observed_at_ms)}</small>
    </div>
    {explorerTx(event.signature) ? <a className="solana-event-link" href={explorerTx(event.signature)} target="_blank" rel="noreferrer">tx ↗</a> : <span className="mono-cell">{shortAddress(event.signature)}</span>}
  </article>
}

export default function MonadListener() {
  const [token, setToken] = useState(() => window.localStorage.getItem(TOKEN_KEY) || '')
  const [draftToken, setDraftToken] = useState(() => window.localStorage.getItem(INPUT_KEY) || window.localStorage.getItem(NAME_KEY) || window.localStorage.getItem(TOKEN_KEY) || window.localStorage.getItem(ADDRESS_KEY) || '')
  const [selectionRevision, setSelectionRevision] = useState(0)
  const [tokenReady, setTokenReady] = useState(() => !window.localStorage.getItem(TOKEN_KEY))
  const [catalog, setCatalog] = useState(null)
  const [alerts, setAlerts] = useState([])
  const [events, setEvents] = useState([])
  const [snapshots, setSnapshots] = useState([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [listenerError, setListenerError] = useState('')
  const [listenerStatus, setListenerStatus] = useState(null)
  const [resolveInfo, setResolveInfo] = useState(null)
  const [resolving, setResolving] = useState(false)
  const [lastRefresh, setLastRefresh] = useState(null)
  const [selectedPool, setSelectedPool] = useState(null)
  const [poolHistory, setPoolHistory] = useState(null)
  const [poolHistoryLoading, setPoolHistoryLoading] = useState(false)
  const [poolMarketLoading, setPoolMarketLoading] = useState(false)
  const [poolCandles, setPoolCandles] = useState([])
  const [poolMarketMeta, setPoolMarketMeta] = useState({})
  const [poolCandleError, setPoolCandleError] = useState('')
  const [candleResolution, setCandleResolution] = useState('1m')
  const queryGeneration = useRef(0)
  const historyKey = useRef('')
  const activeToken = useRef(token)
  activeToken.current = token
  const [historyRefresh, setHistoryRefresh] = useState(0)
  useEffect(() => { const t = setInterval(() => setHistoryRefresh(v => v + 1), 5000); return () => clearInterval(t) }, [])
  const [decodedPoolTypes, setDecodedPoolTypes] = useState(DEFAULT_DECODED_POOL_TYPES)
  useEffect(() => { if (!window.localStorage.getItem(TOKEN_KEY)) request('/api/monad/display').then(v => { if (v.token && !activeToken.current) { setToken(v.token); setDraftToken(current => current || v.token) } }).catch(() => {}) }, [])

  useEffect(() => {
    const input = token.trim()
    if (!input) {
      setTokenReady(true)
      return undefined
    }
    // Interactive queries already resolve to an explicit address. Only old
    // persisted symbol values need this compatibility lookup.
    if (/^0x[0-9a-f]{40}$/i.test(input)) { setTokenReady(true); return undefined }
    let cancelled = false
    setTokenReady(false)
    request('/api/monad/resolve?address=' + encodeURIComponent(input))
      .then((resolved) => {
        if (cancelled) return
        if (!resolved.success) throw new Error(resolved.hint || 'Choose an explicit token address')
        const nextToken = resolved.resolved_token
        setResolveInfo(resolved)
        if (nextToken !== input) {
          setToken(nextToken)
          window.localStorage.setItem(TOKEN_KEY, nextToken)
        }
        setTokenReady(true)
      })
      .catch((resolveError) => {
        if (!cancelled) {
          setTokenReady(false)
          setError(resolveError.message || 'Address resolution failed')
        }
      })
    return () => { cancelled = true }
  }, [token])

  const load = useCallback(async () => {
    if (!token.trim()) {
      setCatalog(null)
      setAlerts([])
      setEvents([])
      setSnapshots([])
      return
    }
    setLoading(true)
    setError('')
    const encoded = encodeURIComponent(token.trim())
    const generation = queryGeneration.current
    const current = () => activeToken.current === token && queryGeneration.current === generation
    const task = (path, accept, primary = false) => request(path)
      .then(value => { if (current()) { accept(value); if (primary) setLastRefresh(Date.now()) } })
      .catch(reason => { if (current()) setError(reason.message || 'Monad API unavailable') })
      .finally(() => { if (primary && current()) setLoading(false) })
    // Render catalog and each feed as soon as its own request completes.
    // Slow RPC/event responses never hold the new pool list hostage.
    await Promise.allSettled([
      task('/api/monad/pools?token=' + encoded + '&per_protocol=2', setCatalog, true),
      task('/api/monad/whale-alerts?token=' + encoded + '&limit=12', v => setAlerts(v.alerts || [])),
      task('/api/monad/events?token=' + encoded + '&limit=18', v => setEvents(v.events || [])),
      task('/api/monad/pool-snapshots?token=' + encoded + '&limit=1000', v => setSnapshots(v.snapshots || [])),
      task('/api/monad/protocols', v => {
        const nativeTypes = (v.protocols || []).filter(p => p.decoder === 'evm-abi').flatMap(p => p.pool_types || [])
        if (nativeTypes.length) setDecodedPoolTypes(new Set(nativeTypes))
      }),
    ])
  }, [token])

  const loadEvents = useCallback(async () => {
    if (!token.trim()) return
    const generation = queryGeneration.current
    try {
      const payload = await request('/api/monad/events?token=' + encodeURIComponent(token.trim()) + '&limit=18')
      if (activeToken.current === token && queryGeneration.current === generation) setEvents(payload.events || [])
    } catch {
      // Keep the last decoded event list if a short poll misses an RPC/API cycle.
    }
  }, [token])

  useEffect(() => {
    const refresh = () => { load(); loadEvents(); setHistoryRefresh(v => v + 1) }
    window.addEventListener('monad-refresh', refresh)
    return () => window.removeEventListener('monad-refresh', refresh)
  }, [load, loadEvents])

  useEffect(() => {
    if (!selectedPool?.pool_address) {
      setPoolHistory(null)
      setPoolCandles([])
      setPoolMarketMeta({})
      setPoolCandleError('')
      return undefined
    }
    let cancelled = false
    const abort = new AbortController()
    const key = token + '/' + selectedPool.pool_address + '/' + candleResolution
    if (historyKey.current !== key) {
      historyKey.current = key
      setPoolHistory(null)
      setPoolCandles([])
      setPoolMarketMeta({})
    }
    setPoolHistoryLoading(true)
    setPoolMarketLoading(true)
    setPoolCandleError('')
    request('/api/monad/pool-history?pool=' + encodeURIComponent(selectedPool.pool_address) + '&target_mint=' + encodeURIComponent(token) + '&hours=24&resolution=' + candleResolution + '&limit=2000', { signal: abort.signal })
      .then(v => { if (!cancelled) setPoolHistory(v) })
      .catch(() => { if (!cancelled) setPoolHistory(null) })
      .finally(() => { if (!cancelled) setPoolHistoryLoading(false) })
    request('/api/monad/pool-candles?pool=' + encodeURIComponent(selectedPool.pool_address) + '&token=' + encodeURIComponent(token) + '&resolution=' + candleResolution + '&limit=' + ({ '1m': 1000, '5m': 288, '15m': 96, '1h': 24 }[candleResolution] || 1000), { signal: abort.signal })
      .then(v => { if (!cancelled) { setPoolCandles(v.price_unit === 'USD' ? normalizeProviderCandles(v) : []); setPoolMarketMeta(v) } })
      .catch(e => { if (!cancelled) { setPoolCandles([]); setPoolCandleError(e.message || 'Market candles unavailable') } })
      .finally(() => { if (!cancelled) setPoolMarketLoading(false) })
    return () => { cancelled = true; abort.abort() }
  }, [selectedPool?.pool_address, token, candleResolution, historyRefresh])

  useEffect(() => {
    if (!token.trim() || !tokenReady) return undefined
    let cancelled = false
    const activate = async () => {
      setListenerError('')
      try {
        const response = await request('/api/monad/listener/start', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ token: token.trim() }),
        })
        if (!cancelled) setListenerStatus(response)
      } catch (startError) {
        if (!cancelled) setListenerError(startError.message || 'Listener start failed')
      }
      if (!cancelled) await load()
    }
    activate()
    const timer = window.setInterval(() => { load(); request('/api/monad/listener/status').then(v => { if (activeToken.current === token) setListenerStatus(v) }).catch(() => {}) }, 5000)
    return () => {
      cancelled = true
      window.clearInterval(timer)
    }
  }, [load, tokenReady, selectionRevision])

  useEffect(() => {
    if (!token.trim() || !tokenReady) return undefined
    const timer = window.setInterval(loadEvents, 2500)
    return () => window.clearInterval(timer)
  }, [loadEvents, tokenReady, token])

  const decodedPools = useMemo(
    () => (catalog?.pools || []).filter((pool) => decodedPoolTypes.has(pool.pool_type)),
    [catalog, decodedPoolTypes],
  )

  const groupedPools = useMemo(() => {
    const groups = new Map()
    for (const pool of decodedPools) {
      const key = pool.protocol || 'Unknown protocol'
      if (!groups.has(key)) groups.set(key, [])
      groups.get(key).push(pool)
    }
    return [...groups.entries()]
  }, [decodedPools])

  useEffect(() => {
    if (!catalog) return
    const pools = decodedPools
    setSelectedPool((current) => {
      if (!pools.length) return null
      if (current?.pool_address) { const updated = pools.find(pool => pool.pool_address === current.pool_address); if (updated) return updated }
      return pools[0]
    })
  }, [catalog, decodedPools])

  const snapshotByPool = useMemo(() => new Map(
    snapshots.map((snapshot) => [snapshot.pool_address?.toLowerCase(), snapshot]),
  ), [snapshots])

  const poolByAddress = useMemo(() => new Map(
    decodedPools.map((pool) => [pool.pool_address?.toLowerCase(), pool]),
  ), [decodedPools])

  const latestTradeByPool = useMemo(() => {
    const result = new Map()
    for (const event of events) {
      const pool = event.watched_pool?.toLowerCase()
      const trade = tradeOf(event)
      if (pool && (trade.swap_price_usd != null || trade.swap_ratio_quote_per_target != null)) {
        result.set(pool, { ...trade, as_of: event.observed_at_ms })
      }
    }
    return result
  }, [events])

  const visibleEvents = useMemo(
    () => events.filter((event) => {
      if (!poolByAddress.has(event.watched_pool?.toLowerCase())) return false
      const transaction = event?.enrichment?.transaction || {}
      if (transaction.err != null) return false
      if (['pool_maintenance', 'unrelated_router_event'].includes(transaction.event_kind)) return false
      if (event?.enrichment?.pending === true) return true
      const trade = transaction.trade || {}
      const hasSettledTrade = ['buy', 'sell'].includes(trade.side)
        && Number(trade.target_amount_ui) > 0
        && Number(trade.quote_amount_ui) > 0
      const hasDecodedLiquidity = ['add_liquidity', 'remove_liquidity'].includes(transaction.event_kind)
        && (transaction.liquidity?.detected === true
          || (Array.isArray(transaction.protocol_details?.amounts)
            && transaction.protocol_details.amounts.some((amount) => Number(amount) > 0)))
      return hasSettledTrade || hasDecodedLiquidity
    }),
    [events, poolByAddress],
  )

  const visibleAlerts = alerts.filter((alert) => poolByAddress.has(alert.watched_pool?.toLowerCase()))
  const totalLiquidity = decodedPools.reduce((sum, pool) => sum + (Number(pool.liquidity_usd) || 0), 0)
  const streamReady = Boolean(lastRefresh && !error && !listenerError)

  const applyToken = async (event) => {
    event.preventDefault()
    const next = draftToken.trim()
    const generation = ++queryGeneration.current
    if (!next) {
      setToken('')
      setResolveInfo(null)
      window.localStorage.removeItem(TOKEN_KEY)
      window.localStorage.removeItem(INPUT_KEY)
      return
    }
    setResolving(true)
    setError('')
    setListenerError('')
    setCatalog(null)
    setPoolHistory(null)
    setPoolCandles([])
    setPoolMarketMeta({})
    setEvents([])
    setAlerts([])
    setSnapshots([])
    setSelectedPool(null)
    setTokenReady(false)
    window.localStorage.setItem(INPUT_KEY, next)
    try {
      const resolved = await request('/api/monad/resolve?address=' + encodeURIComponent(next))
      if (queryGeneration.current !== generation) return
      setResolveInfo(resolved)
      if (!resolved.success) { setError(resolved.hint); return }
      const resolvedToken = resolved.resolved_token
      setResolveInfo(resolved)
      setToken(resolvedToken)
      // Re-submitting the same name/address must reactivate loading as well.
      setTokenReady(true)
      setSelectionRevision(v => v + 1)
      window.localStorage.setItem(TOKEN_KEY, resolvedToken)
    } catch (resolveError) {
      if (queryGeneration.current === generation) setError(resolveError.message || 'Address resolution failed')
    } finally {
      if (queryGeneration.current === generation) setResolving(false)
    }
  }

  const displayError = listenerError || error || (listenerStatus?.cursors || []).find(c => c.error)?.error
  const listenerRunning = Boolean(listenerStatus?.listener)

  return <section className="solana-listener-page monad-listener-page">
    <div className="solana-hero">
      <div className={'solana-connection ' + (displayError ? 'error' : '')}>
        <StatusMark state={displayError ? 'error' : streamReady ? 'ok' : 'idle'} />
        <strong>{displayError ? 'Attention' : listenerRunning ? 'Listening' : streamReady ? 'Connected' : 'Select token'}</strong>
        {lastRefresh && !displayError && <span>{formatTime(lastRefresh)}</span>}
      </div>
      <form className="solana-token-form" onSubmit={applyToken}>
        <div>
          <input id="solana-token" aria-label="Token or pool" value={draftToken} onChange={(event) => setDraftToken(event.target.value)} placeholder="Token name / symbol, address or pool ID" spellCheck="false" />
          <button type="submit" disabled={resolving}>{resolving ? 'Loading…' : 'Load'}</button>
        </div>
      </form>
    </div>

    {(resolveInfo?.address_type === 'pool' || resolveInfo?.address_type === 'symbol') && <div className="solana-resolved"><span>{resolveInfo.address_type === 'pool' ? 'Pool' : 'Symbol'}</span><strong>{shortAddress(resolveInfo.input_address)}</strong><span>→</span><strong>{shortAddress(resolveInfo.resolved_token)}</strong><span>{catalog?.pool_count || 0} selected pools</span></div>}
    {resolveInfo?.requires_selection && <div className="solana-resolved"><label>Choose token address <select aria-label="Choose token address" defaultValue="" onChange={e => { setSelectionRevision(v => v + 1); setTokenReady(true); setToken(e.target.value); setResolveInfo(null); setError(''); window.localStorage.setItem(TOKEN_KEY, e.target.value) }}><option value="" disabled>Select address</option>{resolveInfo.candidates.map(c => <option key={c.address} value={c.address}>{c.symbol} · {c.address}</option>)}</select></label></div>}
    {displayError && <div className="solana-error">{displayError}</div>}

    {!token.trim() ? <div className="solana-empty">
      <div className="solana-empty-mark">◎</div>
      <h3>Enter a token or pool</h3>
    </div> : <>
      <div className="solana-metrics">
        <Metric label="Pools" value={catalog ? decodedPools.length : '—'} accent />
        <Metric label="Liquidity" value={formatMoney(totalLiquidity)} />
        <Metric label="Whales" value={visibleAlerts.length} />
      </div>

      <div className="solana-workspace">
        <main className="solana-pool-column">
          <div className="solana-section-heading">
              <div><h3>Top pools by TVL</h3></div>
          </div>
          <PoolDetail pool={selectedPool} history={poolHistory} providerCandles={poolCandles} marketMeta={poolMarketMeta} poolEvents={visibleEvents.filter((event) => event.watched_pool?.toLowerCase() === selectedPool?.pool_address?.toLowerCase())} candleError={poolCandleError} providerLoading={poolMarketLoading} loading={poolHistoryLoading} poolByAddress={poolByAddress} snapshot={selectedPool ? snapshotByPool.get(selectedPool.pool_address?.toLowerCase()) : null} targetMint={token} candleResolution={candleResolution} onCandleResolutionChange={setCandleResolution} />
          {groupedPools.length ? groupedPools.map(([protocol, pools]) => <ProtocolGroup key={protocol} protocol={protocol} pools={pools} snapshots={snapshotByPool} trades={latestTradeByPool} selectedPool={selectedPool} onSelect={setSelectedPool} targetMint={token} />) : <div className="solana-empty compact"><h3>No decoded Monad pools</h3><p>{catalog?.pool_count ? 'Matching pools exist, but their protocol decoder is not enabled in this listener surface.' : 'Check the mint address or refresh the local pool catalog.'}</p></div>}
        </main>

        <aside className="solana-feed-column">
          <section className="solana-feed-panel">
            <div className="solana-section-heading"><div><h3>Whale alerts</h3></div><span className="solana-count">{visibleAlerts.length}</span></div>
            {visibleAlerts.length ? visibleAlerts.map((event) => <AlertRow key={event.signature + '-' + event.watched_pool + '-' + event.log_index} event={event} pool={poolByAddress.get(event.watched_pool?.toLowerCase())} />) : <div className="solana-feed-empty">No alerts</div>}
          </section>
          <section className="solana-feed-panel">
            <div className="solana-section-heading"><div><h3>Recent events</h3></div><span className="mono-cell">{visibleEvents.length}</span></div>
            {visibleEvents.length ? visibleEvents.slice().reverse().map((event) => <EventRow key={event.signature + '-' + event.watched_pool + '-' + event.log_index} event={event} pool={poolByAddress.get(event.watched_pool?.toLowerCase())} snapshot={snapshotByPool.get(event.watched_pool?.toLowerCase())} />) : <div className="solana-feed-empty">No recent events</div>}
          </section>
        </aside>
      </div>
    </>}
  </section>
}
