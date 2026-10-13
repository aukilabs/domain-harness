// A live, browser-local viewpoint driven only by measured SDK sensor poses.
export function createAttachedCamera({host,camera,controls,map2D,context,request}) {
 const panel=document.createElement('div');panel.hidden=true;panel.setAttribute('aria-label','Attached camera');
 panel.style.cssText='position:absolute;top:12px;left:50%;transform:translateX(-50%);z-index:91;background:#171b23;color:#dce6f4;padding:8px 12px;border:1px solid #39485c;border-radius:6px;font:12px system-ui';
 const status=document.createElement('span'),button=document.createElement('button');button.textContent='Detach';button.type='button';panel.append(status,button);host.append(panel);
 button.style.marginLeft='12px';
 let binding=null,generation=0,busy=false,pending=null,lastStamp=null,freshAt=0;
 function detach(){generation++;pending?.abort();pending=null;binding=null;busy=false;panel.hidden=true;
  camera.up.set(0,1,0);controls.enabled=!map2D.isActive();controls.minDistance=.01;controls.maxPolarAngle=Math.PI;
  return 'Map camera detached.';
 }
 button.onclick=detach;
 window.addEventListener('keydown',e=>{if(e.key==='Escape'&&binding)detach();});
 const vector=v=>Array.isArray(v)&&v.length===3&&v.every(Number.isFinite);
 async function poll(){
  if(!binding)return;
  const current=context();
  if(current.scene!==binding.scene||current.frame!==binding.frame){detach();return;}
  if(freshAt&&performance.now()-freshAt>2000)status.textContent='Paused · sensor poses are stale';
  if(busy)return;
  busy=true;const epoch=generation,selected=binding;pending=new AbortController();
  try {
   const sample=await request(selected,pending.signal);
   if(epoch!==generation)return;
   const current=context();
   if(current.scene!==selected.scene||current.frame!==selected.frame){detach();return;}
   if(sample.frame!==selected.frame||sample.sensor_id!==selected.sensor_id||sample.manifest_hash!==selected.pose_product.manifest_hash
     ||JSON.stringify(sample.source_frame)!==JSON.stringify(selected.source_frame))throw Error('Sensor frame or scene changed; attach again');
   if(!vector(sample.position)||!vector(sample.look_at)||!vector(sample.up)||!Number.isFinite(sample.age_ms)||sample.age_ms<0||sample.age_ms>=2000)throw Error('Fresh sensor pose unavailable');
   const forward=sample.look_at.map((n,i)=>n-sample.position[i]);
   if(Math.abs(Math.hypot(...forward)-1)>1e-5||Math.abs(Math.hypot(...sample.up)-1)>1e-5||Math.abs(forward.reduce((sum,n,i)=>sum+n*sample.up[i],0))>1e-5)throw Error('Invalid sensor orientation');
   const stamp=JSON.stringify([sample.clock,sample.sequence,sample.capture_timestamp_ns]);
   if(stamp!==lastStamp){lastStamp=stamp;freshAt=performance.now()-sample.age_ms;}
   if(!freshAt||performance.now()-freshAt>2000)throw Error('Sensor poses are stale');
   camera.position.fromArray(sample.position);camera.up.fromArray(sample.up);controls.target.fromArray(sample.look_at);
   camera.lookAt(controls.target);camera.updateMatrixWorld();
   status.textContent='Following · '+(selected.alias||selected.peer_id)+' · '+selected.frame_id;
  } catch(error){if(epoch===generation)status.textContent='Paused · '+error.message;}
  finally {if(epoch===generation){busy=false;pending=null;}}
 }
 setInterval(poll,200);
 return {isActive:()=>binding!==null,detach,attach(action){
  const current=context();
  if(action.scene!==current.scene||action.frame!==current.frame||!action.sensor_id||!action.pose_product?.manifest_hash||!action.source_frame)throw Error('Selected scene or sensor changed; ask again');
  detach();if(map2D.isActive())map2D.toggle();
  // Flush orbit momentum once; then OrbitControls must not overwrite sensor roll.
  const damping=controls.enableDamping;controls.enableDamping=false;controls.update();controls.enableDamping=damping;
  controls.enabled=false;binding=action;lastStamp=null;freshAt=0;panel.hidden=false;status.textContent='Waiting for fresh sensor pose…';poll();
  return 'Camera attachment requested · waiting for a live sensor pose.';
 }};
}
