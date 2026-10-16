import test from 'node:test';
import assert from 'node:assert/strict';
import * as THREE from 'three';
import {groundDirection,moveSpeed,isEditing,createGhostCamera} from '../web/ghost-camera.js';
const close=(actual,expected)=>actual.forEach((v,i)=>assert.ok(Math.abs(v-expected[i])<1e-9,`${actual} ≈ ${expected}`));
function rig(position=[0,6,8],options={}){
 const host=new EventTarget(),doc=new EventTarget(),canvas=new EventTarget();doc.activeElement=null;doc.hidden=false;
 const camera=new THREE.PerspectiveCamera();camera.position.set(...position);const controls={enabled:true,target:new THREE.Vector3(0,0,0),...options.controls};camera.lookAt(controls.target);
 const nav=createGhostCamera({camera,controls,canvas,host,doc,enabled:()=>true,floor:options.floor});
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
test('Shift sinks onto the floor and stops at the orbit limits instead of bouncing off them',()=>{
 const maxPolarAngle=Math.PI*.49,minDistance=1.2;
 for(const position of [[0,6,8],[0,6,.0001]]){
  const {camera,controls,key,nav}=rig(position,{floor:()=>0,controls:{maxPolarAngle,minDistance}});controls.target.y=2;
  key('keydown','ShiftLeft');const heights=[];for(let t=0;t<=4000;t+=16){nav.tick(t);heights.push(camera.position.y);}
  assert.equal(controls.target.y,0,'target rests on the floor');
  for(let i=1;i<heights.length;i++)assert.ok(heights[i]<=heights[i-1],'never pushed back up');
  const offset=camera.position.clone().sub(controls.target),phi=Math.acos(offset.y/offset.length());
  assert.ok(phi<=maxPolarAngle+1e-9,'within the polar limit');assert.ok(offset.length()>=minDistance-1e-9,'outside the minimum distance');
  assert.ok(camera.position.y>=.05,'above the floor');
 }
});
test('Space rises while the mouse is held; a cancelled press releases Shift',()=>{
 const {host,canvas,key}=rig();
 canvas.dispatchEvent(new Event('pointerdown'));assert.equal(key('keydown','Space').defaultPrevented,true,'Space during a drag');
 assert.equal(key('keydown','ShiftLeft').defaultPrevented,false,'Shift during a click');
 host.dispatchEvent(new Event('pointercancel'));assert.equal(key('keydown','ShiftLeft').defaultPrevented,true,'Shift after a cancelled press');
});
test('a control left focused by a click keeps no hold on Space and Shift; one focused from the keyboard does',()=>{
 const {host,doc,key,run,camera}=rig();const button={closest:()=>null};
 host.dispatchEvent(new Event('pointerdown'));doc.activeElement=button;
 key('keydown','KeyW');key('keydown','KeyA');const y=camera.position.y;
 assert.equal(key('keydown','Space').defaultPrevented,true,'Space beside WASD');run(300);assert.ok(camera.position.y>y+.5,'rose');
 assert.equal(key('keyup','Space').defaultPrevented,true,'no click on the focused button');
 key('keydown','Tab',{key:'Tab'});
 assert.equal(key('keydown','Space').defaultPrevented,false,'Space activates a keyboard-focused control');
 assert.equal(key('keydown','ShiftLeft',{key:'Shift'}).defaultPrevented,false,'Shift+Tab stays navigation');
 assert.equal(key('keydown','KeyW').defaultPrevented,true,'WASD still moves');
});
test('⌘ stops movement, so a key let go while it is down (no keyup on macOS) never runs on',()=>{
 const {key,nav,camera}=rig();key('keydown','KeyW');for(let t=0;t<=200;t+=16)nav.tick(t);
 key('keydown','MetaLeft',{key:'Meta'});const at=camera.position.clone();for(let t=216;t<=600;t+=16)nav.tick(t);
 assert.deepEqual(camera.position.toArray(),at.toArray());
});
test('high above the floor with the target close by, the target settles on the floor and movement keeps pace',()=>{
 const {camera,controls,key,run}=rig([0,201,1],{floor:()=>0,controls:{minDistance:1.2,maxDistance:400}});controls.target.set(0,200,0);
 key('keydown','KeyW');run(1000);
 assert.equal(controls.target.y,0,'target on the floor');assert.ok(camera.position.z<-10,`moved ${-camera.position.z} m in a second`);
});
