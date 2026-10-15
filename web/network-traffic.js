export function trafficRate(previous, current) {
 const seconds=(current.elapsed_ms-previous.elapsed_ms)/1000;
 if(previous.session!==current.session||seconds<=0||current.received_bytes<previous.received_bytes||current.sent_bytes<previous.sent_bytes)return null;
 return {received:(current.received_bytes-previous.received_bytes)/seconds,sent:(current.sent_bytes-previous.sent_bytes)/seconds};
}
function bytes(value){const units=['B','KiB','MiB','GiB'];let unit=0;while(value>=1024&&unit<3){value/=1024;unit++;}return `${value.toFixed(unit?1:0)} ${units[unit]}`;}
export function createNetworkTraffic() {
 let selectedPeer=null,selectedName='',returnFocus=null;
 const panel=document.createElement('section');panel.id='network-traffic';panel.hidden=true;panel.setAttribute('aria-label','Network traffic');
 panel.innerHTML='<header><strong tabindex="0" role="button" aria-label="Move network traffic panel">Network traffic</strong><button type="button" aria-label="Close network traffic">×</button></header><div class="traffic-content"><p class="traffic-status" role="status">Waiting for sample…</p><div class="traffic-rates"><div>↓ Peer → Home<strong data-rate="received">—</strong><small data-total="received">—</small></div><div>↑ Home → Peer<strong data-rate="sent">—</strong><small data-total="sent">—</small></div></div><svg viewBox="0 0 320 80" role="img" aria-label="Recent receive and send rates"><polyline class="traffic-rx"/><polyline class="traffic-tx"/></svg><div class="traffic-legend"><span>↓ Peer → Home</span><span>↑ Home → Peer</span><span>Last 60 samples</span></div><p class="traffic-scope">Measured by Home for this authenticated peer pair. Bytes accepted by the stream are not proof that the phone decoded or displayed voxels. Excludes HTTP, authentication and transport overhead.</p></div>';
 document.body.append(panel);
 const identity=document.createElement('p');identity.className='traffic-peer-identity';panel.querySelector('.traffic-status').after(identity);
 let ceiling=100,lastRate=null;
 try{const saved=Number(localStorage.getItem('home.networkTraffic.scale'));if([1,10,100,1000,10000].includes(saved))ceiling=saved;}catch{}
 const scaleLabel=document.createElement('label');scaleLabel.className='traffic-scale';scaleLabel.textContent='Gauge range ';const scale=document.createElement('select');scale.setAttribute('aria-label','Gauge maximum');
 for(const value of [1,10,100,1000,10000])scale.add(new Option(value>=1000?`${value/1000} Gbps`:`${value} Mbps`,String(value)));
 scale.value=String(ceiling);scaleLabel.append(scale);panel.querySelector('.traffic-rates').before(scaleLabel);
 for(const key of ['received','sent']){
  const rate=panel.querySelector(`[data-rate="${key}"]`),dial=document.createElement('div');dial.className='traffic-dial';
  dial.innerHTML=`<svg viewBox="0 0 160 100" role="img" aria-label="${key} speedometer"><path class="dial-track" d="M 18 80 A 62 62 0 0 1 142 80"/><path class="dial-fill" pathLength="100" d="M 18 80 A 62 62 0 0 1 142 80"/><path class="dial-needle" d="M 80 80 L 27 80"/><circle cx="80" cy="80" r="4" fill="currentColor"/><text x="12" y="98">0</text><text class="dial-max" x="148" y="98" text-anchor="end"></text></svg><span class="dial-value">—</span>`;
  rate.before(dial);
 }
 function gauges(rate){lastRate=rate;for(const key of ['received','sent']){
  const dial=panel.querySelector(`[data-rate="${key}"]`).previousElementSibling,mbps=rate?rate[key]*8/1e6:null;
  const fraction=mbps===null?0:Math.min(1,mbps/ceiling);
  dial.querySelector('.dial-fill').style.strokeDasharray=`${fraction*100} 100`;
  dial.querySelector('.dial-needle').style.transform=`rotate(${fraction*180}deg)`;
  dial.querySelector('.dial-needle').style.visibility=mbps===null?'hidden':'visible';
  dial.querySelector('.dial-max').textContent=ceiling+' Mbps';
  dial.querySelector('.dial-value').textContent=mbps===null?'—':`${mbps.toFixed(mbps<1?3:1)} Mbps${mbps>ceiling?' · over range':''}`;
  dial.querySelector('svg').setAttribute('aria-label',`${key}: ${mbps===null?'unavailable':mbps.toFixed(3)+' Mbps'}, range 0 to ${ceiling} Mbps`);
 }}
 scale.onchange=()=>{ceiling=Number(scale.value);try{localStorage.setItem('home.networkTraffic.scale',String(ceiling));}catch{}gauges(lastRate);};gauges(null);
 panel.querySelector('.traffic-scope').append(' Gauge range is a display scale, not measured link capacity.');
 const diagnostics=document.createElement('section');diagnostics.className='traffic-diagnostics';
 diagnostics.innerHTML='<div class="traffic-toolbar"><h3>Launch diagnostics</h3><button type="button" class="traffic-export">Export JSON</button></div><p class="traffic-recording"></p><label class="traffic-compare">Compare this launch with <select aria-label="Compare launch"></select></label><div class="traffic-table-wrap"><table class="traffic-comparison"><thead><tr><th>Metric</th><th>This launch</th><th>Selected launch</th><th>Change</th></tr></thead><tbody></tbody></table></div><details open><summary>Launch leaderboard · peak receive</summary><div class="traffic-table-wrap"><table class="traffic-leaderboard"><thead><tr><th>Launch</th><th>Peak ↓</th><th>Peak ↑</th><th>Total ↓ / ↑</th><th>Duration</th></tr></thead><tbody></tbody></table></div></details><details><summary>Top traffic intervals · this launch</summary><div class="traffic-table-wrap"><table class="traffic-bursts"><thead><tr><th>Since launch</th><th>Window</th><th>Receive</th><th>Send</th></tr></thead><tbody></tbody></table></div></details><p class="traffic-retention">Server records every second, even with this panel closed. Last 20 launches retained; last 60 samples and top 10 intervals per peer pair. Older aggregate-only launches are excluded. I/O errors are not packet-loss measurements. Peaks are interval averages, not instantaneous wire speeds.</p>';
 panel.querySelector('.traffic-scope').before(diagnostics);
 let latest=null,selected='';const comparison=diagnostics.querySelector('select');
 const speed=value=>`${(value*8/1e6).toFixed(3)} Mbps`;
 const duration=ms=>{const seconds=Math.floor(ms/1000);return `${Math.floor(seconds/3600)}h ${Math.floor(seconds/60)%60}m ${seconds%60}s`;};
 const launchName=run=>new Date(run.started_at).toLocaleString();
 function rows(table,values){const body=diagnostics.querySelector(table+' tbody');body.replaceChildren();for(const valuesRow of values){const row=body.insertRow();for(const value of valuesRow)row.insertCell().textContent=String(value);}}
 function renderDiagnostics(data){
  latest=data;const current=data.current,prior=data.prior;
  if(!prior.some(run=>run.session===selected))selected=prior[0]?.session||'';
  comparison.replaceChildren(...prior.map((run,i)=>new Option(`${i===0?'Previous launch · ':''}${launchName(run)}`,run.session)));
  if(!prior.length)comparison.add(new Option('No prior recorded launches',''));
  comparison.value=selected;comparison.disabled=!prior.length;
  const baseline=prior.find(run=>run.session===selected);
  diagnostics.querySelector('.traffic-recording').textContent=data.persistence_error||`Started ${launchName(current)} · ${duration(current.elapsed_ms)} uptime · ${data.saved_at?'saved '+new Date(data.saved_at).toLocaleTimeString():'awaiting first save'}`;
  diagnostics.querySelector('.traffic-recording').classList.toggle('traffic-warning',Boolean(data.persistence_error));
  const metrics=[['Peak receive',r=>r.peak_received.received,speed],['Peak send',r=>r.peak_sent.sent,speed],['Average receive',r=>r.elapsed_ms?r.received_bytes*1000/r.elapsed_ms:0,speed],['Average send',r=>r.elapsed_ms?r.sent_bytes*1000/r.elapsed_ms:0,speed],['Received',r=>r.received_bytes,bytes],['Sent',r=>r.sent_bytes,bytes],['Read I/O errors',r=>r.read_errors||0,String],['Write I/O errors',r=>r.write_errors||0,String],['Streams opened',r=>r.opened_streams||0,String],['Active streams',r=>r.active_streams||0,String],['Observed duration',r=>r.elapsed_ms,duration]];
  rows('.traffic-comparison',metrics.map(([name,value,format])=>{const now=value(current),before=baseline?value(baseline):null;return [name,format(now),baseline?format(before):'—',before===null?'—':before===0?(now===0?'0%':'no baseline'):((now-before)/before*100).toFixed(1)+'%'];}));
  rows('.traffic-leaderboard',[current,...prior].sort((a,b)=>b.peak_received.received-a.peak_received.received).map(run=>[run.session===current.session?'This launch':launchName(run)+(run.ended_cleanly?'':' · last checkpoint'),speed(run.peak_received.received),speed(run.peak_sent.sent),bytes(run.received_bytes)+' / '+bytes(run.sent_bytes),duration(run.elapsed_ms)]));
  rows('.traffic-bursts',current.top_intervals.map(point=>[duration(point.elapsed_ms),(point.interval_ms/1000).toFixed(2)+' s',speed(point.received),speed(point.sent)]));
  if(!current.top_intervals.length)rows('.traffic-bursts',[['No traffic recorded yet','—','—','—']]);
 }
 comparison.onchange=()=>{selected=comparison.value;if(latest)renderDiagnostics(latest);};
 diagnostics.querySelector('.traffic-export').onclick=()=>{if(!latest)return;const url=URL.createObjectURL(new Blob([JSON.stringify(latest,null,2)],{type:'application/json'}));const a=document.createElement('a');a.href=url;a.download=`home-traffic-${latest.current.session}.json`;a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);};
 const status=panel.querySelector('.traffic-status');let previous=null,history=[],timer=null,controller=null,generation=0;
 function render(current,rate){
  gauges(rate);
  for(const key of ['received','sent']){panel.querySelector(`[data-rate="${key}"]`).textContent=rate?bytes(rate[key])+'/s':'—';panel.querySelector(`[data-total="${key}"]`).textContent=bytes(current[key+'_bytes'])+' total';}
  if(current.diagnostics){renderDiagnostics(current.diagnostics);history=current.diagnostics.current.samples;}
  else {if(rate)history.push(rate);history=history.slice(-60);}
  const legend=panel.querySelector('.traffic-legend span:last-child');
  legend.textContent=history.length?`${duration(history[0].elapsed_ms||0)}–${duration(history.at(-1).elapsed_ms||0)} · since launch`:'Waiting for history';
  const max=Math.max(1,...history.flatMap(item=>[item.received,item.sent]));
  for(const [key,cls] of [['received','rx'],['sent','tx']])panel.querySelector('.traffic-'+cls).setAttribute('points',history.map((item,i)=>`${history.length<2?320:i*320/(history.length-1)},${78-item[key]/max*76}`).join(' '));
 }
 async function sample(id){
  const request=new AbortController();controller=request;const timeout=setTimeout(()=>request.abort(),5000);
  try{
   const response=await fetch('/api/network-traffic?peer_id='+encodeURIComponent(selectedPeer),{signal:request.signal,cache:'no-store'});if(!response.ok)throw Error('unavailable');
   const current=await response.json();
   if(id!==generation)return;
   if(current.available===false){previous=null;gauges(null);status.textContent=current.reason||'No measured traffic for this peer';for(const el of panel.querySelectorAll('[data-rate],[data-total]'))el.textContent='—';return;}
   if(current.remote_peer_id&&current.remote_peer_id!==selectedPeer)throw Error('Peer identity mismatch');
   identity.textContent=`Home ${current.observer_peer_id||'unknown'} ↔ ${selectedName} ${selectedPeer}`;
   if(typeof current.session!=='string'||!['elapsed_ms','received_bytes','sent_bytes'].every(key=>Number.isFinite(current[key])&&current[key]>=0))throw Error('invalid');
   if(id!==generation)return;
   const rate=current.diagnostics?.current.samples.at(-1)||(previous?trafficRate(previous,current):null);if(!rate)history=[];
   render(current,rate);previous=current;status.textContent=rate?'Live · updates every second':'Collecting first rate sample…';
  }catch{
   if(id!==generation)return;previous=null;gauges(null);status.textContent='Traffic unavailable · retrying…';
   for(const key of ['received','sent'])panel.querySelector(`[data-rate="${key}"]`).textContent='—';
  }finally{clearTimeout(timeout);if(id===generation)timer=setTimeout(()=>sample(id),1000);}
 }
 function show(visible){
  generation++;clearTimeout(timer);controller?.abort();previous=null;history=[];panel.hidden=!visible;

  if(visible){gauges(null);for(const el of panel.querySelectorAll('[data-rate],[data-total]'))el.textContent='—';for(const el of panel.querySelectorAll('polyline'))el.setAttribute('points','');status.textContent='Waiting for sample…';sample(generation);fit();}
 }
 panel.querySelector('button').onclick=()=>{show(false);returnFocus?.focus();};
 // Header dragging and arrow keys keep the panel within the viewport.
 const handle=panel.querySelector('strong');let drag=null;
 function place(x,y){panel.style.left=Math.max(8,Math.min(x,innerWidth-panel.offsetWidth-8))+'px';panel.style.top=Math.max(8,Math.min(y,innerHeight-panel.offsetHeight-8))+'px';panel.style.right='auto';}
 function fit(){if(!panel.hidden){const r=panel.getBoundingClientRect();place(r.left,r.top);}}
 handle.onpointerdown=e=>{if(e.button!==0)return;e.preventDefault();const r=panel.getBoundingClientRect();drag={x:e.clientX-r.left,y:e.clientY-r.top};handle.setPointerCapture(e.pointerId);};
 handle.onpointermove=e=>{if(drag)place(e.clientX-drag.x,e.clientY-drag.y);};
 for(const event of ['pointerup','pointercancel','lostpointercapture'])handle.addEventListener(event,()=>{drag=null;});
 handle.onkeydown=e=>{const moves={ArrowLeft:[-10,0],ArrowRight:[10,0],ArrowUp:[0,-10],ArrowDown:[0,10]};if(!moves[e.key])return;e.preventDefault();const r=panel.getBoundingClientRect(),[x,y]=moves[e.key];place(r.left+x,r.top+y);};
 panel.addEventListener('keydown',e=>{if(e.key==='Escape'){show(false);returnFocus?.focus();}});
 window.addEventListener('resize',fit);
 return (peerId,name,trigger)=>{
  selectedPeer=peerId;selectedName=name||peerId;returnFocus=trigger||null;latest=null;selected='';
  panel.querySelector('header strong').textContent=`Traffic · ${selectedName} ↔ Home`;
  identity.textContent=`Peer ${peerId} · measured by Home`;
  for(const table of diagnostics.querySelectorAll('tbody'))table.replaceChildren();comparison.replaceChildren();
  diagnostics.querySelector('.traffic-recording').textContent='Waiting for peer measurements…';
  show(true);
 };
}
