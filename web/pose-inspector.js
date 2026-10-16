// Read-only, demand-driven inspector. Pausing freezes display, not reception.
export function createPoseInspector(parent) {
 const box=document.createElement('section');box.className='pose-inspector';box.hidden=true;box.setAttribute('aria-label','Pose inspector');
 const title=document.createElement('h3'),status=document.createElement('p'),values=document.createElement('pre');
 values.className='mono-id pose-values';
 const raw=document.createElement('details'),summary=document.createElement('summary'),json=document.createElement('pre');
 summary.textContent='Raw observation';json.className=values.className;raw.append(summary,json);
 const pause=document.createElement('button'),close=document.createElement('button'),copy=document.createElement('button');
 pause.textContent='Pause display';close.textContent='Close';copy.textContent='Copy observation';
 for(const button of [pause,close,copy]){button.type='button';button.className='btn btn-secondary btn-sm';}
 const actions=document.createElement('div');actions.className='pose-actions';actions.append(pause,copy,close);
 const log=document.createElement('select'),hint=document.createElement('p');
 log.setAttribute('aria-label','Pose log');log.size=8;log.className='form-control pose-log';
 hint.textContent='Last 300 received poses since inspection opened (not a complete recording). Select a record to freeze it; resume to follow live.';
 box.append(title,actions,status,hint,log,values,raw);parent.append(box);
 let selection=null,generation=0,paused=false,last=null;
 let records=[],latest=null;
 log.onchange=()=>{const record=records[Number(log.value)];if(!record||!latest)return;paused=true;pause.textContent='Resume display';render({...latest,...record,stream:'historical'});};
 pause.onclick=()=>{paused=!paused;pause.textContent=paused?'Resume display':'Pause display';};
 close.onclick=()=>{selection=null;generation++;box.hidden=true;};
 copy.onclick=async()=>{try {await navigator.clipboard.writeText(last||'');} catch {status.textContent='Clipboard unavailable';}};
 function render(data) {
    const observation=data.observation,update=observation.payload;
    if(!data.resolved?.contract||Object.hasOwn(update,'contract'))throw Error('Unsupported pose contract; registry-resolved compact poses required');
    const c=data.resolved.contract,p=update.tracking.pose;
    status.textContent=update.tracking.status+' · '+data.stream+' · '+data.received_hz.toFixed(1)+' Hz received · '+data.age_ms+' ms since latest arrival';
    values.textContent=[
     'Sequence: '+data.sequence,'Capture timestamp (ns): '+data.timestamp_ns,
     p?'Position: '+p.translation.map(v=>v.toFixed(4)).join(', ')+' '+c.to_frame.units:'No valid pose',
     p?'Quaternion (x,y,z,w): '+p.rotation_xyzw.map(v=>v.toFixed(5)).join(', '):'',
     'From: '+c.from_frame.frame_id,'To: '+c.to_frame.frame_id,
     'Lifetime: '+(data.resolved.references?.lifetime||'sampled'),
     'Axes: '+JSON.stringify(c.to_frame.axes),'Handedness: '+c.to_frame.handedness,
     'Session: '+c.session_id,'Clock: '+JSON.stringify(c.clock),
     'Definitions: fetched and verified via SDK Registry v3',
     'Clock definition: '+JSON.stringify(data.resolved.clock_definition),
     'Native coordinates — no Home transform applied.',
     'Arrival age is not capture-to-receive latency.'
    ].filter(Boolean).join('\n');
    last=JSON.stringify({...observation,timestamp_ns:data.timestamp_ns,sequence:data.sequence},null,2);json.textContent=last;
 }
 async function poll(token) {
  if(!selection||token!==generation)return;
  if(parent.closest('[hidden]')||document.hidden){setTimeout(()=>poll(token),500);return;}
  const q=new URLSearchParams({debug_pose:'true',sensor:selection.id,expected_manifest:selection.hash});
  try {
   const response=await fetch('/api/sensors?'+q,{signal:AbortSignal.timeout(12000)}),data=await response.json();
   if(token!==generation)return;
   if(!response.ok)throw Error(data.error||'Pose unavailable');
   if(!paused) {
    latest=data;
    records=(data.history||[]).slice().reverse();
    if(records.some(r=>Object.hasOwn(r.observation.payload,'contract')))throw Error('Unsupported pose contract');
    log.replaceChildren(...records.map((record,index)=>{const option=document.createElement('option');option.value=String(index);option.textContent='#'+record.sequence+' · '+record.observation.payload.tracking.status+' · '+record.timestamp_ns;return option;}));
    render(data);
   }
  } catch(error) {if(token===generation&&!paused){status.textContent='Stream unavailable · '+error.message;values.textContent='Waiting for a fresh pose. Previously displayed data is not current.';}}
  if(token===generation&&selection)setTimeout(()=>poll(token),250);
 }
 return (peer,product,label)=>{
  selection={id:peer+'/'+product.manifest.product_id,hash:product.manifest_hash};generation++;paused=false;last=null;
  records=[];latest=null;log.replaceChildren();
  box.hidden=false;title.textContent='Pose inspector · '+label;status.textContent='Connecting…';values.textContent='';json.textContent='';pause.textContent='Pause display';
  poll(generation);
 };
}
