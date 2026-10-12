// Translate both the camera and orbit target: free movement without changing aim.
export function isEditing(target) {
 return !!target && (target.isContentEditable || !!target.closest?.('input, textarea, select, [contenteditable]:not([contenteditable="false"]), [role="textbox"]'));
}
export function ghostStep(keys, forward, right, dt, speed=3) {
 const f=Number(keys.has('KeyW'))-Number(keys.has('KeyS'));
 const r=Number(keys.has('KeyD'))-Number(keys.has('KeyA'));
 const up=Number(keys.has('KeyQ'))-Number(keys.has('KeyE'));
 const vector=forward.map((v,i)=>v*f+right[i]*r+(i===1?up:0));
 const norm=Math.hypot(...vector)||1;
 const boost=keys.has('ShiftLeft')||keys.has('ShiftRight')?3:1;
 const distance=Math.max(0,Math.min(dt,.05))*speed*boost/norm;
 return vector.map(v=>v*distance||0);
}
export function lookDirection(forward,dx,dy,sensitivity=.003) {
 const yaw=Math.atan2(forward[0],-forward[2])+dx*sensitivity;
 const pitch=Math.max(-Math.PI/2+.01,Math.min(Math.PI/2-.01,Math.asin(Math.max(-1,Math.min(1,forward[1])))-dy*sensitivity));
 return [Math.sin(yaw)*Math.cos(pitch),Math.sin(pitch),-Math.cos(yaw)*Math.cos(pitch)];
}
export function createGhostCamera({camera,controls,enabled,canvas,host=window,doc=document}) {
 const keys=new Set();let last=null;
 const looking=()=>['KeyW','KeyA','KeyS','KeyD'].some(key=>keys.has(key));
 const originalRotate=controls.enableRotate;
 const originalMin=controls.minPolarAngle,originalMax=controls.maxPolarAngle;
 let orbitDrag=false,ghostDrag=false,freeLook=false;
 const syncMode=()=>{
  const next=looking()&&!orbitDrag&&!blocked();
  if(next===freeLook)return;
  freeLook=next;controls.enableRotate=next?false:originalRotate;
  if(next){controls.minPolarAngle=.01;controls.maxPolarAngle=Math.PI-.01;}
  else {
   // Keep the current aim on release, even if free-look looked above the
   // old orbit horizon. Restoring a narrower bound immediately would snap.
   const forward=camera.getWorldDirection(camera.position.clone());
   const polar=Math.acos(Math.max(-1,Math.min(1,-forward.y)));
   controls.minPolarAngle=Math.min(originalMin??0,polar);
   controls.maxPolarAngle=Math.max(originalMax??Math.PI,polar);
  }
 };
 const reset=()=>{keys.clear();last=null;syncMode();};
 const blocked=e=>!enabled()||!controls.enabled||doc.hidden||isEditing(doc.activeElement)||e?.isComposing||e?.ctrlKey||e?.metaKey||e?.altKey||e?.composedPath?.().some(isEditing);
 const aim=(dx,dy)=>{
  const direction=lookDirection(camera.getWorldDirection(camera.position.clone()).toArray(),dx,dy);
  const distance=Math.max(.1,camera.position.distanceTo(controls.target));
  controls.target.copy(camera.position).add(camera.position.clone().set(...direction).multiplyScalar(distance));
  camera.lookAt(controls.target);
 };
 host.addEventListener('keydown',e=>{
  if(blocked(e)){reset();return;}
  if(!['KeyW','KeyA','KeyS','KeyD','KeyQ','KeyE','ShiftLeft','ShiftRight'].includes(e.code))return;
  if(!e.code.startsWith('Shift'))e.preventDefault();keys.add(e.code);syncMode();
 },true);
 host.addEventListener('keyup',e=>{keys.delete(e.code);syncMode();},true);
 host.addEventListener('blur',()=>{orbitDrag=false;ghostDrag=false;reset();});
 doc.addEventListener('focusin',reset,true);
 doc.addEventListener('visibilitychange',reset);
 canvas?.addEventListener('pointerdown',e=>{
  if(isEditing(doc.activeElement))doc.activeElement.blur?.();
  if(blocked())return;
  if(looking()&&!orbitDrag){ghostDrag=true;canvas.setPointerCapture?.(e.pointerId);e.preventDefault();e.stopImmediatePropagation();}
  else orbitDrag=true;
  syncMode();
 },true);
 canvas?.addEventListener('pointermove',e=>{
  if(ghostDrag)e.stopImmediatePropagation();
  if(blocked(e)||!looking()||orbitDrag||e.pointerType==='touch')return;
  syncMode();
  aim(e.movementX||0,e.movementY||0);
 },true);
 const endDrag=e=>{if(ghostDrag){e.preventDefault();e.stopImmediatePropagation();if(canvas.hasPointerCapture?.(e.pointerId))canvas.releasePointerCapture(e.pointerId);}ghostDrag=false;orbitDrag=false;syncMode();};
 canvas?.addEventListener('pointerup',endDrag,true);
 canvas?.addEventListener('pointercancel',endDrag,true);
 return {tick(now){
  if(blocked()){reset();return;}
  syncMode();
  const dt=last===null?0:(now-last)/1000;last=now;
  if(!keys.size)return;
  const forward=camera.getWorldDirection(camera.position.clone());
  const right=camera.position.clone().set(1,0,0).applyQuaternion(camera.quaternion);
  const delta=ghostStep(keys,forward.toArray(),right.toArray(),dt);
  camera.position.add(camera.position.clone().set(...delta));controls.target.add(camera.position.clone().set(...delta));
 },reset};
}
