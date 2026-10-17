// Match Python's training-time round(), including negative half ties.
export function roundEven(x){
  const n=Math.floor(x),fraction=x-n
  return fraction===.5?(n%2===0?n:n+1):Math.round(x)
}
const mean=x=>x.reduce((s,v)=>s+v,0)/x.length
const std=x=>{const m=mean(x);return Math.sqrt(mean(x.map(v=>(v-m)**2)))}
export function quantize(features,model){
  return features.map((v,i)=>roundEven(Math.max(-model.input_clip,Math.min(model.input_clip,
    (v-model.mean[i])/model.scale[i]))*model.input_q))
}

export function buildInput(candles,model){
  if(candles.length!==25)throw Error('模型需要连续 25 根真实小时 K 线')
  for(let i=0;i<25;i++){
    const [t,o,h,l,c,v]=candles[i]
    if(candles[i].length!==6||!candles[i].every(Number.isFinite)||t%3600!==0||
      Math.min(o,h,l,c)<=0||h<Math.max(o,c)||l>Math.min(o,c)||v<0||
      (i>0&&t-candles[i-1][0]!==3600))throw Error('真实 OHLCV 缺失或不连续')
  }
  const closes=candles.map(b=>b[4]),current=candles.at(-1)
  const returns=closes.slice(1).map((v,i)=>Math.log(v/closes[i]))
  // Same definitions, units, scaler and Q12 rounding as monad-small/train.py.
  const features=[1,3,6,12,24].map(lag=>Math.log(closes[24]/closes[24-lag])).concat([
    (current[2]-current[3])/current[4],current[4]/current[1]-1,
    current[4]/mean(closes.slice(-6))-1,current[4]/mean(closes.slice(-24))-1,
    std(returns.slice(-6)),std(returns),Math.log1p(current[5])-Math.log1p(mean(candles.slice(-24).map(b=>b[5])))
  ])
  return {features,input:quantize(features,model),candleEnd:current[0]+3600}
}

