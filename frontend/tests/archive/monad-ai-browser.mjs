import {chromium} from 'playwright'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
const data = JSON.parse(fs.readFileSync(new URL('../src/generated/monad-ai.json', import.meta.url)))
const origin = process.env.MONAD_WEB_ORIGIN || 'http://127.0.0.1:5173'
const browser = await chromium.launch({headless: true})
const result = {origin, chainCalls: [], errors: []}
try {
  const page = await browser.newPage({viewport: {width: 1440, height: 1000}})
  page.on('pageerror', error => result.errors.push(error.message))
  page.on('request', request => {
    if (request.url().includes('/api/selection/testnet-rpc')) {
      const body = request.postDataJSON()
      result.chainCalls.push(body.method)
      assert(!/send|sign/i.test(body.method))
    }
  })
  await page.goto(`${origin}/#ai-research`, {waitUntil: 'domcontentloaded'})
  await page.getByRole('heading', {name: '交易 AI', exact: true}).waitFor()
  const details = page.locator('details.mai-details')
  assert.equal(await details.evaluate(node => node.open), false)
  assert.equal(await page.locator('.mai-notice').count(), 0)
  const symbols = data.report.assets.map(a => a.symbol)
  const first = symbols[0], period = data.report.config.timeframe_seconds
  await page.getByRole('button', {name: '链上重算', exact: true}).click()
  await page.getByRole('status').filter({hasText: '链上一致'}).waitFor({timeout: 60000})
  result.realReplay = await page.getByRole('status').innerText()
  await page.getByRole('button', {name: '30天', exact: true}).click()
  const candles = data.candles[first], end = candles.at(-1)[0]
  const expectedBars = candles.filter(b => b[0] >= end - 30 * 86400).length
  const svg = page.getByRole('img', {name: `${first} K线`, exact: true})
  assert.equal(await svg.locator('rect').count(), expectedBars * 2)
  result.plottedCandles = expectedBars
  for (const symbol of symbols.slice(1)) {
    await page.getByRole('button', {name: new RegExp(`^${symbol}`)}).click()
    assert.equal(await page.getByRole('status').count(), 0)
    await page.getByRole('button', {name: '回测净值', exact: true}).click()
    assert.equal(await page.getByRole('img', {name: `${symbol} 回测净值`}).count(), 1)
    const test = data.report.backtests.find(b => b.symbol === symbol)
    const rows = page.locator('.mai-panel').filter({has: page.getByRole('heading', {name: new RegExp(`^回测交易 ${symbol}`)})}).locator('tbody tr')
    assert.equal(await rows.count(), Math.max(1, test.history.length))
  }
  await page.getByRole('button', {name: new RegExp(`^${first}`)}).click()
  await page.getByRole('button', {name: 'K 线', exact: true}).click()
  await page.getByRole('button', {name: '7天', exact: true}).click()
  await page.screenshot({path: '/tmp/monad-ai-desktop.png', fullPage: true})
  await page.setViewportSize({width: 390, height: 844})
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)
  await page.screenshot({path: '/tmp/monad-ai-mobile.png', fullPage: true})
  await details.locator('summary').click()
  await page.getByRole('heading', {name: '训练', exact: true}).waitFor()
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false)
  assert(result.chainCalls.includes('eth_call'))
  assert.deepEqual(result.errors, [])
  result.status = 'passed'; result.periodSeconds = period
  const destination = process.env.MONAD_AI_RUN_DIR ? path.join(process.env.MONAD_AI_RUN_DIR, 'browser-verification.json') : new URL('../../research/ai/monad-small/browser-verification.json', import.meta.url)
  fs.writeFileSync(destination, JSON.stringify(result, null, 2) + '\n')
  console.log(JSON.stringify(result, null, 2))
} finally {await browser.close()}
