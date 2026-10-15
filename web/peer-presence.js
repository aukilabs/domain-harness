// Independent, bounded lookups. A slow peer never gates another peer's result.
// Context and Product identities fence late responses after scene/session changes.
export function createPeerPresence({read, changed, now=()=>performance.now(), timeoutMs=2000, maxConcurrent=8}) {
 let context=null, states=new Map(), active=0;
 function cancel(state){state.controller?.abort();}
 function emit(){changed([...states.values()].map(({target,value,error,receivedAt})=>({target,value,error,receivedAt})));}
 function update(nextContext, targets){
  if(context!==nextContext){for(const state of states.values())cancel(state);states.clear();context=nextContext;}
  const wanted=new Map(targets.map(target=>[target.key,target]));
  for(const [key,state] of states)if(!wanted.has(key)){cancel(state);states.delete(key);}
  for(const [key,target] of wanted){if(!states.has(key))states.set(key,{target,due:-Infinity});else states.get(key).target=target;}
  // Oldest due first: a large fleet cannot starve behind the first eight peers.
  for(const state of [...states.values()].sort((a,b)=>a.due-b.due)){
   if(active>=maxConcurrent)break;
   if(state.controller||state.due>now())continue;
   const controller=new AbortController();state.controller=controller;active++;
   const requestedContext=context,target=state.target;let timer;
   const timeout=new Promise((_,reject)=>{timer=setTimeout(()=>{controller.abort();reject(new Error('Pose lookup timed out'));},timeoutMs);});
   Promise.race([Promise.resolve().then(()=>read(target,controller.signal)),timeout]).then(value=>{
    if(context===requestedContext&&states.get(target.key)===state){state.value=value;state.error=null;state.receivedAt=now();}
   },error=>{
    if(context===requestedContext&&states.get(target.key)===state){state.value=null;state.error=error.message;}
   }).finally(()=>{
    clearTimeout(timer);active--;state.controller=null;state.due=now()+(target.intervalMs??200);
    if(context===requestedContext&&states.get(target.key)===state)emit();
   });
  }
  emit(); // Also ages out stale results and removes disconnected peers.
 }
 return {update};
}
