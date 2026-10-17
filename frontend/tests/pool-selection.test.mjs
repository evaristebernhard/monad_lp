import test from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import {buildInput,solveDcar,json} from '../src/poolSelection.js'
const data=JSON.parse(fs.readFileSync(new URL('../src/generated/selection.json',import.meta.url)))
for(const replay of data.dcar.reference.replays)test(`exact Solidity replay ${replay.amount}/${replay.maxSplits}`,()=>{
 const args=buildInput(data.sample,replay.amount,replay.maxSplits),result=solveDcar(args)
 assert.equal(json(result),json(replay.result));assert.equal(result[0].reduce((a,b)=>a+b,0n),args[1]);assert.ok(result[0].filter(x=>x>0n).length<=replay.maxSplits)
})
test('reject invalid amount and split count',()=>{for(const amount of ['0','-1','NaN','1e2','1.0000001',''])assert.throws(()=>buildInput(data.sample,amount,3));for(const k of [0,4,1.5])assert.throws(()=>buildInput(data.sample,'1',k))})
