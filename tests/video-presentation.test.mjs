import test from 'node:test';
import assert from 'node:assert/strict';
import {displaySize,nativePixel} from '../web/video-presentation.js';
test('all display rotations invert to the exact same native camera pixel',()=>{
 const w=960,h=720,x=173,y=241;
 const displayed={0:[x,y],90:[h-y,x],180:[w-x,h-y],270:[y,w-x]};
 for(const rotation of [0,90,180,270]){
  assert.deepEqual(nativePixel(...displayed[rotation],w,h,rotation),[x,y]);
  assert.deepEqual(displaySize(w,h,rotation),rotation%180?[h,w]:[w,h]);
 }
});
