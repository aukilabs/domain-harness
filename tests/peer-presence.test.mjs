import test from 'node:test';
import assert from 'node:assert/strict';
import {createPeerPresence} from '../web/peer-presence.js';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return {promise,resolve,reject};};

test('fast peers render and refresh while another peer and body lookup are pending',async()=>{
 const slow=deferred(),body=deferred();let time=0,rows,fastCalls=0;
 const presence=createPeerPresence({now:()=>time,read:target=>target.key==='slow'?slow.promise:target.key==='bodies'?body.promise:Promise.resolve(++fastCalls),changed:value=>rows=value});
 const targets=[{key:'slow'},{key:'fast'},{key:'bodies'}];
 presence.update('scene',targets);await flush();
 assert.equal(rows.find(r=>r.target.key==='fast').value,1);
 assert.equal(rows.find(r=>r.target.key==='slow').value,undefined);
 time=200;presence.update('scene',targets);await flush();
 assert.equal(fastCalls,2);assert.equal(rows.find(r=>r.target.key==='fast').receivedAt,200);
 slow.resolve(1);body.resolve({robots:[]});await flush();presence.update(null,[]);
});

test('late scene and Product responses cannot resurrect stale poses',async()=>{
 const old=deferred(),newer=deferred();let rows;
 const presence=createPeerPresence({read:t=>t.key==='old'?old.promise:newer.promise,changed:r=>rows=r});
 presence.update('first',[{key:'old'}]);await flush();
 presence.update('second',[{key:'new'}]);await flush();
 old.resolve('wrong scene');await flush();
 assert.equal(rows.length,1);assert.equal(rows[0].target.key,'new');assert.equal(rows[0].value,undefined);
 presence.update('second',[]);newer.resolve('removed Product');await flush();assert.deepEqual(rows,[]);
});

test('rerendering retained results does not refresh their receipt time',async()=>{
 let time=10,rows;
 const presence=createPeerPresence({now:()=>time,read:()=>Promise.resolve({age_ms:17,timestamp_ns:'123'}),changed:r=>rows=r});
 const targets=[{key:'pose',intervalMs:10000}];presence.update('scene',targets);await flush();
 time=3000;presence.update('scene',targets);assert.equal(rows[0].receivedAt,10);assert.equal(rows[0].value.timestamp_ns,'123');
 presence.update(null,[]);
});

test('timeouts abort requests and concurrency is bounded with fair retries',async()=>{
 let time=0,calls=[],aborted=0,rows;
 const presence=createPeerPresence({now:()=>time,maxConcurrent:1,timeoutMs:10,read:(t,signal)=>{
  calls.push(t.key);signal.addEventListener('abort',()=>aborted++);return new Promise(()=>{});
 },changed:r=>rows=r});
 const targets=[{key:'a'},{key:'b'}];presence.update('scene',targets);await flush();assert.deepEqual(calls,['a']);
 await new Promise(r=>setTimeout(r,30));assert.equal(aborted,1);assert.match(rows[0].error,/timed out/);
 time=200;presence.update('scene',targets);await flush();assert.deepEqual(calls,['a','b']);
 presence.update(null,[]);await new Promise(r=>setTimeout(r,20));assert.deepEqual(rows,[]);
});

test('Home viewer draws a valid fast peer before a slow pose or body response',async()=>{
 const source=readFileSync(new URL('../web/app.js',import.meta.url),'utf8');
 const block=source.slice(source.indexOf('const peerPresence=createPeerPresence('),source.indexOf('setInterval(refreshPhonePresence,200)'));
 const slow=deferred(),body=deferred();let time=0,poses=[],valid=true;
 const peer=id=>({peer_id:id,name:id,catalog:{products:[{manifest:{product_id:'pose'},manifest_hash:'manifest',
  metadata:{schema:'spatial-harness.compact-pose-contract/v1',value:{from_frame:{peer_id:id,id:'camera',hash:'frame'}}}}]}});
 const value=()=>({home_pose:{frame:'home',alignment_timestamp_ns:'10',tracking:valid?{status:'valid',pose:{translation:[1,2,3],rotation_xyzw:[0,0,0,1]}}:{status:'lost'}},
  stream:'live',age_ms:5,timestamp_ns:'123',resolved:{contract:{session_id:'session'}}});
 const context=vm.createContext({createPeerPresence:options=>createPeerPresence({...options,now:()=>time}),
  URLSearchParams,performance:{now:()=>time},isLocalScene:()=>true,current:{frame:'home'},activeMap:'local:test',
  fleetNetwork:{peers:[peer('slow'),peer('fast')]},presenceCatalogErrors:[],declaredBodies:{},refreshRunner:()=>{},
  sameFrame:(a,b)=>JSON.stringify(a)===JSON.stringify(b),phoneTrails:{ingest:(_,p)=>{poses=p;}},
  homePoseStatus:{},plural:(n,s)=>`${n} ${s}`, $:()=>({dataset:{}}),
  fetch:async url=>{const result=url==='/api/robot-states'?await body.promise:url.includes('slow')?await slow.promise:value();return {ok:true,json:async()=>result};}});
 vm.runInContext(block,context);context.refreshPhonePresence();await flush();
 assert.equal(poses.length,1);assert.equal(poses[0].peer_id,'fast');assert.equal(poses[0].pose.translation[0],1);
 valid=false;time=200;context.refreshPhonePresence();await flush();assert.equal(poses.length,0,'tracking loss removes the live label');
 context.fleetNetwork={peers:[]};context.refreshPhonePresence();slow.resolve(value());body.resolve({robots:[]});await flush();
 assert.equal(poses.length,0,'late disconnected peer cannot reappear');
});
