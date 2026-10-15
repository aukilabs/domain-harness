import {test} from 'node:test';
import assert from 'node:assert/strict';
import {trafficRate} from '../../web/network-traffic.js';
const sample={session:'one',elapsed_ms:1000,received_bytes:100,sent_bytes:200};
test('rates use actual elapsed time and distinguish send/receive',()=>{
 assert.deepEqual(trafficRate(sample,{...sample,elapsed_ms:3000,received_bytes:2148,sent_bytes:1224}),{received:1024,sent:512});
});
test('restart, reset, and duplicate samples never produce a misleading rate',()=>{
 for(const next of [{...sample,session:'two'},{...sample,elapsed_ms:2000,sent_bytes:0},sample])assert.equal(trafficRate(sample,next),null);
 assert.deepEqual(trafficRate(sample,{...sample,elapsed_ms:2000}),{received:0,sent:0});
});
