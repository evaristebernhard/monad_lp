// Real Cloudflare page and chain calls. Stop at the wallet confirmation boundary.
import { chromium } from 'playwright'
import assert from 'node:assert/strict'
const browser = await chromium.launch({ headless: true })
const page = await browser.newPage()
const errors = []
page.on('pageerror', error => errors.push(error.message))
await page.addInitScript(() => {
  const account = '0x41d5D3B5f2e497d1F4bCF767ffac9DDAb85D2b79'
  window.__requestedTransactions = []
  window.ethereum = {
    on() {}, removeListener() {},
    async request({ method, params }) {
      if (method === 'eth_chainId') return '0x8f'
      if (method === 'eth_accounts' || method === 'eth_requestAccounts') return [account]
      if (method === 'eth_sendTransaction') {
        window.__requestedTransactions.push(params[0])
        const error = new Error('User rejected')
        error.code = 4001
        throw error
      }
      throw new Error('Unexpected wallet method: ' + method)
    },
  }
})
try {
  const response = await page.goto('https://lp.chayu123.me/', { waitUntil: 'domcontentloaded', timeout: 60000 })
  assert.equal(response.status(), 200)
  await page.getByRole('button', { name: 'Connect wallet', exact: true }).click()
  await page.getByRole('button', { name: 'Disconnect', exact: true }).waitFor()
  await page.getByRole('textbox', { name: 'MON amount' }).fill('0.1')
  await page.getByRole('button', { name: 'Get pool quote' }).click()
  const confirm = page.getByRole('button', { name: 'Confirm purchase' })
  await page.waitForFunction(() => [...document.querySelectorAll('button')].some(button => button.textContent === 'Confirm purchase' && !button.disabled), { timeout: 30000 })
  await confirm.click()
  await page.getByRole('alert').filter({ hasText: 'Cancelled in wallet' }).waitFor({ timeout: 30000 })
  const transactions = await page.evaluate(() => window.__requestedTransactions)
  assert.equal(transactions.length, 1)
  assert.equal(BigInt(transactions[0].value), 100000000000000000n)
  assert.equal(transactions[0].to.toLowerCase(), '0x0d97dc33264bfc1c226207428a79b26757fb9dc3')
  assert.deepEqual(errors, [])
  await page.screenshot({ path: 'output/monad-trade-public.png', fullPage: true })
  console.log(JSON.stringify({ cloudflare: true, realQuote: true, realGasEstimate: true, walletSubmissionReached: true, broadcast: false, errors }))
} catch (error) {
  console.error(await page.locator('.monad-trade').innerText().catch(() => 'Page unavailable'))
  throw error
} finally {
  await browser.close()
}
