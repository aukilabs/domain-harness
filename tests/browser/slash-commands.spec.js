import {test,expect} from '@playwright/test';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import path from 'node:path';
const require=createRequire(import.meta.url);
const web=new URL('../../web/',import.meta.url);
const scene={frame:'home',convention:'OpenGL',voxel_size:.1,voxels:[],portals:[],notes:[],objects:[],sources:{},sensor_positions:{},integrated_observations:0,demo:false};
const ref={peer_id:'phone',product_id:'rgb',hash:'current'};
const inventory={scene:'demo',frame:'home',commands:[
 {name:'focus',description:'Move the view',args:[{name:'target',kind:'target'}]},
 {name:'path',description:'Draw a floor route',args:[{name:'waypoint',kind:'target',variadic:true,min:2,max:64}]},
 {name:'camera',description:'Open PiP',args:[{name:'peer',kind:'peer'},{name:'camera',kind:'camera'}]},
 {name:'rename',description:'Rename',args:[{name:'target',kind:'editable_target'},{name:'name',kind:'text'}]},
 {name:'note',description:'Create',args:[{name:'name',kind:'text'},...['x','y','z'].map(name=>({name,kind:'number'}))]},
],peers:[{id:'phone',name:"Dan's Phone",status:'connected',cameras:[{product_id:'rgb',reference:ref,frame_id:'optical'}]},{id:'robot',name:'Bracket 0125',status:'connected',cameras:[]}],entities:[{id:'one',name:'cheese',kind:'note'},{id:'two',name:'cheese',kind:'volume'}]};

async function setup(page,{localScene=false}={}){
 const calls=[],errors=[];page.on('pageerror',e=>errors.push(e.message));
 const currentScene=structuredClone(scene);
 // Entire browser fixture is intercepted; no Home process or live peer is used.
 await page.route('**/*',async route=>{
  const req=route.request(),u=new URL(req.url()),p=u.pathname;
  if(p==='/api/commands'&&req.method()==='POST'){
   const b=req.postDataJSON();calls.push({path:p,body:b});
   if(b.command==='path'){
    const stroke={id:'fixture-path',title:"Way from Dan's Phone to Bracket 0125",color:'#ff5fa8',width_m:.05,segments:[[[0,.06,0],[1,.06,0],[1,.06,2],[3,.06,2]]]};
    currentScene.paths={schema:'spatial-harness.scene-paths/v1',frame:'home',items:[stroke]};
    return route.fulfill({json:{reply:'Floor path saved. No robot movement.',source:'command',changes:[{operation:'create_path',summary:'Floor path saved.',after:stroke}],ui_actions:[]}});
   }
   return route.fulfill({json:{reply:'Direct command completed.',source:'command',changes:[],ui_actions:b.command==='focus'?[{type:'move_camera',scene:b.scene,frame:b.frame,position:[2,3,4],look_at:[0,0,0]}]:b.command==='camera'?[{type:'open_camera_pip',peer_id:'phone',sensor_id:'phone/rgb',product_id:'rgb'}]:[]}});
  }
  if(p==='/api/chat'){calls.push({path:p,body:req.postDataJSON()});return route.fulfill({json:{reply:'Natural-language response.',model:'test',changes:[]}});}
  if(p==='/api/commands')return route.fulfill({json:{...inventory,scene:u.searchParams.get('scene')}});
  if(p==='/api/scene')return route.fulfill({json:currentScene});
  if(p==='/api/fiducials')return route.fulfill({json:{families:[],fiducials:[]}});
  if(p==='/api/sensors')return route.fulfill({json:u.searchParams.has('sensor')?{width:4,height:3,encoding:'luma8',data:'AAAAAAAAAAAAAAAA',sequence:1}:{sensors:[{id:'phone/rgb',peer_id:'phone',label:"Dan's Phone · RGB"}]}});
  if(p==='/api/scene-sources')return route.fulfill({json:{local:[localScene?{id:'local:settings-test',name:'Settings test'}:{id:'demo',name:'Test scene'}],preferred:localScene?'local:settings-test':'demo',runners:[],scenes:[],phones:[]}});
  if(p==='/api/local-scenes/settings-test')return route.fulfill({json:currentScene});
  if(p==='/api/local-scenes/settings-test/wifi')return route.fulfill({json:{ssid:'Fixture network',has_password:true,hidden:false,allowed_peer_ids:[]}});
  if(p.startsWith('/api/'))return route.fulfill({json:{peers:[],sensors:[],poses:[],items:[],status:'disabled'}});
  if(p.startsWith('/vendor/')){
   const root=process.env.HOME_TEST_THREE||path.dirname(path.dirname(require.resolve('three')));
   const files={'three.module.js':'build/three.module.js','three.core.js':'build/three.core.js','OrbitControls.js':'examples/jsm/controls/OrbitControls.js','GLTFLoader.js':'examples/jsm/loaders/GLTFLoader.js','DRACOLoader.js':'examples/jsm/loaders/DRACOLoader.js','BufferGeometryUtils.js':'examples/jsm/utils/BufferGeometryUtils.js'};
   const file=files[p.split('/').at(-1)];return file?route.fulfill({contentType:'text/javascript',body:await readFile(path.join(root,file),'utf8')}):route.abort();
  }
  if(p==='/')return route.fulfill({contentType:'text/html',body:await readFile(new URL('index.html',web),'utf8')});
  if(/^\/[a-z0-9-]+\.(js|css)$/.test(p))return route.fulfill({contentType:p.endsWith('.js')?'text/javascript':'text/css',body:await readFile(new URL(p.slice(1),web),'utf8')});
  return route.abort();
 });
 await page.goto('http://home.test/');
 await expect(page.locator('#connection')).toHaveText('Home Server online',{timeout:20000});
 await expect(page.locator('.agent-bar')).toHaveAttribute('data-mode','tools');
 await page.locator('#agent-toggle').click();
 await expect(page.locator('#scene-chat-input')).toBeVisible();
 return {calls,errors,input:page.locator('#scene-chat-input'),send:page.getByRole('button',{name:'Send message',exact:true})};
}

