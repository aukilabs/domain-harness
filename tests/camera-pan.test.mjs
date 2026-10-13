import test from 'node:test';
import assert from 'node:assert/strict';
import * as THREE from 'three';
import {createCameraPan} from '../web/ghost-camera.js';
function setup(){
 const camera=new THREE.PerspectiveCamera(),controls={target:new THREE.Vector3(),enableDamping:true,update(){}};
 let now=0;return {camera,controls,setTime:t=>now=t,pan:createCameraPan({camera,controls,duration:1000,now:()=>now})};
}
test('pan eases in and out, does not teleport, and lands exactly',()=>{
 const {camera,controls,pan}=setup();
 pan.start(new THREE.Vector3(10,0,0),new THREE.Vector3(0,10,0));
 assert.equal(camera.position.x,0);pan.tick(100);assert.ok(camera.position.x<1);
 pan.tick(500);assert.equal(camera.position.x,5);assert.equal(controls.target.y,5);
 pan.tick(900);assert.ok(camera.position.x>9);pan.tick(1000);
 assert.equal(camera.position.x,10);assert.equal(controls.target.y,10);assert.equal(pan.tick(1100),false);
 assert.equal(controls.enableDamping,true);
});
test('cancel leaves current position; retarget starts from current position',()=>{
 const {camera,pan,setTime}=setup();pan.start(new THREE.Vector3(10,0,0),new THREE.Vector3());pan.tick(500);
 pan.cancel();assert.equal(pan.tick(1000),false);assert.equal(camera.position.x,5);
 setTime(1000);pan.start(new THREE.Vector3(15,0,0),new THREE.Vector3());pan.tick(1000);assert.equal(camera.position.x,5);
 pan.tick(1500);assert.equal(camera.position.x,10);
});
