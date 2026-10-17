import { useEffect, useRef, useState } from 'react'
import { createWalletClient, custom, formatEther, formatUnits, parseAbi, parseEventLogs } from 'viem'
import { monad } from 'viem/chains'
import { POOL_ID, USDC, ROUTER, POOL_MANAGER, ERC20_ABI, publicClient, parseAmount, quoteSwap, buildSwap, readApprovals, approvalStep, buildApproval, ensureMonad, isFresh, QUOTE_TTL_MS } from './monadTrade'

const short = address => address ? `${address.slice(0, 6)}…${address.slice(-4)}` : ''
const format = (amount, token) => formatUnits(amount, token === 'USDC' ? 6 : 18)
const swapEvent = parseAbi(['event Swap(bytes32 indexed id,address indexed sender,int128 amount0,int128 amount1,uint160 sqrtPriceX96,uint128 liquidity,int24 tick,uint24 fee)'])
function receiptDetails(transaction, receipt) {
  let received = null
  if (transaction.kind === 'swap' && transaction.outputToken === 'USDC') {
    const transfers = parseEventLogs({ abi: parseAbi(['event Transfer(address indexed from,address indexed to,uint256 value)']), logs: receipt.logs.filter(log => log.address.toLowerCase() === USDC) }).filter(log => log.args.to.toLowerCase() === transaction.recipient.toLowerCase())
    if (transfers.length) received = transfers.reduce((sum, log) => sum + log.args.value, 0n)
  } else if (transaction.kind === 'swap') {
    const swaps = parseEventLogs({ abi: swapEvent, logs: receipt.logs.filter(log => log.address.toLowerCase() === POOL_MANAGER) }).filter(log => log.args.id === POOL_ID && log.args.sender.toLowerCase() === ROUTER && log.args.amount0 > 0n)
    if (swaps.length) received = swaps.reduce((sum, log) => sum + log.args.amount0, 0n)
  }
  return { ...transaction, status: receipt.status === 'success' ? 'confirmed' : 'reverted', received }
}
const message = error => {
  for (let cause = error, depth = 0; cause && depth < 10; cause = cause.cause, depth++) {
    if (Number(cause.code) === 4001 || cause.name === 'UserRejectedRequestError') return 'Cancelled in wallet. Nothing was submitted.'
  }
  return error.shortMessage || error.message || 'Request failed.'
}