test('full Home Settings keeps Scene Wi-Fi after panel construction and reopening',async({page})=>{
 const {errors,calls}=await setup(page,{localScene:true});
 await page.getByRole('button',{name:'Settings',exact:true}).click();
 const settings=page.locator('#panel-settings');
 await expect(settings.getByRole('region',{name:'Layers',exact:true})).toBeVisible();
 await expect(settings.getByRole('region',{name:'Display',exact:true})).toBeVisible();
 const wifi=settings.getByRole('button',{name:'Scene Wi-Fi',exact:true});
 await expect(wifi).toBeVisible();await expect(wifi).toBeEnabled();
 await wifi.click();
 await expect(page.getByRole('dialog')).toBeVisible();
 await expect(page.getByLabel('Network name (SSID)')).toHaveValue('Fixture network');
 await expect(page.getByLabel('Password',{exact:true})).toHaveValue('');
 await page.getByRole('button',{name:'Cancel',exact:true}).click();
 await page.getByRole('button',{name:'Maps',exact:true}).click();
 await page.getByRole('button',{name:'Settings',exact:true}).click();
 await expect(wifi).toBeVisible();await expect(wifi).toHaveCount(1);
 await page.screenshot({path:'test-results/full-home-scene-wifi.png'});
 expect(errors).toEqual([]);expect(calls).toEqual([]);
});

test('slash, peer and camera completion are keyboard accessible and bypass chat',async({page})=>{
 const {input,send,calls,errors}=await setup(page);
 await input.fill('/cam');await expect(page.getByRole('option')).toContainText('/camera');await input.press('Tab');
 await input.pressSequentially('@Dan');await expect(page.getByRole('option')).toContainText("Dan's Phone");await input.press('Enter');
 await expect(page.getByRole('option')).toContainText('rgb');await input.press('Tab');
 await expect(input).toHaveValue('/camera @"Dan\'s Phone" "rgb" ');
 await send.click();await expect(page.locator('#scene-chat-history')).toContainText('Camera PiP opened');await expect(page.locator('#sensor-pip')).toBeVisible();
 expect(calls).toHaveLength(1);expect(calls[0].path).toBe('/api/commands');expect(calls[0].body.args).toEqual([{type:'peer',id:'phone'},{type:'camera',reference:ref}]);expect(calls[0].body.viewer.frame).toBe('home');
 await expect(page.locator('#scene-chat-history strong').last()).toHaveText('Home');
 await input.fill('What is here?');await send.click();await expect(page.locator('#scene-chat-history')).toContainText('Natural-language response.');
 expect(calls[1].body.messages).toEqual([{role:'user',content:'What is here?'}]);expect(errors).toEqual([]);
});

test('duplicate entity names show type and ID, click chooses exact entity',async({page})=>{
 const {input,send,calls,errors}=await setup(page);
 await input.fill('/focus #che');await expect(page.getByRole('option')).toHaveCount(2);
 await page.getByRole('option').filter({hasText:'volume · two'}).click();await send.click();
 await expect(page.locator('#scene-chat-history')).toContainText('Map camera moving');
 expect(calls[0].body.args).toEqual([{type:'entity',kind:'volume',id:'two'}]);expect(calls[0].path).toBe('/api/commands');
 await page.screenshot({path:'test-results/slash-command-complete.png'});expect(errors).toEqual([]);
});

