import { createPublicClient, http, encodeAbiParameters, encodeFunctionData, keccak256, parseAbi, parseAbiParameters, parseUnits } from 'viem'
import { monad } from 'viem/chains'

export const ZERO = '0x0000000000000000000000000000000000000000'
export const USDC = '0x754704bc059f8c67012fed69bc8a327a5aafb603'
export const POOL_ID = '0x18a9fc874581f3ba12b7898f80a683c66fd5877fd74b26a85ba9a3a79c549954'
// Official Monad deployments; original Universal Router uses the original V4 action ABI.
// https://developers.uniswap.org/docs/protocols/v4/deployments#monad-143
export const ROUTER = '0x0d97dc33264bfc1c226207428a79b26757fb9dc3'
export const PERMIT2 = '0x000000000022d473030f116ddee9f6b43ac78ba3'
export const POOL_MANAGER = '0x188d586ddcf52439676ca21a244753fa19f9ea8e'
export const ERC20_ABI = parseAbi(['function balanceOf(address owner) view returns (uint256)', 'function allowance(address owner,address spender) view returns (uint256)', 'function approve(address spender,uint256 amount) returns (bool)'])
export const PERMIT2_ABI = parseAbi(['function allowance(address owner,address token,address spender) view returns (uint160 amount,uint48 expiration,uint48 nonce)', 'function approve(address token,address spender,uint160 amount,uint48 expiration)'])
export const QUOTER = '0xa222dd357a9076d1091ed6aa2e16c9742dd26891'
export const POOL_KEY = { currency0: ZERO, currency1: USDC, fee: 500, tickSpacing: 10, hooks: ZERO }
export const KEY_TYPE = '(address currency0,address currency1,uint24 fee,int24 tickSpacing,address hooks)'
export const QUOTER_ABI = parseAbi([`function quoteExactInputSingle((${KEY_TYPE} poolKey,bool zeroForOne,uint128 exactAmount,bytes hookData) params) returns (uint256 amountOut,uint256 gasEstimate)`])
export const ROUTER_ABI = parseAbi(['function execute(bytes commands,bytes[] inputs,uint256 deadline) payable'])
export const QUOTE_TTL_MS = 30000
export const publicClient = createPublicClient({ chain: monad, transport: http('/api/monad/trade-rpc', { retryCount: 0, timeout: 22000 }), batch: { multicall: false } })

