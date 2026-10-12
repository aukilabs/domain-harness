import test from 'node:test';
import assert from 'node:assert/strict';
import * as THREE from 'three';
import {ghostStep,isEditing,createGhostCamera,lookDirection} from '../web/ghost-camera.js';
test('mouse look changes aim without orbiting and limits pitch',()=>{
 const right=lookDirection([0,0,-1],100,0);assert.ok(right[0]>0);assert.equal(right[1],0);
 const up=lookDirection([0,0,-1],0,-100);assert.ok(up[1]>0);
 assert.ok(lookDirection([0,0,-1],0,-10000)[1]<1);
 assert.ok(Math.abs(Math.hypot(...right)-1)<1e-12);
});
test('mouse look requires held WASD and stops on release or typing',()=>{
 const host=new EventTarget(),doc=new EventTarget(),canvas=new EventTarget();doc.activeElement=null;doc.hidden=false;
 const camera=new THREE.PerspectiveCamera();camera.position.set(0,1,5);const controls={enabled:true,enableRotate:true,target:new THREE.Vector3(0,1,0)};camera.lookAt(controls.target);
 const nav=createGhostCamera({camera,controls,canvas,host,doc,enabled:()=>true});
 const mouse=()=>{const e=new Event('pointermove');Object.assign(e,{movementX:100,movementY:0,buttons:0,pointerType:'mouse'});canvas.dispatchEvent(e);};
 const key=(type,code)=>{const e=new Event(type,{cancelable:true});Object.assign(e,{code});host.dispatchEvent(e);};
 mouse();assert.deepEqual(controls.target.toArray(),[0,1,0]);
 for(const code of ['KeyQ','KeyE','ShiftLeft']){key('keydown',code);mouse();assert.deepEqual(controls.target.toArray(),[0,1,0]);key('keyup',code);}
 for(const code of ['KeyW','KeyA','KeyS','KeyD']){const before=controls.target.clone();key('keydown',code);mouse();assert.notDeepEqual(controls.target.toArray(),before.toArray());key('keyup',code);const after=controls.target.clone();mouse();assert.deepEqual(controls.target.toArray(),after.toArray());}
 assert.deepEqual(camera.position.toArray(),[0,1,5]);key('keydown','KeyW');
 const aim=controls.target.clone();doc.activeElement={isContentEditable:true};mouse();assert.deepEqual(controls.target.toArray(),aim.toArray());
 doc.activeElement=null;nav.tick(0);nav.tick(20);assert.ok(camera.position.x>0);assert.ok(camera.position.z<5);
});
test('WASD follows view axes, cancels opposites and normalizes diagonals',()=>{
 const forward=[0,0,-1],right=[1,0,0];
 assert.deepEqual(ghostStep(new Set(['KeyW']),forward,right,.02),[0,0,-.06]);
 assert.deepEqual(ghostStep(new Set(['KeyA']),forward,right,.02),[-.06,0,0]);
 assert.deepEqual(ghostStep(new Set(['KeyW','KeyS']),forward,right,.02),[0,0,0]);
 assert.ok(Math.abs(Math.hypot(...ghostStep(new Set(['KeyW','KeyD']),forward,right,.02))-.06)<1e-12);
 assert.deepEqual(ghostStep(new Set(['KeyW']),forward,right,5),[0,0,-.15000000000000002]);
});
test('normal orbit settings return and an existing drag never switches owners',()=>{
 const host=new EventTarget(),doc=new EventTarget(),canvas=new EventTarget();doc.activeElement=null;doc.hidden=false;
 const camera=new THREE.PerspectiveCamera();camera.position.set(0,3,5);
 const controls={enabled:true,enableRotate:true,minPolarAngle:0,maxPolarAngle:Math.PI*.49,target:new THREE.Vector3(0,0,0)};camera.lookAt(controls.target);
 createGhostCamera({camera,controls,canvas,host,doc,enabled:()=>true});
 const event=(target,type,props)=>{const e=new Event(type,{cancelable:true});Object.assign(e,props);target.dispatchEvent(e);return e;};
 event(canvas,'pointerdown',{pointerId:1});event(host,'keydown',{code:'KeyW'});
 const before=controls.target.toArray();event(canvas,'pointermove',{movementX:100,movementY:0,pointerType:'mouse'});assert.deepEqual(controls.target.toArray(),before);assert.equal(controls.enableRotate,true);
 event(canvas,'pointerup',{pointerId:1});assert.equal(controls.enableRotate,false);
 event(host,'keyup',{code:'KeyW'});assert.equal(controls.enableRotate,true);assert.equal(controls.maxPolarAngle,Math.PI*.49);assert.equal(controls.minPolarAngle,0);
 event(host,'keydown',{code:'KeyW'});event(canvas,'pointerdown',{pointerId:2});event(host,'keyup',{code:'KeyW'});
 assert.equal(event(canvas,'pointermove',{movementX:100,movementY:0,pointerType:'mouse'}).cancelBubble,true);
 event(canvas,'pointerup',{pointerId:2});assert.equal(controls.enableRotate,true);
});
test('editable inputs and contenteditable suppress navigation',()=>{
 assert.equal(isEditing({isContentEditable:true}),true);
 assert.equal(isEditing({closest:()=>({tagName:'INPUT'})}),true);
 assert.equal(isEditing({closest:()=>null}),false);
});
test('Q/E use world vertical and either Shift triples speed',()=>{
 const forward=[0,.6,-.8],right=[1,0,0];
 assert.deepEqual(ghostStep(new Set(['KeyQ']),forward,right,.02),[0,.06,0]);
 assert.deepEqual(ghostStep(new Set(['KeyE']),forward,right,.02),[0,-.06,0]);
 assert.deepEqual(ghostStep(new Set(['KeyQ','KeyE']),forward,right,.02),[0,0,0]);
 for(const shift of ['ShiftLeft','ShiftRight']){
  assert.ok(Math.abs(Math.hypot(...ghostStep(new Set(['KeyW',shift]),forward,right,.02))-.18)<1e-12);
 }
 assert.ok(Math.abs(Math.hypot(...ghostStep(new Set(['KeyW','KeyQ','KeyD']),forward,right,.02))-.06)<1e-12);
});
test('typing never captures WASD; blur clears held movement',()=>{
 const host=new EventTarget(),doc=new EventTarget();doc.activeElement=null;doc.hidden=false;
 const nav=createGhostCamera({host,doc,camera:new THREE.PerspectiveCamera(),controls:{enabled:true},enabled:()=>true});
 const key=(code='KeyW')=>{const e=new Event('keydown',{cancelable:true});Object.assign(e,{code});host.dispatchEvent(e);return e;};
 doc.activeElement={isContentEditable:true};for(const code of ['KeyW','KeyQ','KeyE','ShiftLeft'])assert.equal(key(code).defaultPrevented,false);nav.tick(0);
 doc.activeElement=null;assert.equal(key().defaultPrevented,true);
 host.dispatchEvent(new Event('blur'));nav.tick(10);nav.tick(20); // no camera access after blur
});
