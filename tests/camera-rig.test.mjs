import test from 'node:test';
import assert from 'node:assert/strict';
import * as THREE from 'three';
import {createCameraRig} from '../web/camera-rig.js';
function setup(position,target,extra={}){
 const camera=new THREE.PerspectiveCamera();camera.position.set(...position);
 const controls={target:new THREE.Vector3(...target),minDistance:1.2,maxDistance:60,...extra};camera.lookAt(controls.target);
 const rig=createCameraRig({camera,controls,floor:()=>0});
 const toward=to=>new THREE.Vector3(...to).sub(camera.position).normalize();
 return {camera,controls,rig,toward};
}
const onViewLine=(camera,controls,direction)=>controls.target.clone().sub(camera.position).normalize().distanceTo(direction)<1e-9;
test('a keyboard move rests the target on the floor along the view, keeping the aim',()=>{
 const {camera,controls,rig}=setup([0,6,8],[0,2,0]);const aim=controls.target.clone().sub(camera.position).normalize();
 rig.translate(new THREE.Vector3(0,1,0));
 assert.equal(controls.target.y,0);assert.ok(onViewLine(camera,controls,aim));
});
test('rising high keeps the target on the floor, so the orbit distance grows with the height (to its limit)',()=>{
 const {camera,controls,rig}=setup([0,3,3],[0,0,0]);
 for(let i=0;i<100;i++)rig.translate(new THREE.Vector3(0,2,0));
 assert.equal(camera.position.distanceTo(controls.target),60,'held at the far limit');
 assert.ok(controls.target.y>0,'floats only beyond the limit');
});
test('sinking stops where the floor would come nearer than the minimum distance; never pushed back up',()=>{
 const {camera,controls,rig}=setup([0,6,8],[0,0,0]);let y=camera.position.y,stopped=false;
 for(let i=0;i<200;i++){stopped=rig.translate(new THREE.Vector3(0,-.1,0))||stopped;assert.ok(camera.position.y<=y);y=camera.position.y;rig.settle();assert.equal(camera.position.y,y,'settle leaves it');}
 assert.equal(stopped,true);assert.equal(controls.target.y,0);assert.ok(Math.abs(camera.position.distanceTo(controls.target)-1.2)<1e-9);
});
test('the wheel moves forward toward the floor under the cursor and lands, never through it',()=>{
 const {camera,controls,rig,toward}=setup([0,7,7],[0,0,0]);const ray=toward([0,0,-1]);let moving=true,steps=0;
 while(moving&&steps<2000){moving=rig.dolly(ray,-Math.log(1/.95));steps++;assert.ok(camera.position.y>0);assert.ok(controls.target.y>=0);}
 assert.ok(camera.position.y<1.2,`landed at ${camera.position.y}`);assert.ok(camera.position.distanceTo(controls.target)>=1.2-1e-9);
});
test('from high above with the target floating beside the camera, the wheel flies all the way down',()=>{
 const {camera,controls,rig,toward}=setup([0,207,7],[0,200,0]);const ray=toward([0,200,0]);
 for(let i=0;i<400;i++)rig.dolly(ray,-Math.log(1/.95));
 assert.ok(camera.position.y<2,`reached ${camera.position.y}`);assert.equal(controls.target.y,0);
});
test('backing away stops at the far limit',()=>{
 const {camera,controls,rig,toward}=setup([0,7,7],[0,0,0]);const ray=toward([0,0,0]).negate();let steps=0;
 while(rig.dolly(ray,Math.log(1/.95))&&steps<1000)steps++;
 assert.ok(steps<1000);assert.ok(camera.position.distanceTo(controls.target)<=60+1e-9);assert.equal(controls.target.y,0);
});
test('the wheel keeps a target set above the floor (a note) while within the limits',()=>{
 const {camera,controls,rig,toward}=setup([2,3,2],[0,1.5,0]);const ray=toward([0,1.5,0]);
 rig.dolly(ray,-Math.log(1/.95));
 assert.ok(controls.target.distanceTo(new THREE.Vector3(0,1.5,0))<1e-9);
});
test('settle leaves a target set elsewhere where it was put, holding only the floor',()=>{
 const {camera,controls,rig}=setup([0,6,8],[0,1.5,0]);rig.settle();
 assert.deepEqual(controls.target.toArray(),[0,1.5,0]);
 camera.position.y=-1;controls.target.y=-.5;rig.settle();assert.equal(camera.position.y,.05);assert.equal(controls.target.y,0);
});