test('unresolved or edited references, malformed arguments and unknown commands never call chat',async({page})=>{
 const {input,send,calls}=await setup(page);
 await input.fill('/focus #cheese');await expect(page.getByRole('option')).toHaveCount(2);await input.press('Escape');await send.click();
 await expect(page.locator('#scene-chat-status')).toContainText('autocomplete');expect(calls).toHaveLength(0);
 await input.fill('/point @robot #cheese');await send.click();await expect(page.locator('#scene-chat-status')).toContainText('not exposed');
 await input.fill('/note "Hi" nope 1 2');await send.click();await expect(page.locator('#scene-chat-status')).toContainText('number');expect(calls).toHaveLength(0);
 await input.fill('/focus #che');await page.getByRole('option').first().click();await input.press('Backspace');await input.press('Backspace');await input.pressSequentially('x');await send.click();
 await expect(page.locator('#scene-chat-status')).toContainText('quoted argument');expect(calls).toHaveLength(0);
});

test('scene switch clears bound IDs and suggestions work on narrow screens',async({page})=>{
 const {input,send,calls}=await setup(page);await page.setViewportSize({width:440,height:820});
 await input.fill('/focus #che');await page.getByRole('option').first().click();
 await page.locator('#map-select').evaluate(el=>el.dispatchEvent(new Event('change',{bubbles:true})));
 await send.click();await expect(page.locator('#scene-chat-status')).toContainText('not ready');expect(calls).toHaveLength(0);
 await input.fill('/');await expect(page.getByRole('listbox')).toBeVisible();
 const box=await page.getByRole('listbox').boundingBox();expect(box.x).toBeGreaterThanOrEqual(0);expect(box.x+box.width).toBeLessThanOrEqual(440);
 await page.screenshot({path:'test-results/slash-command-suggestions.png'});
});

test('path autocompletes both peer endpoints and saves without chat or device commands',async({page})=>{
 const {input,send,calls,errors}=await setup(page);
 await input.fill('/pa');await expect(page.getByRole('option')).toContainText('/path');await input.press('Tab');
 await input.pressSequentially('@Dan');await page.getByRole('option').filter({hasText:"Dan's Phone"}).click();
 await input.pressSequentially('@Bracket');await expect(page.getByRole('option')).toContainText('Bracket 0125');await input.press('Tab');
 await expect(input).toHaveValue('/path @"Dan\'s Phone" @"Bracket 0125" ');
 await send.click();await expect(page.locator('#scene-chat-history')).toContainText('Floor path saved');
 expect(calls).toHaveLength(1);expect(calls[0].path).toBe('/api/commands');expect(calls[0].body.command).toBe('path');
 expect(calls[0].body.args).toEqual([{type:'peer',id:'phone'},{type:'peer',id:'robot'}]);
 await expect(page.locator('[data-path-id="fixture-path"]').first()).toBeAttached();
 await expect(page.locator('#scene-chat-history .scene-chat-message.assistant .scene-chat-text').last()).not.toContainText('segments');
 await expect(page.locator('#scene-chat-history .scene-chat-explanation')).toBeHidden();
 await expect(page.locator('#empty')).toBeHidden();
 await page.screenshot({path:'test-results/slash-path-saved.png'});expect(errors).toEqual([]);
});

test('path accepts mixed and entity endpoints and rejects an edited second selection',async({page})=>{
 const {input,send,calls}=await setup(page);
 await input.fill('/path #che');await page.getByRole('option').filter({hasText:'note · one'}).click();
 await input.pressSequentially('@Bracket');await page.getByRole('option').filter({hasText:'Bracket 0125'}).click();
 await send.click();await expect(page.locator('#scene-chat-history')).toContainText('Floor path saved');
 expect(calls[0].body.args).toEqual([{type:'entity',kind:'note',id:'one'},{type:'peer',id:'robot'}]);
 await input.fill('/path #che');await page.getByRole('option').filter({hasText:'note · one'}).click();
 await input.pressSequentially('#che');await page.getByRole('option').filter({hasText:'volume · two'}).click();
 await send.click();await expect.poll(()=>calls.length).toBe(2);
 expect(calls[1].body.args).toEqual([{type:'entity',kind:'note',id:'one'},{type:'entity',kind:'volume',id:'two'}]);
 await input.fill('/path @Dan');await page.getByRole('option').filter({hasText:"Dan's Phone"}).click();
 await input.pressSequentially('@Bracket');await page.getByRole('option').filter({hasText:'Bracket 0125'}).click();
 await input.press('Backspace');await input.press('Backspace');await input.pressSequentially('x');await send.click();
 await expect(page.locator('#scene-chat-status')).toContainText('quoted argument');expect(calls).toHaveLength(2);
});

