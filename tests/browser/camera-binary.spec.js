import {test,expect} from '@playwright/test';
import {readFileSync} from 'node:fs';
test('binary PiP preserves capture metadata and image bytes without base64',async({page})=>{
 await page.route('**/camera-frame.js',r=>r.fulfill({contentType:'text/javascript',body:readFileSync(new URL('../../web/camera-frame.js',import.meta.url),'utf8')}));
 await page.goto('/');
 const result=await page.evaluate(async()=>{
  const {readCameraResponse}=await import('/camera-frame.js');
  const metadata={width:2,height:2,encoding:'rgb8',sequence:'7',capture_timestamp_ns:'9007199254740993',clock:{peer_id:'arbitrary-peer',id:'clock',hash:'h'},ar:{visible:[]}};
  const meta=new TextEncoder().encode(JSON.stringify(metadata)),pixels=Uint8Array.from([255,0,34,10,128,1,0,0,0,255,255,255]);
  const packet=new Uint8Array(4+meta.length+pixels.length);new DataView(packet.buffer).setUint32(0,meta.length,true);packet.set(meta,4);packet.set(pixels,4+meta.length);
  window.atob=()=>{throw Error('Binary path must not decode base64');};
  const make=b=>new Response(b,{headers:{'content-type':'application/vnd.spatial-harness.camera-frame'}});
  const parsed=await readCameraResponse(make(packet));
  let rejected=false;try{await readCameraResponse(make(packet.slice(0,6)));}catch{rejected=true;}
  return {timestamp:parsed.capture_timestamp_ns,pixels:[...parsed.bytes],clock:parsed.clock,rejected};
 });
 expect(result).toEqual({timestamp:'9007199254740993',pixels:[255,0,34,10,128,1,0,0,0,255,255,255],clock:{peer_id:'arbitrary-peer',id:'clock',hash:'h'},rejected:true});
});

test('PiP draws a binary frame with AR metadata and native picking intact',async({page})=>{
 // Keep this test independent of the size/activity of the running Home map.
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Test scene'}],preferred:'demo',runners:[],scenes:[],phones:[]}}));
 await page.route('**/api/scene',r=>r.fulfill({json:{frame:'home',voxel_size:.1,portals:[],notes:[],objects:[],voxels:[],sources:{}}}));
 await page.route('**/api/peer',r=>r.fulfill({json:{enabled:true,status:'Ready',peers:[]}}));
 await page.route('**/api/robot-occupancy*',r=>r.fulfill({json:{volumes:[]}}));
 for(const name of ['app.js','camera-frame.js'])await page.route('**/'+name,r=>r.fulfill({contentType:'text/javascript',body:readFileSync(new URL('../../web/'+name,import.meta.url),'utf8')}));
 let sequence=0,pick;
 await page.route('**/api/sensors**',r=>{
  if(!new URL(r.request().url()).searchParams.has('sensor'))return r.fulfill({json:{sensors:[{id:'arbitrary/pixels',peer_id:'arbitrary',label:'Generic binary camera',ar_available:true}]}});
  const meta=Buffer.from(JSON.stringify({width:4,height:3,encoding:'luma8',sequence:String(++sequence),capture_timestamp_ns:'9007199254740993',camera_token:'exact-capture',
   presentation:{rotation_clockwise_deg:90,source:'capture_pose'},ar:{status:'ready',projection:'sdk-camera',frame:'home',vertices:[],notes:[],voxel_count:0}}));
  const header=Buffer.alloc(4);header.writeUInt32LE(meta.length);
  return r.fulfill({contentType:'application/vnd.spatial-harness.camera-frame',body:Buffer.concat([header,meta,Buffer.alloc(12,180)])});
 });
 await page.route('**/api/label-pick',r=>{pick=r.request().postDataJSON();return r.fulfill({json:{hit:{token:'hit',position:[0,0,0],floor:true}}});});
 await page.goto('/');await page.waitForFunction(()=>document.querySelector('#scene-chat-form').onsubmit!==null);
 await expect(page.locator('#sensor-select option[value="arbitrary/pixels"]')).toHaveCount(1);
 await page.selectOption('#sensor-select','arbitrary/pixels',{force:true});
 await expect(page.locator('#sensor-feed-status')).toContainText('Receiving');
 await expect(page.locator('#sensor-stage')).toHaveAttribute('data-rotation','90');
 await page.locator('#sensor-ar').check();await expect(page.locator('#sensor-ar-overlay')).toBeVisible();
 await page.locator('#sensor-image').click({button:'right'});await expect(page.locator('#label-dialog-save')).toBeEnabled();
 expect(pick.camera_token).toBe('exact-capture');expect(pick.source).toBe('camera');
 await page.locator('#label-dialog-cancel').click();
});
