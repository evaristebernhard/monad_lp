import {test} from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import {buildInput,fetchLiveInput,roundEven} from '../../research/ai/showcase/live-input.mjs'
const read=name=>JSON.parse(fs.readFileSync(new URL('../../research/ai/monad-small/'+name,import.meta.url),'utf8'))
const model=read('model.json'),data=read('dataset.json'),vectors=read('vectors.json')
test('live feature builder matches original training vectors without changing the model',()=>{
  for(const v of vectors){
    const bars=data.assets.find(a=>a.symbol===v.asset).candles
    const i=bars.findIndex(b=>b[0]+3600===v.timestamp)
    const result=buildInput(bars.slice(i-24,i+1),model)
    assert.deepEqual(result.input,v.input_q12)
    result.features.forEach((f,j)=>assert(Math.abs(f-v.features[j])<1e-12))
  }
  assert.deepEqual([roundEven(.5),roundEven(1.5),roundEven(-.5),roundEven(-1.5)],[0,2,0,-2])
})
test('missing OHLCV never falls back to archived inputs',async()=>{
  await assert.rejects(fetchLiveInput(model,async()=>({ok:false,status:429})),/HTTP 429/)
  const bars=data.assets.find(a=>a.symbol==='WMON').candles.slice(-25)
  const payload={meta:{base:{address:'0x3bd359c1119da7da1d913d1c4d2b7c461115433a'}},data:{attributes:{ohlcv_list:bars}}}
  const fetcher=async()=>({ok:true,json:async()=>payload})
  await assert.rejects(fetchLiveInput(model,fetcher,bars.at(-1)[0]+7200),/最新已收盘/)
  bars.splice(10,1)
  await assert.rejects(fetchLiveInput(model,fetcher,bars.at(-1)[0]+3600),/连续 25/)
})
