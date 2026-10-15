import { createCameraRig } from './camera-rig.js';
// Translate both the camera and orbit target: free movement without changing aim.
export function isEditing(target) {
 return !!target && (target.isContentEditable || !!target.closest?.('input, textarea, select, [contenteditable]:not([contenteditable="false"]), [role="textbox"]'));
}
// Animate position and orbit target together; manual input may cancel at any time.
export function createCameraPan({camera,controls,duration=700,now=()=>performance.now()}) {
 let flight=null;
 return {
  cancel(){flight=null;},
  start(position,target){
   const damping=controls.enableDamping;controls.enableDamping=false;controls.update();controls.enableDamping=damping;
   flight={began:now(),from:camera.position.clone(),targetFrom:controls.target.clone(),position:position.clone(),target:target.clone()};
  },
  tick(time){
   if(!flight)return false;
   const t=Math.min(1,Math.max(0,(time-flight.began)/duration)),ease=t*t*(3-2*t);
   camera.position.copy(flight.from).lerp(flight.position,ease);
   controls.target.copy(flight.targetFrom).lerp(flight.target,ease);
   camera.lookAt(controls.target);
   if(t===1)flight=null;
   return true;
  }
 };
}
// Keyboard movement over the ground: W/S along the camera's heading (pitch ignored, so looking down never
// turns W into a zoom), A/D strafe, Space/Shift rise and sink. Opposites cancel; diagonals are normalized.
// `heading` is the camera's forward direction projected onto the ground, as [x, z].
// Keys that move focus or act on it: after them, focus is the keyboard's.
const FOCUS_KEYS=['Tab','Enter','Escape','ArrowUp','ArrowDown','ArrowLeft','ArrowRight','Home','End'];
export const MOVE_KEYS=['KeyW','KeyA','KeyS','KeyD','Space','ShiftLeft','ShiftRight','KeyQ','KeyE'];
export function groundDirection(keys, heading) {
 const f=Number(keys.has('KeyW'))-Number(keys.has('KeyS'));
 const r=Number(keys.has('KeyD'))-Number(keys.has('KeyA'));
 const up=Number(keys.has('Space'))-Number(keys.has('ShiftLeft')||keys.has('ShiftRight'));
 const length=Math.hypot(...heading)||1,hx=heading[0]/length,hz=heading[1]/length;
 const vector=[hx*f-hz*r,up,hz*f+hx*r];
 const norm=Math.hypot(...vector);
 return norm?vector.map(v=>v/norm):[0,0,0];
}
// Metres per second: slow up close for precise placement, fast when zoomed out over a whole store.
export const moveSpeed=distance=>Math.min(25,Math.max(1,.8*distance));
export const TURN_SPEED=Math.PI/2; // Q/E: 90° per second around the target.
// Moves go through the camera rig (camera-rig.js), which keeps them within the floor and orbit limits; without
// one, a rig over `floor` is made here.
export function createGhostCamera({camera,controls,enabled,canvas,host=window,doc=document,floor=()=>-Infinity,rig=createCameraRig({camera,controls,floor}),onMove=()=>{}}) {
 const keys=new Set(),velocity=[0,0,0];let last=null,pointerHeld=false,pointerFocus=false;
 const blocked=e=>!enabled()||!controls.enabled||doc.hidden||isEditing(doc.activeElement)||e?.isComposing||e?.ctrlKey||e?.metaKey||e?.altKey||e?.composedPath?.().some(isEditing);
 const reset=()=>{keys.clear();velocity.fill(0);last=null;};
 host.addEventListener('keydown',e=>{
  // Keyboard navigation (Tab, arrows, Enter, Escape) makes focus the keyboard's: Space/Shift go back to the
  // focused control. ⌘, Ctrl and Alt start a shortcut and stop movement; on macOS no keyup arrives for keys let go
  // while ⌘ is down, so a held W would otherwise run on by itself.
  if(!MOVE_KEYS.includes(e.code)){
   if(FOCUS_KEYS.includes(e.key)){pointerFocus=false;for(const code of ['Space','ShiftLeft','ShiftRight'])keys.delete(code);}
   if(['Meta','Control','Alt'].includes(e.key))reset();
   return;
  }
  if(blocked(e)){reset();return;}
  // Space and Shift move the camera unless a control was focused from the keyboard: there they keep their usual
  // jobs (Space activates, Shift+Tab and Shift+Arrow). A control merely left focused by a click (a button, a
  // panel tab, a card) doesn't keep them, or Space silently stops working beside WASD. Shift with the mouse held
  // belongs to the click; Space has no such job, so it still rises while dragging to look around.
  const free=!doc.activeElement||doc.activeElement===doc.body||doc.activeElement===canvas||pointerFocus;
  if((e.code==='Space'||e.code.startsWith('Shift'))&&!free||e.code.startsWith('Shift')&&pointerHeld)return;
  e.preventDefault();if(!keys.size)onMove();keys.add(e.code);
 },true);
 // A captured Space also skips its keyup, which would otherwise click a button clicked earlier.
 host.addEventListener('keyup',e=>{if(e.code==='Space'&&keys.has(e.code))e.preventDefault();keys.delete(e.code);},true);
 host.addEventListener('blur',reset);
 doc.addEventListener('focusin',e=>{if(isEditing(e.target))reset();},true);
 doc.addEventListener('visibilitychange',reset);
 canvas?.addEventListener('pointerdown',()=>{pointerHeld=true;if(isEditing(doc.activeElement))doc.activeElement.blur?.();},true);
 host.addEventListener('pointerdown',()=>{pointerFocus=true;},true);
 // A cancelled press (a system gesture, a touch turned scroll) never sends pointerup.
 for(const type of ['pointerup','pointercancel'])host.addEventListener(type,()=>{pointerHeld=false;},true);
 return {tick(now){
  if(blocked()){reset();return false;}
  const dt=last===null?0:Math.min(.05,(now-last)/1000);last=now;
  // Idle: nothing held and nothing still coasting, so skip the per-frame vector work.
  if(!keys.size&&!velocity[0]&&!velocity[1]&&!velocity[2])return false;
  const distance=camera.position.distanceTo(controls.target);
  // Turning: orbit the camera around the target about the vertical.
  const turn=Number(keys.has('KeyE'))-Number(keys.has('KeyQ'));
  if(turn&&dt){const offset=camera.position.clone().sub(controls.target).applyAxisAngle(camera.up,-turn*TURN_SPEED*dt);camera.position.copy(controls.target).add(offset);camera.lookAt(controls.target);}
  // Moving: ease towards the wanted velocity (about 150 ms up to speed, a quick settle on release).
  // Heading: forward on the ground. Straight down, forward has no ground part, but the screen's up does, so
  // the two together are always defined.
  // Screen-up points along the heading when looking down and against it when looking up, hence the sign.
  const forward=camera.getWorldDirection(camera.position.clone()),screenUp=camera.up.clone().set(0,1,0).applyQuaternion(camera.quaternion),sign=forward.y>0?-1:1;
  const wanted=groundDirection(keys,[forward.x+sign*screenUp.x,forward.z+sign*screenUp.z]).map(v=>v*moveSpeed(distance));
  const ease=1-Math.exp(-dt*(keys.size?16:22));
  for(let i=0;i<3;i++)velocity[i]+=(wanted[i]-velocity[i])*ease;
  if(Math.hypot(...velocity)<.002){velocity.fill(0);return !!turn;}
  // The rig stops a sink at the floor; dropping the downward speed there means no coast to wait out on Space.
  if(rig.translate(camera.position.clone().set(...velocity).multiplyScalar(dt))&&velocity[1]<0)velocity[1]=0;
  return true;
 },reset,
 // Shift is held to sink (not as a modifier for a click or drag).
 sinking:()=>keys.has('ShiftLeft')||keys.has('ShiftRight')};
}
