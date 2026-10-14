import * as THREE from 'three';

export const PHONE_TRAIL_MS = 10000;
export class PhoneTrailHistory {
 constructor(){this.frame=null;this.peers=new Map();}
 reset(frame){this.frame=frame;this.peers.clear();}
 ingest(frame,poses,now){
  if(frame!==this.frame)this.reset(frame);
  for(const sample of poses){
   const {peer_id:peer,clock,pose,capture_timestamp_ns:stamp}=sample;
   if(!peer||!clock?.session_id||pose?.to_frame_id!==frame||!(typeof stamp==='string'?/^\d+$/.test(stamp):Number.isSafeInteger(stamp))||
      !Array.isArray(pose.translation)||pose.translation.length!==3||!pose.translation.every(Number.isFinite)||
      !Array.isArray(pose.rotation_wxyz)||pose.rotation_wxyz.length!==4||!pose.rotation_wxyz.every(Number.isFinite))continue;
   let state=this.peers.get(peer);
   if(!state||state.session!==clock.session_id){
    if(!state&&this.peers.size>=64)continue;
    state={session:clock.session_id,last:-1,entries:[]};this.peers.set(peer,state);
   }
   // Duplicate polling must not extend a ghost's life; reject out-of-order samples.
   if(BigInt(stamp)<=BigInt(state.last))continue;
   state.last=stamp;
   const age=Math.max(0,Number(sample.received_age_ms)||0);
   if(age>=PHONE_TRAIL_MS)continue;
   state.entries.push({key:JSON.stringify([peer,clock.session_id,stamp]),peer,pose,born:now-age});
   if(state.entries.length>50)state.entries.shift();
  }
  return this.samples(now);
 }
 samples(now){
  for(const state of this.peers.values())state.entries=state.entries.filter(e=>now-e.born<PHONE_TRAIL_MS);
  const entries=[...this.peers.values()].flatMap(s=>s.entries).sort((a,b)=>a.born-b.born);
  // Global draw-call bound even with many connected phones.
  for(const entry of entries.slice(0,Math.max(0,entries.length-512))){
   const state=this.peers.get(entry.peer);state.entries=state.entries.filter(e=>e!==entry);
  }
  return entries.slice(-512).map(e=>({...e,opacity:Math.max(0,1-(now-e.born)/PHONE_TRAIL_MS)}));
 }
}

/// Width and height of a phone's camera image in the camera's own frame, read
/// from the camera Output the phone declares in its Catalog. A phone that
/// sends its image turned upright says so in the Output's frame, so the size
/// is turned back. Null when the phone declares no camera.
export function cameraShape(catalog){
 for(const component of catalog?.components||[])for(const output of Object.values(component.current_outputs||{})){
  const payload=output?.manifest?.payload,frame=output?.manifest?.spatial_frame_id||'';
  if(payload?.kind!=='camera'||!(payload.width>0)||!(payload.height>0))continue;
  // A shared screen is published as a camera too, but the pose is the camera's: only the camera's image shapes the outline.
  if(frame.startsWith('phone-screen:'))continue;
  return /:turned-(90|270)$/.test(frame)?[payload.height,payload.width]:[payload.width,payload.height];
 }
 return null;
}
/// Half extents of the outline: the image's proportions, 16 cm on its long side.
export function outlineExtents(shape){
 const [width,height]=shape&&shape[0]>0&&shape[1]>0?shape:[4,3],scale=.08/Math.max(width,height);
 return [width*scale,height*scale];
}

export function createPhoneTrails(scene,makeLabel,shapeOf=()=>null){
 const history=new PhoneTrailHistory(),group=new THREE.Group();scene.add(group);
 // A phone reports its camera's pose, so it is drawn as its camera's image:
 // the size the phone declares, in the camera's own frame, looking along -Z.
 const outlines=new Map();
 function outline(peer){
  const [x,y]=outlineExtents(shapeOf(peer)),key=x.toFixed(4)+','+y.toFixed(4);
  if(!outlines.has(key))outlines.set(key,new THREE.BufferGeometry().setFromPoints([
   [-x,-y,0],[x,-y,0],[x,-y,0],[x,y,0],
   [x,y,0],[-x,y,0],[-x,y,0],[-x,-y,0],
   [0,0,0],[0,0,-.18],[0,0,-.18],[-.025,0,-.13],[0,0,-.18],[.025,0,-.13],
  ].map(p=>new THREE.Vector3(...p))));
  return outlines.get(key);
 }
 const ghosts=new Map(),live=new Map();
 function removeGhost(key){const g=ghosts.get(key);group.remove(g);g.material.dispose();ghosts.delete(key);}
 function removeLive(key){const g=live.get(key);group.remove(g);g.traverse(o=>{if(o.material){o.material.map?.dispose();o.material.dispose();}});live.delete(key);}
 const worldUp=new THREE.Vector3();
 // The name floats above the phone in the scene, whichever way the phone is turned.
 function place(g,pose){g.position.fromArray(pose.translation);const [w,x,y,z]=pose.rotation_wxyz;g.quaternion.set(x,y,z,w);
  if(g.userData.tag)g.userData.tag.position.copy(worldUp.set(0,.15,0).applyQuaternion(g.quaternion.clone().invert()));}
 function reset(frame){for(const k of ghosts.keys())removeGhost(k);for(const k of live.keys())removeLive(k);history.reset(frame);}
 function ingest(frame,poses,now=performance.now()){
  if(frame!==history.frame)reset(frame);
  history.ingest(frame,poses,now);
  const active=new Set();
  for(const sample of poses){
   if(sample.pose?.to_frame_id!==frame||!history.peers.has(sample.peer_id))continue;
   active.add(sample.peer_id);let g=live.get(sample.peer_id);
   // A sleeping phone keeps its last pose, drawn grey and labelled, until Home drops it.
   const asleep=sample.state==='asleep';
   const geometry=outline(sample.peer_id);
   const name=(sample.name||'Phone')+(asleep?' · asleep':'');
   if(g&&(g.userData.asleep!==asleep||g.userData.geometry!==geometry||g.userData.name!==name)){removeLive(sample.peer_id);g=null;}
   if(!g){g=new THREE.Group();g.userData.geometry=geometry;g.add(new THREE.LineSegments(geometry,new THREE.LineBasicMaterial({color:asleep?'#9aa3a8':'#76dfff'})));
    const tag=makeLabel(name);g.add(tag);g.userData.tag=tag;g.userData.asleep=asleep;g.userData.name=name;group.add(g);live.set(sample.peer_id,g);}
   place(g,sample.pose);g.userData.received=asleep?now:now-Math.max(0,Number(sample.received_age_ms)||0);
  }
  for(const k of live.keys())if(!active.has(k))removeLive(k);
 }
 function tick(now=performance.now()){
  const entries=history.samples(now),keys=new Set(entries.map(e=>e.key));
  for(const k of ghosts.keys())if(!keys.has(k))removeGhost(k);
  for(const entry of entries){let glyph=ghosts.get(entry.key);
   if(!glyph){glyph=new THREE.LineSegments(outline(entry.peer),new THREE.LineBasicMaterial({color:'#76dfff',transparent:true,depthWrite:false}));place(glyph,entry.pose);group.add(glyph);ghosts.set(entry.key,glyph);}
   glyph.material.opacity=.55*entry.opacity;
  }
  for(const [key,g] of live)if(now-g.userData.received>=3000)removeLive(key);
  return {ghosts:ghosts.size,live:live.size};
 }
 return {reset,ingest,tick,group};
}
