import {useEffect, useRef, useState} from 'react'
import {createPublicClient, http, formatEther} from 'viem'
import {monadTestnet} from 'viem/chains'
import data from '../../generated/monad-ai.json'
import {ACTIONS, FEATURE_LABELS, predictTiny} from './monadAi'
import './monadAi.css'

const client = createPublicClient({chain: monadTestnet, transport: http('/api/selection/testnet-rpc', {timeout: 30000, retryCount: 0})})
const pct = x => `${(x * 100).toFixed(2)}%`
const date = t => new Date(t * 1000).toLocaleString('zh-CN', {month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hour12: false})
const price = n => Number(n).toLocaleString('en-US', {maximumSignificantDigits: 7})
function Chart({points, color, label}) {
  if (!points.length) return <p className="mai-muted">此区间没有数据</p>
  const min = Math.min(...points.map(p => p[1])), max = Math.max(...points.map(p => p[1]))
  const coords = points.map(p => `${36 + (p[0] - points[0][0]) / (points.at(-1)[0] - points[0][0] || 1) * 928},${190 - (p[1] - min) / (max - min || 1) * 160}`).join(' ')
  return <div className="mai-chart"><svg viewBox="0 0 1000 230" role="img" aria-label={label}>
    {[30, 110, 190].map(y => <line key={y} x1="36" x2="964" y1={y} y2={y} stroke="currentColor" opacity=".1"/>)}
    <polyline points={coords} fill="none" stroke={color} strokeWidth="2.2" vectorEffect="non-scaling-stroke"/>
    <text x="36" y="18">{price(max)}</text><text x="36" y="211">{price(min)}</text>
    <text x="36" y="229">{new Date(points[0][0] * 1000).toLocaleDateString()}</text><text x="964" y="229" textAnchor="end">{new Date(points.at(-1)[0] * 1000).toLocaleDateString()}</text>
  </svg></div>
}
function Kline({bars, symbol, period}) {
  const latest = bars.at(-1)
  if (!latest) return null
  const low = Math.min(...bars.map(b => b[3])), high = Math.max(...bars.map(b => b[2]))
  const first = bars[0][0], end = latest[0] + period
  const x = t => 55 + (t - first) / (end - first) * 920
  const y = p => 28 + (high - p) / (high - low || 1) * 210
  const width = Math.max(.6, Math.min(8, 920 * period / (end - first) * .65))
  const volume = Math.max(...bars.map(b => b[5]), 1)
  return <div className="mai-chart"><div className="mai-ohlc">{[['开', latest[1]], ['高', latest[2]], ['低', latest[3]], ['收', latest[4]]].map(([label, value]) => <span key={label}>{label} <b>{price(value)}</b></span>)}</div><svg viewBox="0 0 1000 325" role="img" aria-label={`${symbol} K线`}>
    {[0, .5, 1].map(r => <g key={r}><line x1="55" x2="975" y1={28 + r * 210} y2={28 + r * 210} stroke="currentColor" opacity=".12"/><text className="mai-price-tick" x="2" y={32 + r * 210}>{price(high - r * (high - low))}</text></g>)}
    {bars.map(b => {const color = b[4] >= b[1] ? '#70d6b1' : '#e88986'; return <g key={b[0]} fill={color} stroke={color}><title>{date(b[0])} 开 {price(b[1])} 高 {price(b[2])} 低 {price(b[3])} 收 {price(b[4])} 量 ${price(b[5])}</title><line x1={x(b[0])} x2={x(b[0])} y1={y(b[2])} y2={y(b[3])} strokeWidth="1"/><rect x={x(b[0]) - width / 2} y={Math.min(y(b[1]), y(b[4]))} width={width} height={Math.max(.8, Math.abs(y(b[1]) - y(b[4])))} stroke="none"/><rect x={x(b[0]) - width / 2} y={295 - b[5] / volume * 37} width={width} height={b[5] / volume * 37} opacity=".4" stroke="none"/></g>})}
    <text x="55" y="320">{date(first)}</text><text x="975" y="320" textAnchor="end">{date(latest[0])}</text>
  </svg></div>
}

