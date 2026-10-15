import * as THREE from 'three';
import { createNotePin } from './note-bubble.js';

// Note level of detail. Close up a note is its text bubble; past `iconAt` metres from the camera it becomes a
// pin with a note glyph; past `clusterAt`, pins that overlap on screen (within `radius` px) merge into one pin
// with a count. The bubbles stay in `notes` (and stay the selection targets); pins and clusters live in a group
// of their own and mirror the notes group's visibility. `pinned(sprite)` keeps a note's text showing (the open card).
export function createNoteLod({scene,camera,renderer,notes,pinned=()=>false,iconAt=24,clusterAt=36,radius=44}){
 const group=new THREE.Group(),pins=new Map(),far=new WeakMap(),point=new THREE.Vector3();scene.add(group);
 // Cluster pins are pooled, and their pictures cached per count, so regrouping every frame draws nothing new.
 const pool=[],looks=new Map(),look=count=>{if(!looks.has(count))looks.set(count,createNotePin({count}));return looks.get(count);};
 const pinFor=bubble=>{let pin=pins.get(bubble);if(!pin){pin=createNotePin({peer:bubble.userData.peer});pin.userData={kind:'note',note:bubble.userData.note};group.add(pin);pins.set(bubble,pin);}return pin;};
 const dispose=sprite=>{group.remove(sprite);sprite.material.map.dispose();sprite.material.dispose();};
 function tick(){
  group.visible=notes.visible;
  const bubbles=notes.children.filter(o=>o.userData.kind==='note');
  for(const [bubble,pin] of pins)if(bubble.parent!==notes){dispose(pin);pins.delete(bubble);}
  for(const cluster of pool)cluster.visible=false;
  if(!notes.visible)return;
  const width=renderer.domElement.clientWidth,height=renderer.domElement.clientHeight,loose=[];
  for(const bubble of bubbles){
   // A metre of hysteresis either side, so a note sitting on the threshold doesn't flicker.
   const distance=camera.position.distanceTo(bubble.position),wasFar=far.get(bubble)||false,isFar=!pinned(bubble)&&(wasFar?distance>iconAt-1:distance>iconAt+1);far.set(bubble,isFar);
   bubble.visible=!isFar;const pin=isFar?pinFor(bubble):pins.get(bubble);if(pin){pin.visible=isFar;pin.position.copy(bubble.position);}
   if(isFar&&distance>clusterAt){point.copy(bubble.position).project(camera);if(point.z<1)loose.push({pin,x:(point.x+1)/2*width,y:(1-point.y)/2*height});}
  }
  // Greedy screen-space grouping: each unclaimed pin gathers the unclaimed pins within `radius` of it.
  let used=0;
  for(const seed of loose){if(seed.taken)continue;const members=loose.filter(p=>!p.taken&&Math.hypot(p.x-seed.x,p.y-seed.y)<=radius);if(members.length<2)continue;
   const centre=new THREE.Vector3();for(const p of members){p.taken=true;p.pin.visible=false;centre.add(p.pin.position);}centre.divideScalar(members.length);
   const template=look(members.length);let cluster=pool[used++];if(!cluster){cluster=new THREE.Sprite(template.material);cluster.renderOrder=template.renderOrder;group.add(cluster);pool.push(cluster);}
   cluster.material=template.material;cluster.scale.copy(template.scale);cluster.center.copy(template.center);cluster.position.copy(centre);cluster.visible=true;cluster.userData={kind:'note-cluster',centre,notes:members.map(p=>p.pin.userData.note)};}
  renderer.domElement.dataset.notePinCount=String([...pins.values()].filter(p=>p.visible).length);renderer.domElement.dataset.noteClusterCount=String(used);
 }
 // What clicks can land on: visible pins and clusters.
 const objects=()=>group.visible?group.children.filter(o=>o.visible):[];
 return {tick,objects,clusterAt};
}
