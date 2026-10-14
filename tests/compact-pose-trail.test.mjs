import {test} from 'node:test';
import assert from 'node:assert/strict';
import * as THREE from 'three';
import {displayBasis,createCompactPoseTrail} from '../web/compact-pose-trail.js';
test('registry axes orient robot coordinates without a Home transform',()=>{
 const basis=displayBasis({units:'meters',handedness:'right',axes:{x:'right',y:'forward',z:'up'}});
 assert.deepEqual(new THREE.Vector3(1,2,3).applyMatrix4(basis).toArray(),[1,3,-2]);
 assert.throws(()=>displayBasis({units:'meters'}));
});
test('compact trail draws valid records, skips lost and clears between peers',()=>{
 const scene=new THREE.Scene(),trail=createCompactPoseTrail(scene);
 const observation={payload:{tracking:{status:'valid',pose:{translation:[1,2,3],rotation_xyzw:[0,0,0,1]}}}};
 const data={resolved:{contract:{to_frame:{frame_id:'native',units:'meters',handedness:'right',axes:{x:'right',y:'up',z:'backward'}}}},observation,history:[{observation},{observation:{payload:{tracking:{status:'lost'}}}}]};
 assert.equal(trail.update(data).count,1);
 assert.equal(trail.update({...data,history:[]}).count,0);
 trail.reset();assert.equal(scene.children[0].children.length,0);
 assert.throws(()=>trail.update({...data,resolved:null}));
});
