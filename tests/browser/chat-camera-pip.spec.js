import {test,expect} from '@playwright/test';
import {readFile} from 'node:fs/promises';

for(const available of [true,false])test(`chat PiP ${available?'opens exactly one requested camera':'rejects a vanished camera'}`,async({page})=>{
 const frames=[];
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Home Server'}],preferred:'demo',runners:[],scenes:[],phones:[]}}));
 await page.route('**/style.css',async r=>r.fulfill({contentType:'text/css',body:await readFile(new URL('../../web/style.css',import.meta.url),'utf8')}));
 await page.route('**/*.js',async r=>{
  const path=new URL(r.request().url()).pathname;
  if(!/^\/[a-z0-9-]+\.js$/.test(path))return r.continue();
  return r.fulfill({contentType:'text/javascript',body:await readFile(new URL('../../web'+path,import.meta.url),'utf8')});
 });
 await page.route('**/api/sensors**',r=>{
  const id=new URL(r.request().url()).searchParams.get('sensor');
  if(id){frames.push(id);return r.fulfill({json:{width:4,height:3,encoding:'luma8',data:'AAAAAAAAAAAAAAAA',sequence:1,camera_token:'capture-token',ar:{status:'ready',projection:'sdk-camera',frame:'local-scene:test:portal-HNY3JGNLRAE',vertices:[],notes:[],voxel_count:0}}});}
  return r.fulfill({json:{sensors:available?[{id:'phone/rgb',peer_id:'phone',label:"Dan's Phone · RGB",ar_available:true},{id:'robot/left',peer_id:'robot',label:'Robot left'}]:[]}});
 });
 let cameraPick=null;
 await page.route('**/api/label-pick',r=>{
  cameraPick=r.request().postDataJSON();
  return r.fulfill({json:{hit:{token:'surface-token',position:[1,0,2],floor:false}}});
 });
 await page.route('**/api/chat',r=>r.fulfill({json:{reply:'Camera selected.',model:'test',changes:[],ui_actions:[{type:'open_camera_pip',peer_id:'phone',sensor_id:'phone/rgb',product_id:'rgb'}]}}));
 await page.goto('/');
 await page.waitForFunction(()=>document.querySelector('#scene-chat-form').onsubmit!==null);
 await expect(page.locator('#map-select option[value="demo"]')).toHaveText('Home Server');
 await page.locator('#scene-chat-input').fill("Pull up Dan's Phone's camera");
 await page.locator('#scene-chat-form button[type="submit"]').click();
 if(available){
  await expect(page.locator('#sensor-pip')).toBeVisible();
  await expect(page.locator('#sensor-select')).toHaveValue('phone/rgb');
  await expect(page.locator('#sensor-feed-status')).toContainText('Receiving');
  expect(frames.length).toBeGreaterThan(0);expect(new Set(frames)).toEqual(new Set(['phone/rgb']));
  await expect(page.locator('#scene-chat-history')).toContainText('Camera PiP opened');
  const pip=page.locator('#sensor-pip'),heading=page.locator('#sensor-title'),grip=page.locator('.pip-resize');
  await expect(grip).toBeVisible();
  const before=await pip.boundingBox(),title=await heading.boundingBox();
  await page.mouse.move(title.x+20,title.y+title.height/2);await page.mouse.down();
  await page.mouse.move(title.x-140,title.y-160,{steps:5});await page.mouse.up();
  const moved=await pip.boundingBox();expect(moved.x).toBeLessThan(before.x-100);expect(moved.y).toBeLessThan(before.y-100);
  const corner=await grip.boundingBox();
  await page.mouse.move(corner.x+12,corner.y+12);await page.mouse.down();
  await page.mouse.move(corner.x+92,corner.y+72,{steps:5});await page.mouse.up();
  const resized=await pip.boundingBox();expect(resized.width).toBeGreaterThan(moved.width+60);expect(resized.height).toBeGreaterThan(moved.height+40);
  const image=await page.locator('#sensor-image').boundingBox(),stage=await page.locator('#sensor-stage').boundingBox();
  expect(image.width/image.height).toBeCloseTo(4/3,2);expect(image.width).toBeCloseTo(stage.width,1);expect(image.height).toBeCloseTo(stage.height,1);
  await page.locator('#sensor-ar').check();
  await expect(page.locator('#sensor-ar-overlay')).toBeVisible();
  await page.locator('#sensor-image').click({button:'right'});
  await expect(page.locator('#label-dialog')).toBeVisible();
  await expect(page.locator('#label-dialog-save')).toBeEnabled();
  expect(cameraPick.source).toBe('camera');expect(cameraPick.camera_token).toBe('capture-token');
  expect(cameraPick.pixel[0]).toBeCloseTo(2,1);expect(cameraPick.pixel[1]).toBeCloseTo(1.5,1);
  await page.locator('#label-dialog-cancel').click();
  // Keyboard controls and viewport changes cannot strand the window off-screen.
  await heading.focus();await page.keyboard.press('ArrowLeft');
  expect((await pip.boundingBox()).x).toBeLessThan(resized.x);
  await page.setViewportSize({width:800,height:600});
  await expect.poll(async()=>{
   const p=await pip.boundingBox(),v=await page.locator('.viewport').boundingBox();
   return p.x>=v.x&&p.y>=v.y&&p.x+p.width<=v.x+v.width+1&&p.y+p.height<=v.y+v.height+1;
  }).toBe(true);
  expect(new Set(frames)).toEqual(new Set(['phone/rgb']));
  await page.locator('#sensor-close').click();await expect(pip).toBeHidden();
 }else{
  await expect(page.locator('#scene-chat-history')).toContainText('Camera not opened: Camera is no longer available');
  await expect(page.locator('#sensor-pip')).toBeHidden();expect(frames).toEqual([]);
 }
});
