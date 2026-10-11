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
 await page.route('**/api/local-scenes/test/presence',r=>r.fulfill({json:{frame,poses:active?[{
  peer_id:'test-phone',name:'Phone',clock:{session_id:'session'},capture_timestamp_ns:stamp,
  pose:{to_frame_id:frame,translation:[stamp*.1,1,0],rotation_wxyz:[1,0,0,0]}}]:[]}}));
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
