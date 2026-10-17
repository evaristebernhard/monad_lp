// One persistent response avoids a public HTTP round trip for every refresh.
export async function subscribeJson(url,{body,signal,onValue,onError}){
 while(!signal.aborted){
  let reader
  try{
   const response=await fetch(url,{method:body===undefined?'GET':'POST',headers:body===undefined?undefined:{'Content-Type':'application/json'},body:body===undefined?undefined:JSON.stringify(body),signal,cache:'no-store'})
   if(!response.ok||!response.body)throw Error('实时连接不可用')
   reader=response.body.getReader();const decoder=new TextDecoder();let buffer=''
   for(;;){
    const {value,done}=await reader.read();if(done)throw Error('实时连接中断')
    buffer+=decoder.decode(value,{stream:true});const frames=buffer.split('\n\n');buffer=frames.pop()
    const messages=frames.map(frame=>frame.split('\n').filter(line=>line.startsWith('data:')).map(line=>line.slice(5).trim()).join('\n')).filter(Boolean)
    // If the browser was delayed, consume the newest complete snapshot.
    if(messages.length&&!signal.aborted)await onValue(JSON.parse(messages.at(-1)))
   }
  }catch(e){if(!signal.aborted)onError(e)}
  finally{if(reader){try{await reader.cancel()}catch{}reader.releaseLock()}}
  if(!signal.aborted)await new Promise(resolve=>{const abort=()=>{clearTimeout(timer);resolve()},timer=setTimeout(()=>{signal.removeEventListener('abort',abort);resolve()},1000);signal.addEventListener('abort',abort,{once:true})})
 }
}
