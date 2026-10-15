import {test,expect} from 'playwright/test';
import {readFile} from 'node:fs/promises';

for(const mode of ['move','stale','2d','invalid','look-up'])test(`chat map-camera ${mode}`,async({page})=>{
 const stale=mode==='stale',invalid=mode==='invalid';
 const position=invalid?[0,0,0]:mode==='look-up'?[0,1,0]:[2,3,4];
 const lookAt=invalid?[0,0,0]:mode==='look-up'?[0,1.1,.1]:[0,1,0];
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Home Server'}],preferred:'demo',runners:[],scenes:[],phones:[]}}));
 await page.route('**/*.js',async r=>{
  const path=new URL(r.request().url()).pathname;
  if(!/^\/[a-z0-9-]+\.js$/.test(path))return r.continue();
  return r.fulfill({contentType:'text/javascript',body:await readFile(new URL('../../web'+path,import.meta.url),'utf8')});
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
 await expect(page.locator('#scene-chat-history')).toContainText(stale?'Map camera not moved: Selected scene changed':invalid?'Map camera not moved: Invalid camera target distance':'Map camera moved.');
 await page.waitForTimeout(500); // Several animation ticks must not restore old damping/aim.
 await page.locator('#agent-toggle').click();await page.locator('#scene-chat-input').fill('What is my view?');
 await page.locator('#scene-chat-form button[type="submit"]').click();
 await expect.poll(()=>requests.length).toBe(2);
 const actual=requests[1].viewer,expected=(stale||invalid)?requests[0].viewer:{camera_position:position,camera_target:lookAt};
 if(!stale&&!invalid)expect(actual.mode).toBe('3d');
 for(const key of ['camera_position','camera_target'])for(let i=0;i<3;i++)expect(actual[key][i]).toBeCloseTo(expected[key][i],5);
 await expect(page.locator('#sensor-pip')).toBeHidden();
});
