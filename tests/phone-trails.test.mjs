import {test} from 'node:test';
import assert from 'node:assert/strict';
import {PhoneTrailHistory,cameraShape,outlineExtents} from '../web/phone-trails.js';
const pose=(stamp,peer='phone',session='a',frame='room')=>({peer_id:peer,clock:{session_id:session},capture_timestamp_ns:stamp,pose:{to_frame_id:frame,translation:[1,2,3],rotation_wxyz:[1,0,0,0]}});
test('fades for ten seconds, duplicates never extend lifetime',()=>{
 const h=new PhoneTrailHistory();h.ingest('room',[pose(1)],100);
 assert.equal(h.samples(5100)[0].opacity,.5);
 h.ingest('room',[pose(1)],9000);assert.equal(h.samples(10100).length,0);
 h.ingest('room',[pose(1)],11000);assert.equal(h.samples(11000).length,0);
});
test('separate peers, changed sessions and map switches are isolated',()=>{
 const h=new PhoneTrailHistory();h.ingest('room',[pose(1),pose(2,'other')],0);
 assert.equal(h.samples(0).length,2);
 h.ingest('room',[pose(3),pose(2)],100);assert.equal(h.samples(100).length,3);
 h.ingest('room',[pose(1,'phone','new')],200);assert.equal(h.samples(200).length,2);
 h.ingest('elsewhere',[pose(4)],300);assert.equal(h.samples(300).length,0);
});
test('age and resource bounds',()=>{
 const h=new PhoneTrailHistory();h.ingest('room',[{...pose(1),received_age_ms:9000}],10000);
 assert.ok(h.samples(10000)[0].opacity<.11);
 for(let i=2;i<100;i++)h.ingest('room',[pose(i)],10000);
 assert.equal(h.samples(10000).length,50);
 for(let p=0;p<64;p++)for(let i=1;i<15;i++)h.ingest('room',[pose(i,'p'+p)],10000);
 assert.ok(h.samples(10000).length<=512);assert.ok(h.peers.size<=64);
});
test('a phone is drawn with the size of the camera image it declares, in the camera frame',()=>{
 const camera=(width,height,frame)=>({components:[{current_outputs:{depth:{manifest:{payload:{kind:'structured'}}}}},
  {current_outputs:{frames:{manifest:{spatial_frame_id:frame,payload:{kind:'camera',width,height}}}}}]});
 assert.deepEqual(cameraShape(camera(640,480,'phone-camera:s')),[640,480]);
 // Sent turned upright: the size is turned back into the camera's own frame.
 assert.deepEqual(cameraShape(camera(480,640,'phone-camera:s:turned-90')),[640,480]);
 assert.deepEqual(cameraShape(camera(480,640,'phone-camera:s:turned-270')),[640,480]);
 assert.deepEqual(cameraShape(camera(640,480,'phone-camera:s:turned-180')),[640,480]);
 assert.equal(cameraShape({components:[]}),null);assert.equal(cameraShape(undefined),null);
 const close=(a,b)=>a.forEach((v,i)=>assert.ok(Math.abs(v-b[i])<1e-9));
 close(outlineExtents([640,480]),[.08,.06]);close(outlineExtents([1080,1920]),[.045,.08]);
 // No declared camera: a 4:3 image, which is what phone cameras produce.
 close(outlineExtents(null),[.08,.06]);
});
