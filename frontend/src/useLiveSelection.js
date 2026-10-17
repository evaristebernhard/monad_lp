import {useEffect,useRef,useState} from 'react'
import {parseAmount} from './monadTrade.js'
import {subscribeSelectionQuotes,selectV4} from './selectionQuotes.js'
export const QUOTE_STALE_MS=5000
export default function useLiveSelection({candidates,amount,payToken,enabled,paused}){
 const [value,setValue]=useState(null),[error,setError]=useState(''),[loading,setLoading]=useState(false),[now,setNow]=useState(Date.now()),[nonce,setNonce]=useState(0),[visible,setVisible]=useState(!document.hidden)
 const latest=useRef(null),inputKey=`${amount}:${payToken}`,current=value?.key===inputKey?value:null
 useEffect(()=>{
  const timer=setInterval(()=>setNow(Date.now()),1000),visibility=()=>setVisible(!document.hidden)
  document.addEventListener('visibilitychange',visibility)
  return()=>{clearInterval(timer);document.removeEventListener('visibilitychange',visibility)}
 },[])
 useEffect(()=>{
  if(!enabled||paused||!visible){setLoading(false);return}
  try{parseAmount(amount,payToken)}catch{setError('请输入有效金额');setLoading(false);return}
  const controller=new AbortController();let disposed=false
  setError('');setLoading(true)
  const onError=e=>{if(!disposed){setError(e.message||'报价刷新失败');setLoading(false);setNow(Date.now())}}
  void subscribeSelectionQuotes(candidates,amount,payToken,controller.signal,snapshot=>{
   if(disposed)return
   if(latest.current?.key===inputKey&&BigInt(snapshot.sourceBlock)<BigInt(latest.current.snapshot.sourceBlock))throw Error('报价节点落后，保留上次结果')
   const next={key:inputKey,snapshot,decision:selectV4(snapshot)};latest.current=next;setValue(next);setError('');setLoading(false);setNow(Date.now())
  },onError).catch(onError)
  return()=>{disposed=true;controller.abort()}
 },[candidates,amount,payToken,inputKey,enabled,paused,visible,nonce])
 const snapshot=current?.snapshot||null,sourceTime=snapshot?.sourceTimestamp?Number(snapshot.sourceTimestamp)*1000:Infinity
 const ageMs=snapshot?Math.max(0,now-Math.min(Date.parse(snapshot.startedAt),sourceTime)):null
 return {snapshot,decision:current?.decision||null,loading,error,ageMs,stale:!!snapshot&&(!!error||ageMs>QUOTE_STALE_MS),refresh:()=>setNonce(n=>n+1)}
}