export function parseAmount(value, token = 'MON') {
  const decimals = token === 'USDC' ? 6 : 18
  if (!new RegExp(`^(?:0|[1-9]\\d*)(?:\\.\\d{1,${decimals}})?$`).test(value)) throw new Error(`Enter a positive ${token} amount, with at most ${decimals} decimals.`)
  const amount = parseUnits(value, decimals)
  if (amount <= 0n || amount >= 2n ** 128n) throw new Error(`${token} amount is outside the supported range.`)
  return amount
}
export function parseMon(value) { return parseAmount(value) }
export async function readApprovals(account, client = publicClient) {
  const [erc20, [router, expiration]] = await Promise.all([
    client.readContract({ address: USDC, abi: ERC20_ABI, functionName: 'allowance', args: [account, PERMIT2] }),
    client.readContract({ address: PERMIT2, abi: PERMIT2_ABI, functionName: 'allowance', args: [account, USDC, ROUTER] }),
  ])
  return { erc20, router, expiration }
}
export function approvalStep(approvals, amount, now = Math.floor(Date.now() / 1000)) {
  if (!approvals) return 'loading'
  if (approvals.erc20 < amount) return 'token'
  if (approvals.router < amount || approvals.expiration <= now + 30) return 'router'
  return 'ready'
}
export function buildApproval({ account, amount, step, timestamp }) {
  if (amount <= 0n || amount >= 2n ** 128n) throw new Error('Invalid approval amount.')
  if (step === 'token') return { account, to: USDC, value: 0n, data: encodeFunctionData({ abi: ERC20_ABI, functionName: 'approve', args: [PERMIT2, amount] }) }
  if (step !== 'router') throw new Error('Invalid approval step.')
  return { account, to: PERMIT2, value: 0n, data: encodeFunctionData({ abi: PERMIT2_ABI, functionName: 'approve', args: [USDC, ROUTER, amount, Number(timestamp) + 600] }) }
}
export function minimumOutput(amountOut, slippageBps) {
  if (!Number.isInteger(slippageBps) || slippageBps < 1 || slippageBps > 100) throw new Error('Slippage must be between 0.01% and 1%.')
  const minimum = amountOut * BigInt(10000 - slippageBps) / 10000n
  if (minimum <= 0n || minimum >= 2n ** 128n) throw new Error('Amount is too small or too large for this pool.')
  return minimum
}
export function buildSwap({ account, amountIn, amountOutMinimum, deadline, zeroForOne = true }) {
  if (keccak256(encodeAbiParameters(parseAbiParameters(KEY_TYPE), [POOL_KEY])) !== POOL_ID) throw new Error('Pool identity mismatch.')
  if (amountIn <= 0n || amountIn >= 2n ** 128n || amountOutMinimum <= 0n || amountOutMinimum >= 2n ** 128n) throw new Error('Invalid swap amounts.')
  // V4_SWAP: SWAP_EXACT_IN_SINGLE → SETTLE_ALL → TAKE_ALL.
  // TAKE_ALL pays the wallet that called execute. SWEEP refunds any unused MON.
  const currencyIn = zeroForOne ? ZERO : USDC, currencyOut = zeroForOne ? USDC : ZERO
  const actions = encodeAbiParameters(parseAbiParameters('bytes,bytes[]'), ['0x060c0f', [
    encodeAbiParameters(parseAbiParameters(`(${KEY_TYPE} poolKey,bool zeroForOne,uint128 amountIn,uint128 amountOutMinimum,bytes hookData)`), [{ poolKey: POOL_KEY, zeroForOne, amountIn, amountOutMinimum, hookData: '0x' }]),
    encodeAbiParameters(parseAbiParameters('address,uint256'), [currencyIn, amountIn]),
    encodeAbiParameters(parseAbiParameters('address,uint256'), [currencyOut, amountOutMinimum]),
  ]])
  const refund = encodeAbiParameters(parseAbiParameters('address,address,uint256'), [currencyIn, '0x0000000000000000000000000000000000000001', 0n])
  return { account, to: ROUTER, value: zeroForOne ? amountIn : 0n, data: encodeFunctionData({ abi: ROUTER_ABI, functionName: 'execute', args: ['0x1004', [actions, refund], deadline] }) }
}
export async function quoteSwap(amountIn, slippageBps, zeroForOne = true, client = publicClient) {
  const { result } = await client.simulateContract({ address: QUOTER, abi: QUOTER_ABI, functionName: 'quoteExactInputSingle', args: [{ poolKey: POOL_KEY, zeroForOne, exactAmount: amountIn, hookData: '0x' }] })
  const [amountOut] = result
  return { zeroForOne, amountIn, amountOut, minimum: minimumOutput(amountOut, slippageBps), slippageBps, receivedAt: Date.now() }
}
export function quoteMon(amountIn, slippageBps, client = publicClient) { return quoteSwap(amountIn, slippageBps, true, client) }
export function isFresh(quote, now = Date.now()) { return !!quote && now - quote.receivedAt < QUOTE_TTL_MS }
export async function ensureMonad(provider) {
  if (Number(await provider.request({ method: 'eth_chainId' })) === 143) return
  try { await provider.request({ method: 'wallet_switchEthereumChain', params: [{ chainId: '0x8f' }] }) }
  catch (e) {
    if (Number(e.code) !== 4902) throw e
    await provider.request({ method: 'wallet_addEthereumChain', params: [{ chainId: '0x8f', chainName: 'Monad Mainnet', nativeCurrency: { name: 'MON', symbol: 'MON', decimals: 18 }, rpcUrls: ['https://rpc.monad.xyz'], blockExplorerUrls: ['https://monadscan.com'] }] })
    await provider.request({ method: 'wallet_switchEthereumChain', params: [{ chainId: '0x8f' }] })
  }
  if (Number(await provider.request({ method: 'eth_chainId' })) !== 143) throw new Error('Switch your wallet to Monad Mainnet.')
}
