import test from 'node:test';
import assert from 'node:assert/strict';
import * as THREE from 'three';
import {createCameraPan} from '../web/ghost-camera.js';
import {moveMapCamera} from '../web/view-camera.js';

const action = {type:'move_camera',scene:'home',frame:'home-frame',position:[2,3,4],look_at:[0,1,0]};
function setup() {
  const camera = new THREE.PerspectiveCamera();camera.position.set(8,6,10);
  const controls = {target:new THREE.Vector3(),enableDamping:true,update(){}};
  let time=0,flat=false,detaches=0;
  const cameraPan = createCameraPan({camera,controls,now:()=>time});
  const context = {scene:'home',frame:'home-frame',camera,controls,cameraPan,
    map2D:{isActive:()=>flat,toggle:()=>{flat=!flat;}},beforeMove:()=>{detaches++;}};
  return {camera,controls,cameraPan,context,setTime:t=>time=t,setFlat:()=>{flat=true;},detaches:()=>detaches};
}
test('Qwen camera move shares double-click easing and reaches the exact requested view',()=>{
  const {camera,controls,cameraPan,context,detaches}=setup();
  const start=camera.position.clone();
  assert.equal(moveMapCamera(action,context),'Map camera moving.');
  assert.equal(detaches(),1);
  assert.deepEqual(camera.position.toArray(),start.toArray());
  cameraPan.tick(70);
  assert.ok(camera.position.distanceTo(start)>0);
  assert.ok(camera.position.distanceTo(start)<start.distanceTo(new THREE.Vector3(...action.position))*.1);
  cameraPan.tick(350);
  assert.deepEqual(camera.position.toArray(),start.clone().lerp(new THREE.Vector3(...action.position),.5).toArray());
  assert.deepEqual(controls.target.toArray(),[0,.5,0]);
  cameraPan.tick(700);
  assert.deepEqual(camera.position.toArray(),action.position);
  assert.deepEqual(controls.target.toArray(),action.look_at);
  assert.equal(controls.enableDamping,true);
  assert.equal(cameraPan.tick(701),false);
});
test('manual cancellation and replacement moves use the same shared flight',()=>{
  const {camera,cameraPan,context,setTime}=setup();
  moveMapCamera(action,context);cameraPan.tick(350);
  const halfway=camera.position.clone();
  cameraPan.cancel();cameraPan.tick(700);assert.deepEqual(camera.position.toArray(),halfway.toArray());
  setTime(700);moveMapCamera({...action,position:[10,3,4]},context);
  assert.deepEqual(camera.position.toArray(),halfway.toArray());
  cameraPan.tick(1050);assert.deepEqual(camera.position.toArray(),halfway.clone().lerp(new THREE.Vector3(10,3,4),.5).toArray());
});
test('invalid/stale moves do not detach, leave 2D, or interrupt an existing pan',()=>{
  const {camera,cameraPan,context,setFlat,detaches}=setup();
  cameraPan.start(new THREE.Vector3(6,6,10),new THREE.Vector3());setFlat();
  for(const bad of [{...action,frame:'old'}, {...action,position:[0,1,0]}, {...action,position:[NaN,0,0]}]) {
    assert.throws(()=>moveMapCamera(bad,context));
  }
  assert.equal(detaches(),0);assert.equal(context.map2D.isActive(),true);
  cameraPan.tick(700);assert.deepEqual(camera.position.toArray(),[6,6,10]);
});
test('a valid move leaves 2D and allows close-up or upward-looking destinations',()=>{
  const {camera,controls,cameraPan,context,setFlat}=setup();setFlat();
  moveMapCamera({...action,position:[0,1,0],look_at:[0,1.1,.1]},context);
  assert.equal(context.map2D.isActive(),false);
  cameraPan.tick(700);
  assert.deepEqual(camera.position.toArray(),[0,1,0]);
  assert.deepEqual(controls.target.toArray(),[0,1.1,.1]);
  assert.equal(controls.minDistance,.01);assert.equal(controls.maxPolarAngle,Math.PI);
});
