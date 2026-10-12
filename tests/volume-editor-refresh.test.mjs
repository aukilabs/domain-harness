import test from 'node:test';
import assert from 'node:assert/strict';
import {mergeVolumeDraft,legacyVolumesForView} from '../web/semantic-volumes.js';
test('remote description updates the draft without losing unsaved geometry',()=>{
 const previous={description:'old',size:[1,2,3]},local={...previous,size:[2,2,3]},remote={...previous,description:'new instruction'};
 assert.deepEqual(mergeVolumeDraft(previous,local,remote),{draft:{description:'new instruction',size:[2,2,3]},conflicts:[]});
});
test('floating point round-trip is not an unsaved edit and receives the latest position',()=>{
 const previous={center:[1.109544,9.2680879,-1.1906646],size:[2,1,4],yaw:.3};
 const local={...previous,center:[1.109544,9.268087900000002,-1.1906646],yaw:.30000000000000004};
 const remote={...previous,center:[1.109544,8.2680879,-1.1906646]};
 assert.deepEqual(mergeVolumeDraft(previous,local,remote),{draft:remote,conflicts:[]});
});
test('authoritative editor volumes suppress the legacy duplicate, retaining remote fallback',()=>{
 const legacy=[{id:'same-id'}];
 assert.deepEqual(legacyVolumesForView({frame:'home',semantic_volumes:{frame:'home',items:[{id:'same-id'}]},volumes:legacy}),[]);
 assert.deepEqual(legacyVolumesForView({frame:'home',semantic_volumes:{frame:'home',items:[]},volumes:legacy}),[]);
 assert.deepEqual(legacyVolumesForView({frame:'remote',volumes:legacy}),legacy);
 assert.deepEqual(legacyVolumesForView({frame:'remote',semantic_volumes:{frame:'other',items:[]},volumes:legacy}),legacy);
});
test('simultaneous description edits are a conflict, not an overwrite',()=>{
 const result=mergeVolumeDraft({description:'old'},{description:'local edit'},{description:'remote edit'});
 assert.equal(result.draft.description,'local edit');assert.deepEqual(result.conflicts,['description']);
});
