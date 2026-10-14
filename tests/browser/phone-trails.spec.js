import { test, expect } from '@playwright/test';
import { resolve } from 'node:path';
test('phone trail fades without new data and clears on a map switch',async({page,request})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const base=await(await request.get('/api/domain-scene')).json();
 const frame='local-scene:test:portal-ABCDEFGHIJK';
 const scene={...base,frame,notes:[],demo:false,environment_name:'Phone test'};
 let stamp=1,active=true;
 await page.route('**/app.js',r=>r.fulfill({path:resolve('web/app.js'),contentType:'text/javascript'}));
 await page.route('**/phone-trails.js',r=>r.fulfill({path:resolve('web/phone-trails.js'),contentType:'text/javascript'}));
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Home'},{id:'local:test',name:'Phone test'}],runners:[]}}));
 await page.route('**/api/local-scenes/test',r=>r.fulfill({json:scene}));
 await page.route('**/api/peer',r=>r.fulfill({json:{enabled:true,status:'Ready',peers:[{peer_id:'test-phone',name:'Phone',catalog_status:'ready',catalog:{products:[{manifest:{product_id:'pose'},manifest_hash:'hash',metadata:{schema:'spatial-harness.compact-pose-contract/v1',value:{from_frame:{id:'phone-camera:session'}}}}]}}]}}));
 await page.route('**/api/sensors?*',async r=>{
  const query=new URL(r.request().url()).searchParams;
  if(query.get('debug_pose')!=='true')return r.fulfill({json:{sensors:[]}});
  expect(query.get('home_frame')).toBe(frame);
  return r.fulfill({json:{stream:'live',age_ms:0,timestamp_ns:String(stamp),resolved:{contract:{session_id:'session'}},home_pose:{frame,alignment_timestamp_ns:'1',tracking:active?{status:'valid',pose:{translation:[stamp*.1,1,0],rotation_xyzw:[0,0,0,1]}}:{status:'lost'}}}});
 });
 await page.goto('/');await page.selectOption('#map-select','local:test',{force:true});
 await expect(page.locator('#canvas')).toHaveAttribute('data-phone-ghost-count','1');
 stamp=2;await expect(page.locator('#canvas')).toHaveAttribute('data-phone-ghost-count','2');
 active=false;await expect(page.locator('#canvas')).toHaveAttribute('data-phone-live-count','0');
 await expect(page.locator('#canvas')).toHaveAttribute('data-phone-ghost-count','0',{timeout:15000});
 active=true;stamp=3;await expect(page.locator('#canvas')).toHaveAttribute('data-phone-ghost-count','1');
 await page.selectOption('#map-select','demo',{force:true});
 await expect(page.locator('#canvas')).toHaveAttribute('data-phone-ghost-count','0');
 expect(errors).toEqual([]);
});