export default function MonadTrade() {
  const [wallets, setWallets] = useState([]), [walletId, setWalletId] = useState('')
  const [provider, setProvider] = useState(null), [account, setAccount] = useState(''), [chain, setChain] = useState(null)
  const [payToken, setPayToken] = useState('MON'), [usdcBalance, setUsdcBalance] = useState(null), [approvals, setApprovals] = useState(null)
  const [amount, setAmount] = useState(''), [slippage, setSlippage] = useState(50)
  const [quote, setQuote] = useState(null), [balance, setBalance] = useState(null)
  const [busy, setBusy] = useState(''), [error, setError] = useState(''), [tx, setTx] = useState(null), [now, setNow] = useState(Date.now())
  const generation = useRef(0), mounted = useRef(true)
  const invalidate = () => { generation.current++; setQuote(null); setError('') }
  useEffect(() => {
    mounted.current = true
    const announce = event => {
      const { info, provider: announced } = event.detail || {}
      if (!info?.uuid || !announced?.request) return
      setWallets(current => current.some(w => w.id === info.uuid) ? current : [...current, { id: info.uuid, name: info.name, provider: announced }])
      setWalletId(current => current || info.uuid)
    }
    window.addEventListener('eip6963:announceProvider', announce)
    window.dispatchEvent(new Event('eip6963:requestProvider'))
    if (window.ethereum?.request) {
      setWallets(current => [...current, { id: 'injected', name: 'Browser wallet', provider: window.ethereum }])
      setWalletId(current => current || 'injected')
    }
    const clock = setInterval(() => setNow(Date.now()), 1000)
    return () => { mounted.current = false; generation.current++; clearInterval(clock); window.removeEventListener('eip6963:announceProvider', announce) }
  }, [])
  useEffect(() => {
    if (!provider) return
    const accountsChanged = accounts => { invalidate(); setAccount(accounts[0] || ''); setBalance(null); setUsdcBalance(null); setApprovals(null) }
    const chainChanged = id => { invalidate(); setChain(Number(id)); setBalance(null); setUsdcBalance(null); setApprovals(null) }
    const disconnected = () => { invalidate(); setAccount(''); setBalance(null); setUsdcBalance(null); setApprovals(null) }
    provider.on?.('accountsChanged', accountsChanged); provider.on?.('chainChanged', chainChanged); provider.on?.('disconnect', disconnected)
    return () => { provider.removeListener?.('accountsChanged', accountsChanged); provider.removeListener?.('chainChanged', chainChanged); provider.removeListener?.('disconnect', disconnected) }
  }, [provider])
  useEffect(() => {
    if (!account || chain !== 143) return
    let cancelled = false
    const load = async () => { try { const [value, tokens] = await Promise.all([publicClient.getBalance({ address: account }), publicClient.readContract({ address: USDC, abi: ERC20_ABI, functionName: 'balanceOf', args: [account] })]); if (!cancelled) { setBalance(value); setUsdcBalance(tokens) } } catch { if (!cancelled) { setBalance(null); setUsdcBalance(null) } } }
    load(); const timer = setInterval(load, 10000)
    return () => { cancelled = true; clearInterval(timer) }
  }, [account, chain, tx?.status])
  useEffect(() => {
    if (!account || chain !== 143 || payToken !== 'USDC') return
    let cancelled = false
    const load = async () => { try { const value = await readApprovals(account); if (!cancelled) setApprovals(value) } catch { if (!cancelled) setApprovals(null) } }
    load(); const timer = setInterval(load, 10000)
    return () => { cancelled = true; clearInterval(timer) }
  }, [account, chain, payToken, tx?.status])
  const checkWallet = async (id, walletAccount) => {
    const [network, accounts] = await Promise.all([provider.request({ method: 'eth_chainId' }), provider.request({ method: 'eth_accounts' })])
    if (generation.current !== id || Number(network) !== 143 || accounts[0]?.toLowerCase() !== walletAccount.toLowerCase()) throw new Error('Wallet changed. Please try again with the selected account on Monad.')
  }
  const send = async (transaction, details, id) => {
    const [gas, gasPrice, funds] = await Promise.all([publicClient.estimateGas(transaction), publicClient.getGasPrice(), publicClient.getBalance({ address: transaction.account })])
    const gasLimit = gas * 120n / 100n
    if (funds < transaction.value + gasLimit * gasPrice * 2n) throw new Error('Insufficient MON: leave enough MON for the network fee.')
    await checkWallet(id, transaction.account)
    if (details.kind === 'swap' && !isFresh(details.quote)) throw new Error('Quote expired. Get a new pool quote.')
    setBusy('Confirm in your wallet')
    const hash = await createWalletClient({ chain: monad, transport: custom(provider) }).sendTransaction({ ...transaction, gas: gasLimit })
    const pending = { hash, status: 'pending', recipient: transaction.account, ...details }
    if (!mounted.current) return
    setTx(pending); setQuote(null); setBusy('Waiting for confirmation')
    const receipt = await publicClient.waitForTransactionReceipt({ hash, timeout: 120000, pollingInterval: 1500 })
    if (!mounted.current) return
    setTx(receiptDetails(pending, receipt))
    if (receipt.status !== 'success') throw new Error('Transaction reverted. Check the transaction details for the network fee.')
    if (details.kind === 'approval') { const current = await readApprovals(transaction.account); if (generation.current === id) setApprovals(current) }
    else window.dispatchEvent(new Event('monad-demo-refresh'))
  }
  const connect = async () => {
    invalidate(); setBusy('Connecting wallet')
    try {
      const selected = wallets.find(w => w.id === walletId)?.provider
      if (!selected) throw new Error('Open this page in MetaMask, Rabby, or another EVM wallet browser.')
      await ensureMonad(selected)
      const accounts = await selected.request({ method: 'eth_requestAccounts' })
      if (!accounts[0]) throw new Error('No wallet account selected.')
      setProvider(selected); setAccount(accounts[0]); setChain(143)
    } catch (e) { setError(message(e)) } finally { setBusy('') }
  }
  const preview = async () => {
    invalidate(); const id = generation.current; setBusy('Getting pool quote')
    try {
      const result = await quoteSwap(parseAmount(amount, payToken), slippage, payToken === 'MON')
      if (mounted.current && generation.current === id) setQuote(result)
    } catch (e) { if (generation.current === id) setError(message(e)) }
    finally { if (mounted.current) setBusy('') }
  }
  const buy = async () => {
    const id = generation.current, currentQuote = quote, walletAccount = account
    setError(''); setBusy('Simulating transaction')
    try {
      await checkWallet(id, walletAccount)
      if (!isFresh(currentQuote)) throw new Error('Quote expired. Get a new pool quote.')
      if (!currentQuote.zeroForOne) {
        const [current, tokens] = await Promise.all([readApprovals(walletAccount), publicClient.readContract({ address: USDC, abi: ERC20_ABI, functionName: 'balanceOf', args: [walletAccount] })])
        if (tokens < currentQuote.amountIn) throw new Error('Insufficient USDC balance.')
        if (approvalStep(current, currentQuote.amountIn) !== 'ready') { setApprovals(current); throw new Error('Authorize the USDC amount before swapping.') }
      }
      const block = await publicClient.getBlock()
      const transaction = buildSwap({ account: walletAccount, amountIn: currentQuote.amountIn, amountOutMinimum: currentQuote.minimum, zeroForOne: currentQuote.zeroForOne, deadline: block.timestamp + 120n })
      await send(transaction, { kind: 'swap', outputToken: currentQuote.zeroForOne ? 'USDC' : 'MON', quote: currentQuote }, id)
    } catch (e) { if (mounted.current) setError(message(e)) }
    finally { if (mounted.current) setBusy('') }
  }
  const authorize = async () => {
    invalidate(); const id = generation.current, walletAccount = account
    setBusy('Checking USDC authorization')
    try {
      await checkWallet(id, walletAccount)
      const input = parseAmount(amount, 'USDC')
      const [current, block, tokens] = await Promise.all([readApprovals(walletAccount), publicClient.getBlock(), publicClient.readContract({ address: USDC, abi: ERC20_ABI, functionName: 'balanceOf', args: [walletAccount] })])
      if (tokens < input) throw new Error('Insufficient USDC balance.')
      const step = approvalStep(current, input, Number(block.timestamp))
      setApprovals(current)
      if (step === 'ready') return
      const transaction = buildApproval({ account: walletAccount, amount: input, step, timestamp: block.timestamp })
      await checkWallet(id, walletAccount)
      await send(transaction, { kind: 'approval', stage: step, approvedAmount: input }, id)
    } catch (e) { if (mounted.current) setError(message(e)) }
    finally { if (mounted.current) setBusy('') }
  }
  const checkConfirmation = async () => {
    setBusy('Checking confirmation'); setError('')
    try { setTx(receiptDetails(tx, await publicClient.getTransactionReceipt({ hash: tx.hash }))) }
    catch { setError('Confirmation is not available yet. Check the explorer or check again.') }
    finally { setBusy('') }
  }
  const disconnect = () => { invalidate(); setProvider(null); setAccount(''); setChain(null); setBalance(null); setUsdcBalance(null); setApprovals(null) }
  let input = null
  try { input = parseAmount(amount, payToken) } catch { /* Wait for a valid amount. */ }
  const step = input ? approvalStep(approvals, input, Math.floor(now / 1000)) : 'loading'
  const outputToken = payToken === 'MON' ? 'USDC' : 'MON'
  return <section className="monad-trade" aria-labelledby="monad-trade-title">
    <div className="monad-trade-heading"><div><h2 id="monad-trade-title">{payToken === 'MON' ? 'Buy USDC with MON' : 'Buy MON with USDC'}</h2><p>Monad Mainnet · Uniswap V4 · Pool fee 0.05%</p></div>
      <div className="monad-trade-wallet">{account ? <><span title={account}>{short(account)}{balance !== null && ` · ${Number(formatEther(balance)).toFixed(5)} MON`}{usdcBalance !== null && ` · ${format(usdcBalance, 'USDC')} USDC`}</span><button className="secondary-button" onClick={disconnect} disabled={!!busy}>Disconnect</button></> : <>{wallets.length > 1 && <select aria-label="Wallet" value={walletId} onChange={e => setWalletId(e.target.value)} disabled={!!busy}>{wallets.map(w => <option key={w.id} value={w.id}>{w.name}</option>)}</select>}<button className="primary-button" onClick={connect} disabled={!!busy}>{wallets.length ? 'Connect wallet' : 'Connect EVM wallet'}</button></>}{account && chain !== 143 && <button className="primary-button" disabled={!!busy} onClick={connect}>Switch to Monad</button>}</div>
    </div>
    <div className="monad-trade-form"><label>Direction<select aria-label="Trade direction" value={payToken} disabled={!!busy || tx?.status === 'pending'} onChange={e => { invalidate(); setPayToken(e.target.value); setAmount('') }}><option value="MON">MON → USDC</option><option value="USDC">USDC → MON</option></select></label><label>Pay {payToken}<input aria-label={`${payToken} amount`} inputMode="decimal" placeholder="0.0" value={amount} disabled={!!busy} onChange={e => { invalidate(); setAmount(e.target.value) }} /></label><label>Slippage<select aria-label="Slippage" value={slippage} disabled={!!busy} onChange={e => { invalidate(); setSlippage(Number(e.target.value)) }}><option value={10}>0.1%</option><option value={50}>0.5%</option><option value={100}>1%</option></select></label><button className="secondary-button" disabled={!amount || !!busy} onClick={preview}>Get pool quote</button>{payToken === 'USDC' && account && chain === 143 && step !== 'ready' && <button className="secondary-button" disabled={!input || !!busy || tx?.status === 'pending' || step === 'loading'} onClick={authorize}>{step === 'loading' ? 'Checking authorization' : step === 'token' ? '1. Approve USDC' : '2. Authorize swap'}</button>}<button className="primary-button" disabled={!account || chain !== 143 || (payToken === 'USDC' && step !== 'ready') || !isFresh(quote, now) || !!busy || tx?.status === 'pending'} onClick={buy}>Confirm purchase</button></div>
    {quote && <div className="monad-trade-quote"><span>Expected: <strong>{format(quote.amountOut, outputToken)} {outputToken}</strong></span><span>Minimum received: <strong>{format(quote.minimum, outputToken)} {outputToken}</strong></span><span>{isFresh(quote, now) ? `Quote expires in ${Math.max(0, Math.ceil((QUOTE_TTL_MS - now + quote.receivedAt) / 1000))}s` : 'Quote expired — refresh before buying'}</span></div>}
    <p className="monad-trade-note">Pool ID: <span className="mono-cell">{POOL_ID}</span><br />USDC contract: <span className="mono-cell">{USDC}</span><br />{payToken === 'MON' ? 'MON pays for this swap and the network fee. Your wallet receives the USDC. No token approval is needed.' : 'USDC buys native MON in this same pool. Keep some MON for gas. First approve the entered USDC amount, then authorize the router for that amount for 10 minutes. Each approval requires a wallet transaction; no unlimited approval. Get a new quote after authorizing.'}</p>
    {payToken === 'USDC' && input && <p>Authorization limit: {format(input, 'USDC')} USDC · Router authorization: 10 minutes{step === 'ready' && ' · Ready to swap'}</p>}
    {!!busy && <p role="status">{busy}…</p>}{error && <p role="alert" className="solana-error">{error}</p>}
    {tx && <p role="status"><a href={`https://monadscan.com/tx/${tx.hash}`} target="_blank" rel="noopener noreferrer">{tx.kind === 'approval' ? 'Approval' : 'Transaction'} {tx.status} · {short(tx.hash)}</a>{tx.status === 'confirmed' && tx.kind === 'swap' && (tx.received !== null ? ` · Received ${format(tx.received, tx.outputToken)} ${tx.outputToken}${tx.outputToken === 'MON' ? ' (before network fee)' : ''}` : ' · Output amount unavailable in receipt')}{tx.status === 'pending' && !busy && <> · Confirmation not yet observed. <button className="secondary-button" onClick={checkConfirmation}>Check confirmation</button></>}</p>}
  </section>
}
