import * as THREE from 'three';

export const PHONE_TRAIL_MS = 10000;
export class PhoneTrailHistory {
 constructor(){this.frame=null;this.peers=new Map();}
 reset(frame){this.frame=frame;this.peers.clear();}
 ingest(frame,poses,now){
  if(frame!==this.frame)this.reset(frame);
  for(const sample of poses){
   const {peer_id:peer,clock,pose,capture_timestamp_ns:stamp}=sample;
   if(!peer||!clock?.session_id||pose?.to_frame_id!==frame||!Number.isFinite(stamp)||
      !Array.isArray(pose.translation)||pose.translation.length!==3||!pose.translation.every(Number.isFinite)||
      !Array.isArray(pose.rotation_wxyz)||pose.rotation_wxyz.length!==4||!pose.rotation_wxyz.every(Number.isFinite))continue;
   let state=this.peers.get(peer);
   if(!state||state.session!==clock.session_id){
    if(!state&&this.peers.size>=64)continue;
    state={session:clock.session_id,last:-1,entries:[]};this.peers.set(peer,state);
   }
   // Duplicate polling must not extend a ghost's life; reject out-of-order samples.
   if(stamp<=state.last)continue;
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

export function createPhoneTrails(scene,makeLabel){
 const history=new PhoneTrailHistory(),group=new THREE.Group();scene.add(group);
 const geometry=new THREE.BufferGeometry().setFromPoints([
  [-.045,-.08,0],[.045,-.08,0],[.045,-.08,0],[.045,.08,0],
  [.045,.08,0],[-.045,.08,0],[-.045,.08,0],[-.045,-.08,0],
  [0,0,0],[0,0,-.18],[0,0,-.18],[-.025,0,-.13],[0,0,-.18],[.025,0,-.13],
 ].map(p=>new THREE.Vector3(...p)));
 const ghosts=new Map(),live=new Map();
 function removeGhost(key){const g=ghosts.get(key);group.remove(g);g.material.dispose();ghosts.delete(key);}
 function removeLive(key){const g=live.get(key);group.remove(g);g.traverse(o=>{if(o.material){o.material.map?.dispose();o.material.dispose();}});live.delete(key);}
 function place(g,pose){g.position.fromArray(pose.translation);const [w,x,y,z]=pose.rotation_wxyz;g.quaternion.set(x,y,z,w);}
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
   if(g&&g.userData.asleep!==asleep){removeLive(sample.peer_id);g=null;}
   if(!g){g=new THREE.Group();g.add(new THREE.LineSegments(geometry,new THREE.LineBasicMaterial({color:asleep?'#9aa3a8':'#76dfff'})));
    const tag=makeLabel((sample.name||'Phone')+(asleep?' · asleep':''),asleep?'#6b7479':'#167f9c');tag.position.y=.15;g.add(tag);g.userData.asleep=asleep;group.add(g);live.set(sample.peer_id,g);}
   place(g,sample.pose);g.userData.received=asleep?now:now-Math.max(0,Number(sample.received_age_ms)||0);
  }
  for(const k of live.keys())if(!active.has(k))removeLive(k);
 }
 function tick(now=performance.now()){
  const entries=history.samples(now),keys=new Set(entries.map(e=>e.key));
  for(const k of ghosts.keys())if(!keys.has(k))removeGhost(k);
  for(const entry of entries){let glyph=ghosts.get(entry.key);
   if(!glyph){glyph=new THREE.LineSegments(geometry,new THREE.LineBasicMaterial({color:'#76dfff',transparent:true,depthWrite:false}));place(glyph,entry.pose);group.add(glyph);ghosts.set(entry.key,glyph);}
   glyph.material.opacity=.55*entry.opacity;
  }
  for(const [key,g] of live)if(now-g.userData.received>=3000)removeLive(key);
  return {ghosts:ghosts.size,live:live.size};
 }
 return {reset,ingest,tick,group};
}
