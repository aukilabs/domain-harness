import test from 'node:test';
import assert from 'node:assert/strict';
import {projectColumns} from '../web/map-2d.js';
test('projects unique occupied columns in the explicit one-meter band',()=>{
 const voxels=[[.05,0,.05],[.05,.05,.05],[.05,1,.05],[.05,1.01,.05],[-.05,.5,-.05]].map(center=>({center}));
 assert.deepEqual(projectColumns(voxels,.1,0),[
  {center:[.05,0,.05],value:100},
  {center:[-.05,0,-.05],value:100},
 ]);
});
test('floor is configurable and missing geometry is not free',()=>{
 assert.deepEqual(projectColumns([],.1,0),[]);
 assert.equal(projectColumns([{center:[0,2.5,0]}],.1,2).length,1);
 assert.equal(projectColumns([{center:[0,2.5,0]}],.1,0).length,0);
});
