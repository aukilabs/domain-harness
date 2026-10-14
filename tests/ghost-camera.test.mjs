import test from 'node:test';
import assert from 'node:assert/strict';
import * as THREE from 'three';
import {groundDirection,moveSpeed,isEditing,createGhostCamera} from '../web/ghost-camera.js';
const close=(actual,expected)=>actual.forEach((v,i)=>assert.ok(Math.abs(v-expected[i])<1e-9,`${actual} ≈ ${expected}`));
function rig(position=[0,6,8]){
 const host=new EventTarget(),doc=new EventTarget(),canvas=new EventTarget();doc.activeElement=null;doc.hidden=false;
 const camera=new THREE.PerspectiveCamera();camera.position.set(...position);const controls={enabled:true,target:new THREE.Vector3(0,0,0)};camera.lookAt(controls.target);
 const nav=createGhostCamera({camera,controls,canvas,host,doc,enabled:()=>true});
 const key=(type,code,props={})=>{const e=new Event(type,{cancelable:true});Object.assign(e,{code},props);host.dispatchEvent(e);return e;};
 const run=ms=>{for(let t=0;t<=ms;t+=16)nav.tick(t);};
 return {host,doc,canvas,camera,controls,nav,key,run};
}
test('W/S follow the heading over the ground, ignoring pitch',()=>{
 close(groundDirection(new Set(['KeyW']),[0,-1]),[0,0,-1]);
 close(groundDirection(new Set(['KeyS']),[0,-1]),[0,0,1]);
 close(groundDirection(new Set(['KeyD']),[0,-1]),[1,0,0]);
 close(groundDirection(new Set(['KeyA']),[0,-1]),[-1,0,0]);
 close(groundDirection(new Set(['KeyW']),[0,-.2]),[0,0,-1]);
});
test('Space rises, either Shift sinks, opposites cancel and diagonals are normalized',()=>{
 close(groundDirection(new Set(['Space']),[0,-1]),[0,1,0]);
 for(const shift of ['ShiftLeft','ShiftRight'])close(groundDirection(new Set([shift]),[0,-1]),[0,-1,0]);
 close(groundDirection(new Set(['Space','ShiftLeft']),[0,-1]),[0,0,0]);
 close(groundDirection(new Set(['KeyW','KeyS']),[0,-1]),[0,0,0]);
 assert.ok(Math.abs(Math.hypot(...groundDirection(new Set(['KeyW','KeyD','Space']),[0,-1]))-1)<1e-12);
});
test('speed scales with zoom distance within limits',()=>{
 assert.equal(moveSpeed(.5),1);assert.equal(moveSpeed(10),8);assert.equal(moveSpeed(100),25);
});
test('W moves camera and target together along the ground, even looking straight down',()=>{
 for(const position of [[0,6,8],[0,10,.0001]]){
  const {camera,controls,key,run}=rig(position);const start=camera.position.clone();
  key('keydown','KeyW');run(500);
  assert.ok(controls.target.z<-.5,'moved forward');assert.ok(Math.abs(camera.position.y-start.y)<1e-9,'height unchanged');
  close(camera.position.clone().sub(controls.target).toArray(),start.toArray());
 }
});
test('Q/E turn around the target at a constant distance',()=>{
 const {camera,controls,key,run}=rig();const distance=camera.position.distanceTo(controls.target),before=camera.position.clone();
 key('keydown','KeyE');run(500);
 assert.ok(Math.abs(camera.position.distanceTo(controls.target)-distance)<1e-9);assert.ok(camera.position.distanceTo(before)>1);
});
test('typing never captures movement keys; Shift during a click is left alone; blur clears',()=>{
 const {doc,canvas,host,key,nav,camera}=rig();
 doc.activeElement={isContentEditable:true};for(const code of ['KeyW','Space','ShiftLeft','KeyQ'])assert.equal(key('keydown',code).defaultPrevented,false);
 doc.activeElement=null;assert.equal(key('keydown','Space').defaultPrevented,true);key('keyup','Space');
 canvas.dispatchEvent(new Event('pointerdown'));assert.equal(key('keydown','ShiftLeft').defaultPrevented,false);host.dispatchEvent(new Event('pointerup'));
 assert.equal(key('keydown','KeyW',{metaKey:true}).defaultPrevented,false);
 key('keydown','KeyW');host.dispatchEvent(new Event('blur'));const at=camera.position.clone();nav.tick(0);nav.tick(16);nav.tick(32);
 assert.deepEqual(camera.position.toArray(),at.toArray());
});
test('editable inputs and contenteditable suppress navigation',()=>{
 assert.equal(isEditing({isContentEditable:true}),true);
 assert.equal(isEditing({closest:()=>({tagName:'INPUT'})}),true);
 assert.equal(isEditing({closest:()=>null}),false);
});
