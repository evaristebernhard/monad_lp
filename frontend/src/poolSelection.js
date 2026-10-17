import { encodeAbiParameters, parseAbiParameters, keccak256, stringToHex, parseUnits } from 'viem'
const SCALE=1n<<64n
export const poolParams=parseAbiParameters('(bytes32 id,uint256 reserveIn,uint256 reserveOut,uint256 feeBps,uint256 gasCost,uint256 weightBps,uint256 capIn)[],uint256,uint8,uint256')
export function buildInput(sample, amount, splits) {
 if(!/^\d+(?:\.\d{1,6})?$/.test(amount))throw Error('金额最多支持 6 位小数')
 const a=parseUnits(amount,6), max=(1n<<80n)-1n
 if(a<=0n||a>max||!Number.isInteger(splits)||splits<1||splits>3)throw Error('请输入正数金额，拆单数为 1–3')
 const pools=sample.pools.map(p=>({id:keccak256(stringToHex(p.name)),reserveIn:parseUnits(String(p.reserve_in),6),reserveOut:parseUnits(String(p.reserve_out),6),feeBps:BigInt(p.fee_bps),gasCost:parseUnits(String(p.incremental_gas_in_out),6),weightBps:10000n-BigInt(p.risk_bps),capIn:a}))
 return [pools,a,splits,parseUnits(String(sample.base_gas_in_output_token),6)]
}
function sqrt(n){if(n<2n)return n;let x=1n<<BigInt(Math.ceil(n.toString(2).length/2));for(;;){const y=(x+n/x)/2n;if(y>=x)return x;x=y}}
export function solveDcar(args){
 const [pools,amount,k,base]=args, n=pools.length
 const quote=(p,x)=>p.reserveOut*((10000n-p.feeBps)*x)/(10000n*p.reserveIn+(10000n-p.feeBps)*x)
 const marginal=(p,x)=>p.weightBps*p.reserveOut*(10000n-p.feeBps)*p.reserveIn*SCALE/(10000n*p.reserveIn+(10000n-p.feeBps)*x)**2n
 const inverse=(p,mu)=>{const root=sqrt(p.weightBps*p.reserveOut*(10000n-p.feeBps)*p.reserveIn*SCALE/mu),start=10000n*p.reserveIn;if(root<=start)return 0n;const x=(root-start)/(10000n-p.feeBps);return x<p.capIn?x:p.capIn}
 let best=null
 for(let mask=1;mask<1<<n;mask++){
  const ids=pools.map((_,i)=>i).filter(i=>mask&(1<<i));if(ids.length>k)continue
  if(ids.reduce((s,i)=>s+pools[i].capIn,0n)<amount)continue
  let high=ids.reduce((m,i)=>{const v=marginal(pools[i],0n);return v>m?v:m},0n)+1n,low=1n
  for(let r=0;r<160&&low<high;r++){const mid=low+(high-low)/2n,total=ids.reduce((s,i)=>s+inverse(pools[i],mid),0n);if(total>amount)low=mid+1n;else high=mid}
  const xs=pools.map((p,i)=>ids.includes(i)?inverse(p,high):0n);let remaining=amount-xs.reduce((a,b)=>a+b,0n)
  for(let pass=0;remaining>0n&&pass<n;pass++){let selected=-1,m=0n;for(const i of ids)if(xs[i]<pools[i].capIn){const v=marginal(pools[i],xs[i]);if(selected<0||v>m){selected=i;m=v}}if(selected<0)throw Error('容量不足');const room=pools[selected].capIn-xs[selected],extra=remaining<room?remaining:room;xs[selected]+=extra;remaining-=extra}
  let output=0n,score=-base
  xs.forEach((x,i)=>{if(x>0n){const out=quote(pools[i],x);output+=out;score+=out*pools[i].weightBps/10000n-pools[i].gasCost}})
  if(!best||score>best[2])best=[xs,output,score,keccak256(encodeAbiParameters(poolParams,args))]
 }
 if(!best)throw Error('无可行拆单');return best
}
export const json=(value)=>JSON.stringify(value,(_,v)=>typeof v==='bigint'?v.toString():v,2)