export default function MonadAi() {
  const {report, model, deployment} = data
  const selected = report.candidates[report.selected_candidate]
  const [asset, setAsset] = useState(report.assets[0].symbol), [chart, setChart] = useState('price'), [vectorIndex, setVectorIndex] = useState(0), [days, setDays] = useState(7)
  const [replay, setReplay] = useState(null), [busy, setBusy] = useState(false), [error, setError] = useState('')
  const generation = useRef(0)
  useEffect(() => () => {generation.current++}, [])
  const info = report.assets.find(a => a.symbol === asset), backtest = report.backtests.find(a => a.symbol === asset)
  const vectors = data.vectors.filter(v => v.asset === asset), vector = vectors[Math.min(vectorIndex, vectors.length - 1)]
  const local = predictTiny(model, vector.input_q12)
  function changeAsset(value) {generation.current++; setAsset(value); setVectorIndex(0); setReplay(null); setError(''); setBusy(false)}
  async function verify() {
    const id = ++generation.current
    setBusy(true); setReplay(null); setError('')
    try {
      if (!deployment || deployment.status !== 'verified') throw Error('模型部署中')
      if (await client.getChainId() !== 10143) throw Error('连接失败，请重试')
      const address = deployment.address, abi = data.abi
      const [modelHash, datasetHash, result, gasEstimate] = await Promise.all([
        client.readContract({address, abi, functionName: 'MODEL_SHA256'}), client.readContract({address, abi, functionName: 'DATASET_SHA256'}),
        client.readContract({address, abi, functionName: 'predict', args: [vector.input_q12]}),
        client.estimateContractGas({account: deployment.deployer, address, abi, functionName: 'predict', args: [vector.input_q12]})
      ])
      if (modelHash !== `0x${report.model_sha256}` || datasetHash !== `0x${report.dataset_sha256}`) throw Error('请刷新页面，更新模型版本')
      const [scores, action] = result
      if (action !== local.action || scores.map(String).join(',') !== local.scores.join(',')) throw Error('计算结果不同，请查看模型详情')
      if (id === generation.current) setReplay({scores: scores.map(String), action, gas: String(gasEstimate)})
    } catch (e) {if (id === generation.current) setError(e.shortMessage || e.message)}
    finally {if (id === generation.current) setBusy(false)}
  }
  const period = report.config.timeframe_seconds || 3600
  const bars = data.candles[asset], latest = bars.at(-1)
  const shown = bars.filter(b => b[0] >= latest[0] - days * 86400)
  const dayIndex = Math.max(0, bars.length - 1 - Math.round(86400 / period))
  const change = latest[4] / bars[dayIndex][4] - 1
  const chainOp = deployment?.operations[`decision-${asset}`]
  const featureLabels = FEATURE_LABELS.map(label => label.replace(/(\d+) 小时/g, (_, n) => `${Number(n) * period / 3600} 小时`))
  return <section className="mai-page">
    <div className="mai-heading"><div><h1>交易 AI</h1><p>Gecko · Monad · {period / 3600} 小时</p></div><div className="mai-heading-links"><span className="mai-status">测试网</span>{deployment && <a href={deployment.explorer} target="_blank" rel="noreferrer">合约 ↗</a>}</div></div>
    <div className="mai-asset-tabs" role="group" aria-label="选择代币">{report.assets.map(a => <button key={a.symbol} aria-pressed={asset === a.symbol} onClick={() => changeAsset(a.symbol)}>{a.symbol}<span>{a.candles.toLocaleString()} 根</span></button>)}</div>
    <div className="mai-columns">
      <article className="mai-panel mai-market"><div className="mai-panel-head"><div><h2>{asset} <span>/ USD</span></h2><div className="mai-price">${price(latest[4])}<span className={change >= 0 ? 'mai-positive' : 'mai-negative'}>{change >= 0 ? '+' : ''}{pct(change)} <small>24h</small></span></div></div><div className="mai-chart-controls"><div className="mai-switch"><button aria-pressed={chart === 'price'} onClick={() => setChart('price')}>K 线</button><button aria-pressed={chart === 'equity'} onClick={() => setChart('equity')}>回测净值</button></div>{chart === 'price' && <div className="mai-switch">{[7, 30, 90].map(n => <button key={n} aria-pressed={days === n} onClick={() => setDays(n)}>{n}天</button>)}</div>}</div></div>
        {chart === 'price' ? <Kline bars={shown} symbol={asset} period={period}/> : <Chart points={backtest.equity_curve.map(p => [p.timestamp, p.equity])} color="#70d6b1" label={`${asset} 回测净值`}/>}
        <div className="mai-source"><a href={`https://www.geckoterminal.com/monad/pools/${info.pool}`} target="_blank" rel="noreferrer">{info.pool_name} ↗</a><span>{info.coverage ? `近期覆盖 ${pct(info.coverage.recent_coverage)} · ` : ''}{date(latest[0])}</span></div>
      </article>
      <article className="mai-panel mai-replay"><div className="mai-panel-head"><h2>模型信号</h2><span className="mai-muted">未来 {report.config.horizon_hours}h</span></div><div className={`mai-decision action-${local.action}`}><strong>{ACTIONS[local.action]}</strong><span>{date(vector.timestamp)}</span></div>
        <label htmlFor="ai-vector">回放时点</label><select id="ai-vector" value={Math.min(vectorIndex, vectors.length - 1)} onChange={e => {generation.current++; setVectorIndex(Number(e.target.value)); setReplay(null); setError(''); setBusy(false)}}>{vectors.map((v, i) => <option key={`${v.timestamp}-${i}`} value={i}>{date(v.timestamp)}</option>)}</select>
        <button className="mai-verify" onClick={verify} disabled={busy || deployment?.status !== 'verified'}>{busy ? '计算中…' : '链上重算'} <span>↗</span></button>
        {error && <p role="alert" className="mai-error">{error}</p>}{replay && <div className="mai-replay-result" role="status">✓ 链上一致 · {Number(replay.gas).toLocaleString()} gas</div>}
        <div className="mai-model-line"><span>{selected.architecture.join(' → ')}</span><span>{selected.parameters} 参数</span></div>
      </article>
    </div>
    <div className="mai-stats mai-backtest"><article><small>回测收益</small><strong className={backtest.return_after_assumed_costs < 0 ? 'mai-negative' : 'mai-positive'}>{pct(backtest.return_after_assumed_costs)}</strong></article><article><small>持有收益</small><strong>{pct(backtest.observed_windows_buy_hold_return)}</strong></article><article><small>最大回撤</small><strong>{pct(backtest.max_drawdown)}</strong></article><article><small>买卖次数</small><strong>{backtest.trades}</strong></article></div>
    <article className="mai-panel"><div className="mai-panel-head"><h2>回测交易 <span>{asset}</span></h2><span className="mai-muted">手续费与滑点已计入</span></div><div className="mai-table-wrap"><table><thead><tr><th>时间</th><th>动作</th><th>价格 / USD</th><th>原因</th></tr></thead><tbody>{backtest.history.length ? backtest.history.map((t, i) => <tr key={i}><td>{date(t.timestamp)}</td><td className={t.action === 'BUY' ? 'mai-positive' : 'mai-negative'}>{t.action === 'BUY' ? '买入' : '卖出'}</td><td>{t.price_usd ? price(t.price_usd) : '—'}</td><td>{t.reason === 'end_of_test' ? '结束平仓' : t.reason === 'close_before_data_gap' ? '缺口结算' : '模型信号'}</td></tr>) : <tr><td colSpan="4" className="mai-muted">这段时间没有买卖信号</td></tr>}</tbody></table></div></article>
    <details className="mai-panel mai-details"><summary>模型与数据详情 <span>{report.assets.reduce((n, a) => n + a.candles, 0).toLocaleString()} 根 K 线 · 准确率 {pct(report.test.accuracy)}</span></summary><div className="mai-columns"><div><h2>输入 · {date(vector.timestamp)}</h2><div className="mai-table-wrap"><table><thead><tr><th>特征</th><th>数值</th><th>整数输入</th></tr></thead><tbody>{featureLabels.map((label, i) => <tr key={label}><td>{label}</td><td>{Number(vector.features[i]).toPrecision(5)}</td><td>{vector.input_q12[i]}</td></tr>)}</tbody></table></div></div>
      <div><h2>训练</h2>{Object.entries(report.splits).map(([name, split]) => <div className="mai-split" key={name}><strong>{({train: '训练集', validation: '验证集', test: '测试集'})[name]} <span>{split.count} 个</span></strong><p>{split.from.slice(0, 16).replace('T', ' ')} — {split.to.slice(0, 16).replace('T', ' ')} UTC</p></div>)}<p className="mai-muted">涨跌标签 ±{report.config.label_threshold_bps / 100}%。横盘基准准确率 {pct(report.hold_only_test_accuracy)}。回测按手续费与每边 {report.config.slippage_bps_per_side} bps 滑点计算，未计 gas。</p><p className="mai-muted">Gecko 原始成交 K 线，按连续覆盖选池。当前页面为历史回放。</p><p className="mai-hash">模型：{report.model_sha256}</p><p className="mai-hash">数据：{report.dataset_sha256}</p><div className="mai-downloads"><a href="/ai/monad-small-report.json" download>训练报告 ↓</a><a href="/ai/monad-small-model.json" download>模型参数 ↓</a>{chainOp && <a href={`https://testnet.monadscan.com/tx/${chainOp.hash}`} target="_blank" rel="noreferrer">决策交易 ↗</a>}</div>{chainOp && <p className="mai-muted">本次交易手续费 {Number(formatEther(BigInt(chainOp.feeWei))).toFixed(6)} 测试 MON</p>}</div>
    </div></details>
  </section>
}
