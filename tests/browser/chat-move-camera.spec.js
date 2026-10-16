import {test,expect} from 'playwright/test';
import {readFile} from 'node:fs/promises';

for(const mode of ['move','stale','2d','invalid','look-up'])test(`chat map-camera ${mode}`,async({page})=>{
 const stale=mode==='stale',invalid=mode==='invalid';
 const position=invalid?[0,0,0]:mode==='look-up'?[0,1,0]:[2,3,4];
 const lookAt=invalid?[0,0,0]:mode==='look-up'?[0,1.1,.1]:[0,1,0];
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Home Server'}],preferred:'demo',runners:[],scenes:[],phones:[]}}));
 await page.route('**/api/scene',r=>r.fulfill({json:{frame:'fixture-frame',voxels:[{center:[0,.5,0],evidence:1}],voxel_size:1,portals:[],notes:[],objects:[],sources:{},integrated_observations:0}}));
 await page.route('**/api/peer',r=>r.fulfill({json:{enabled:false,peers:[]}}));
 await page.route('**/api/sensors**',r=>r.fulfill({json:{sensors:[]}}));
 await page.route('**/*.js',async r=>{
  const path=new URL(r.request().url()).pathname;
  if(!/^\/[a-z0-9-]+\.js$/.test(path))return r.continue();
  let body=await readFile(new URL('../../web'+path,import.meta.url),'utf8');
  if(path==='/app.js')body+=`\nwindow.testCamera=()=>({position:camera.position.toArray(),target:controls.target.toArray()});
   const testStart=cameraPan.start.bind(cameraPan);cameraPan.start=(...args)=>{const before=window.testCamera();testStart(...args);window.testPan={before,after:window.testCamera()};};`;
  return r.fulfill({contentType:'text/javascript',body});
 });
 const requests=[];
 await page.route('**/api/chat',r=>{
  const body=r.request().postDataJSON();requests.push(body);
  return r.fulfill({json:requests.length>1?{reply:'View inspected.',model:'test',changes:[]}:{reply:'Requested view.',model:'test',changes:[],ui_actions:[{type:'move_camera',scene:body.scene,frame:stale?'stale-frame':body.viewer.frame,position,look_at:lookAt}]}});
 });
 await page.goto('/');
 await expect(page.locator('#map-select option[value="demo"]')).toHaveText('Home Server');
 await expect(page.locator('#connection')).toHaveText('Home Server online');
 if(mode==='2d'){await page.locator('#top').click();await expect(page.getByLabel('2D occupancy map',{exact:true})).toBeVisible();}
 await page.locator('#agent-toggle').click();await page.locator('#scene-chat-input').fill('Move the map camera to [2,3,4] looking at [0,1,0]');
 await page.locator('#scene-chat-form button[type="submit"]').click();
 await expect(page.locator('#scene-chat-history')).toContainText(stale?'Map camera not moved: Selected scene changed':invalid?'Map camera not moved: Invalid camera target distance':'Map camera moving.');
 if(!stale&&!invalid){
  const pan=await page.evaluate(()=>window.testPan);
  for(const key of ['position','target'])for(let i=0;i<3;i++)expect(pan.after[key][i]).toBeCloseTo(pan.before[key][i],10);
  expect(pan.after.position).not.toEqual(position); // No teleport when the tool is applied.
  await expect.poll(async()=>{const view=await page.evaluate(()=>window.testCamera());return Math.hypot(...view.position.map((n,i)=>n-position[i]),...view.target.map((n,i)=>n-lookAt[i]));}).toBeLessThan(1e-5);
 }
 await page.waitForTimeout(100); // Later orbit updates must not restore the old aim.
 await page.locator('#agent-toggle').click();await page.locator('#scene-chat-input').fill('What is my view?');
 await page.locator('#scene-chat-form button[type="submit"]').click();
 await expect.poll(()=>requests.length).toBe(2);
 const actual=requests[1].viewer,expected=(stale||invalid)?requests[0].viewer:{camera_position:position,camera_target:lookAt};
 if(!stale&&!invalid)expect(actual.mode).toBe('3d');
 for(const key of ['camera_position','camera_target'])for(let i=0;i<3;i++)expect(actual[key][i]).toBeCloseTo(expected[key][i],5);
 await expect(page.locator('#sensor-pip')).toBeHidden();
});
test('chat map-camera never goes beneath the grid',async({page})=>{
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Home Server'}],preferred:'demo',runners:[],scenes:[],phones:[]}}));
 const requests=[];
 await page.route('**/api/chat',r=>{const body=r.request().postDataJSON();requests.push(body);
  return r.fulfill({json:requests.length>1?{reply:'View inspected.',model:'test',changes:[]}:{reply:'Requested view.',model:'test',changes:[],ui_actions:[{type:'move_camera',scene:body.scene,frame:body.viewer.frame,position:[2,-3,4],look_at:[0,1,0]}]}});});
 await page.goto('/');await expect(page.locator('#connection')).toHaveText('Home Server online');
 await page.locator('#agent-toggle').click();await page.locator('#scene-chat-input').fill('Look up from under the floor');
 await page.locator('#scene-chat-form button[type="submit"]').click();await expect(page.locator('#scene-chat-history')).toContainText('Map camera moved.');
 await page.waitForTimeout(500);
 await page.locator('#agent-toggle').click();await page.locator('#scene-chat-input').fill('What is my view?');
 await page.locator('#scene-chat-form button[type="submit"]').click();await expect.poll(()=>requests.length).toBe(2);
 const [x,y,z]=requests[1].viewer.camera_position;
 expect(y).toBeGreaterThan(-.1);expect(x).toBeCloseTo(2,5);expect(z).toBeCloseTo(4,5);
});
