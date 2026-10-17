import test from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import {predictTiny} from '../src/ai/research/monadAi.js'
const data = JSON.parse(fs.readFileSync(new URL('../src/generated/monad-ai.json', import.meta.url)))
test('browser integer inference matches held-out real-data reference scores', () => {
  assert.deepEqual(new Set(data.vectors.map(v => v.asset)), new Set(data.report.assets.map(a => a.symbol)))
  for (const vector of data.vectors) {
    const actual = predictTiny(data.model, vector.input_q12)
    assert.deepEqual(actual.scores, vector.scores_q12.map(String))
    assert.equal(actual.action, vector.action)
  }
})
test('browser rejects invalid input instead of evaluating a different protocol', () => {
  assert.throws(() => predictTiny(data.model, Array(12).fill(16385)))
  assert.throws(() => predictTiny(data.model, Array(11).fill(0)))
  assert.throws(() => predictTiny(data.model, Array(12).fill(NaN)))
})