test('path keeps offering autocomplete for third and later waypoints, without silently adding one',async({page})=>{
 const {input,send,calls,errors}=await setup(page);
 await input.fill('/path #che');await page.getByRole('option').filter({hasText:'note · one'}).click();
 await input.pressSequentially('#che');await page.getByRole('option').filter({hasText:'volume · two'}).click();
 await expect(page.getByRole('listbox')).toBeHidden();
 await input.pressSequentially('@Bracket');await expect(page.getByRole('option')).toContainText('Bracket 0125');await input.press('Tab');
 await expect(page.getByRole('listbox')).toBeHidden();
 await input.pressSequentially('@Dan');await page.getByRole('option').filter({hasText:"Dan's Phone"}).click();
 await expect(input).toHaveValue('/path #"cheese" #"cheese" @"Bracket 0125" @"Dan\'s Phone" ');
 await expect(page.getByRole('listbox')).toBeHidden();
 await send.click();await expect(page.locator('#scene-chat-history')).toContainText('Floor path saved');
 expect(calls).toHaveLength(1);expect(calls[0].body.args).toEqual([{type:'entity',kind:'note',id:'one'},{type:'entity',kind:'volume',id:'two'},{type:'peer',id:'robot'},{type:'peer',id:'phone'}]);
 expect(errors).toEqual([]);
});

test('ordinary Qwen messages autocomplete @peers and #nodes with stable identities',async({page})=>{
 const {input,send,calls,errors}=await setup(page);
 await input.fill('How far is @Dan');await expect(page.getByRole('option')).toHaveCount(1);
 await input.press('Tab');await input.pressSequentially('from #che');
 await expect(page.getByRole('option')).toHaveCount(2);
 await page.screenshot({path:'test-results/qwen-chat-references.png'});
 await page.getByRole('option').filter({hasText:'volume · two'}).click();
 await input.pressSequentially('?');await send.click();
 await expect.poll(()=>calls.length).toBe(1);expect(calls[0].path).toBe('/api/chat');
 const content=calls[0].body.messages[0].content,data=JSON.parse(content.split('\n').at(-1));
 expect(data.references.map(({type,id,kind})=>({type,id,kind}))).toEqual([{type:'peer',id:'phone',kind:undefined},{type:'entity',kind:'volume',id:'two'}]);
 await expect(page.locator('#scene-chat-history .user')).toContainText(`How far is @"Dan's Phone" from #"cheese" ?`);
 await expect(page.locator('#scene-chat-history')).not.toContainText('Selected references');
 await input.fill('And now?');await send.click();await expect.poll(()=>calls.length).toBe(2);
 expect(calls[1].body.messages[0].content).toBe(content);expect(errors).toEqual([]);
});

test('prose completion edits the caret token only, and Escape does not close chat',async({page})=>{
 const {input,send,calls}=await setup(page);
 await input.fill('Show #che, please');await input.evaluate(el=>{el.setSelectionRange(9,9);el.dispatchEvent(new Event('click'));});
 await expect(page.getByRole('option')).toHaveCount(2);await input.press('Escape');
 await expect(page.getByRole('listbox')).toBeHidden();await expect(input).toBeVisible();
 await input.evaluate(el=>el.dispatchEvent(new Event('click')));
 await page.getByRole('option').filter({hasText:'note · one'}).click();
 await expect(input).toHaveValue('Show #"cheese", please');await send.click();
 await expect.poll(()=>calls.length).toBe(1);expect(calls[0].path).toBe('/api/chat');
 expect(JSON.parse(calls[0].body.messages[0].content.split('\n').at(-1)).references[0].id).toBe('one');
});

test('prose never submits unresolved or scene-invalidated selections as confirmed identities',async({page})=>{
 const {input,send,calls}=await setup(page);
 await input.fill('Where is #cheese');await input.press('Escape');await send.click();
 await expect(page.locator('#scene-chat-status')).toContainText('autocomplete');expect(calls).toHaveLength(0);
 await input.fill('Where is #che');await page.getByRole('option').filter({hasText:'volume · two'}).click();
 await page.locator('#map-select').evaluate(el=>el.dispatchEvent(new Event('change',{bubbles:true})));
 await send.click();await expect(page.locator('#scene-chat-status')).toContainText('not ready');expect(calls).toHaveLength(0);
});

test('failed chat restores the selected identities for a manual retry',async({page})=>{
 const {input,send,calls}=await setup(page);let fail=true;
 await page.route('**/api/chat',route=>{if(fail){fail=false;return route.fulfill({status:503,json:{error:'Fixture unavailable'}});}return route.fallback();});
 await input.fill('Where is @Dan');await input.press('Tab');await send.click();
 await expect(input).toHaveValue(`Where is @"Dan's Phone" `);
 await expect(send).toBeEnabled();await send.click();await expect.poll(()=>calls.length).toBe(1);
 expect(JSON.parse(calls[0].body.messages[0].content.split('\n').at(-1)).references[0].id).toBe('phone');
});
