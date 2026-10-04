import { test,expect } from '@playwright/test';
test('map, project, replay, inspect, and adapt to mobile',async({page,request})=>{
 test.setTimeout(120000);const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto('/');await expect(page.locator('#connection')).toHaveText('Home Server online');
 await expect(page.locator('#p2p-status')).toHaveText('P2P disabled');
 await expect(page.locator('#pairing-download')).toBeHidden();
 expect((await request.get('/api/home-connection')).status()).toBe(422);
 await expect(page.locator('.portal-row')).toHaveCount(13);
 const domain=await (await request.get('/api/domain-scene')).json();expect(domain.portals).toHaveLength(13);expect(domain.portals.every(p=>p.pose_in_map.translation[1]===0)).toBe(true);
 const initial=await (await request.get('/api/scene')).json();expect(initial.frame).toBe('domain:b2881524-954d-4979-b715-240ed8f02807:home-origin-UCOF1J40N7O');expect(initial.portals).toHaveLength(14);expect(initial.portals.find(p=>p.anchor_id==='UCOF1J40N7O').pose_in_map.translation).toEqual([0,0,0]);
 await page.getByRole('button',{name:'2ZWHV5L6I04 5 cm'}).click();await expect(page.locator('#selection')).toContainText('-2.0787');
 await page.getByRole('button',{name:'Reset camera view'}).click();
 await page.screenshot({path:'test-results/domain-portals.png'});
 await page.getByRole('button',{name:'Load two test peers'}).click();
 const fixtures=await (await request.get('/api/fixtures')).json();const scanCount=fixtures.contributions.reduce((n,c)=>n+c.observations.length,0);await expect(page.locator('#observation-count')).toHaveText(String(scanCount));
 await expect(page.locator('#empty')).toBeHidden();
 await expect(page.locator('#notice')).toContainText('Ready to project a detection');
 const projected=page.waitForResponse(r=>r.url().endsWith('/api/detections'));
 await page.getByRole('button',{name:'Project a book detection'}).click();
 expect((await projected).ok()).toBe(true);
 await expect(page.locator('#object-count')).toHaveText('1');
 await expect(page.locator('#selection')).toContainText('0.90');
 const before=await (await request.get('/api/scene')).json();
 await page.getByRole('button',{name:'Replay iPhone contribution'}).click();
 await expect(page.locator('#notice')).toContainText('Replay ignored');
 const after=await (await request.get('/api/scene')).json();expect(after).toEqual(before);
 await page.locator('[data-layer="voxels"]').uncheck();await page.locator('[data-layer="voxels"]').check();
 await page.getByRole('button',{name:'Top view',exact:true}).click();
 await page.getByRole('button',{name:'Reset camera view'}).click();
 await page.screenshot({path:'test-results/home-desktop.png'});
 await page.locator('#map-select').selectOption('domain');await expect(page.locator('#scene-title')).toHaveText('13 stored domain portals');
 await page.locator('#map-select').selectOption('demo');await expect(page.locator('#scene-title')).toHaveText('Domain + synthetic scene');
 await page.setViewportSize({width:390,height:844});
 expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
 await page.screenshot({path:'test-results/home-mobile.png'});
 const f=await (await request.get('/api/fixtures')).json();
 const rejected=await request.post('/api/contributions',{data:f.contributions[0],headers:{Origin:'https://foreign.example'}});expect(rejected.status()).toBe(422);
 expect(await (await request.get('/api/scene')).json()).toEqual(before);
 expect(errors).toEqual([]);
});

test('live runner pose is independent of scene updates and becomes stale or lost',async({page})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 let telemetry={status:'live',pose:{translation:[-4.071,.025,3.943],rotation_xyzw:[-.70710678,0,0,.70710678]},detail:'13 reference portals verified'};
 await page.route('**/api/runner-pose',route=>route.fulfill({json:telemetry}));
 await page.goto('/');
 await expect(page.locator('#runner-status')).toHaveText('Galbot · live');
 await expect(page.locator('#runner-card')).toHaveAttribute('data-position','[-4.071,0.025,3.943]');
 await expect(page.locator('#runner-card')).toHaveAttribute('data-rotation','[-0.70710678,0,0,0.70710678]');
 const observations=await page.locator('#observation-count').textContent();
 await page.getByRole('button',{name:'Focus Galbot'}).click();
 await expect(page.locator('#selection')).toContainText('Calibrated robot base');
 await page.screenshot({path:'test-results/runner-pose.png'});
 telemetry={...telemetry,pose:{...telemetry.pose,translation:[-3,.1,4]}};
 await expect(page.locator('#runner-card')).toHaveAttribute('data-position','[-3,0.1,4]');
 await expect(page.locator('#observation-count')).toHaveText(observations);
 telemetry={...telemetry,status:'stale'};
 await expect(page.locator('#runner-status')).toHaveText('Galbot · stale');
 await expect(page.locator('#runner-position')).toContainText('last known');
 await page.locator('#map-select').selectOption('domain');
 await expect(page.locator('#runner-card')).toHaveAttribute('data-position','[-3,0.1,4]');
 telemetry={...telemetry,status:'lost',pose:null};
 await expect(page.locator('#focus-runner')).toBeDisabled();
 await expect(page.locator('#runner-card')).toHaveAttribute('data-position','');
 expect(errors).toEqual([]);
});

test('live mode shows only real portals and hides the synthetic input controls',async({page,request})=>{
 const domain=await (await request.get('/api/domain-scene')).json();
 const live={...domain,demo:false,sdk_revision:'test',mode:'Live runner mapping',synthetic_layout:null,sensor_positions:{}};
 await page.route('**/api/scene',r=>r.fulfill({json:live}));
 await page.route('**/api/mapping',r=>r.fulfill({json:{status:'waiting',detail:'Waiting for measured LiDAR'}}));
 await page.goto('/');
 await expect(page.locator('#scene-title')).toHaveText('Canonical portals');
 await expect(page.locator('#voxel-count')).toHaveText('0');
 await expect(page.locator('#object-count')).toHaveText('0');
 await expect(page.locator('#demo-controls')).toBeHidden();
 await expect(page.locator('.flow')).toBeHidden();
 await expect(page.locator('#demo-view')).toBeHidden();
 await expect(page.locator('.portal-row')).toHaveCount(13);
 await expect(page.locator('#mapping-status')).toHaveText('Canonical reference');
});

test('scene selector switches owner, geometry and export without merging maps',async({page,request})=>{
 const domain=await (await request.get('/api/domain-scene')).json();
 const home={...domain,demo:false,integrated_observations:20,synthetic_layout:null,sensor_positions:{}};
 const robot={...home,integrated_observations:3,voxels:[{center:[-4,.25,4],linear_rgb:[1,0,0]}]};
 await page.route('**/api/scene',r=>r.fulfill({json:home}));
 await page.route('**/api/runner-scene',r=>r.fulfill({json:robot}));
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{status:'connected',detail:'Scenegraph built on Galbot'}}));
 await page.goto('/');await expect(page.locator('#observation-count')).toHaveText('20');
 await page.locator('#map-select').selectOption('runner');
 await expect(page.locator('#scene-title')).toHaveText('Galbot scenegraph');
 await expect(page.locator('#map-authority')).toHaveText('Galbot mapper');
 await expect(page.locator('#voxel-count')).toHaveText('1');
 await expect(page.locator('#observation-count')).toHaveText('3');
 await expect(page.locator('.portal-row')).toHaveCount(13);
 await expect(page.locator('.export')).toHaveAttribute('href','/api/runner-snapshot');
 await expect(page.locator('#inspect-snapshot')).toHaveAttribute('href','/api/runner-snapshot');
 await page.locator('#map-select').selectOption('demo');
 await expect(page.locator('#voxel-count')).toHaveText('0');
 await expect(page.locator('#map-authority')).toHaveText('Home Server');
 await expect(page.locator('#observation-count')).toHaveText('20');
 await page.route('**/api/runner-scene',r=>r.fulfill({status:422,json:{error:'not available'}}));
 await page.locator('#map-select').selectOption('runner');
 await expect(page.locator('#scene-title')).toHaveText('Waiting for Galbot scenegraph');
 await expect(page.locator('#voxel-count')).toHaveText('—');
 await expect(page.locator('#map-authority')).toHaveText('Galbot mapper');
});

test('sensor preview selects, renders, survives map changes, and closes',async({page})=>{
 await page.route('**/api/sensors*',async route=>{
  const selected=new URL(route.request().url()).searchParams.has('sensor');
  await route.fulfill({json:selected?{sequence:1,width:2,height:1,encoding:'rgb8',data:Buffer.from([255,0,0,0,0,255]).toString('base64')}:{sensors:[{id:'peer/camera',label:'Wrist camera'}]}});
 });
 await page.goto('/');
 await expect(page.locator('#sensor-select option')).toHaveCount(2);
 await page.selectOption('#sensor-select','peer/camera');
 await expect(page.locator('#sensor-pip')).toBeVisible();
 await expect(page.locator('#sensor-feed-status')).toContainText('Receiving');
 expect(await page.locator('#sensor-image').evaluate(c=>Array.from(c.getContext('2d').getImageData(0,0,2,1).data))).toEqual([255,0,0,255,0,0,255,255]);
 await page.selectOption('#map-select','domain');
 await expect(page.locator('#sensor-pip')).toBeVisible();
 await expect(page.locator('#sensor-feed-status')).toContainText('Stale');
 await page.getByRole('button',{name:'Close sensor preview'}).click();
 await expect(page.locator('#sensor-pip')).toBeHidden();
 await expect(page.locator('#sensor-select')).toHaveValue('');
});

test('observed local portals are blue and local telemetry never uses Home pose',async({page,request})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const domain=await (await request.get('/api/domain-scene')).json();
 const portal={...domain.portals[0],pose_in_map:{...domain.portals[0].pose_in_map,translation:[0,0,0],rotation_wxyz:[1,0,0,0]}};
 const local={...domain,frame:'galbot-local:test:opengl',local_map:true,demo:false,portals:[portal],local_mapping:{origin:{portal_code:portal.anchor_id},robot_pose:{pose:{translation:[1,2,3],rotation_xyzw:[0,0,0,1]}}},remote_status:{status:'connected',age_ms:10}};
 await page.route('**/api/runner-scene',r=>r.fulfill({json:local}));
 await page.route('**/api/runner-pose',r=>r.fulfill({json:{status:'live',pose:{translation:[99,99,99],rotation_xyzw:[0,0,0,1]}}}));
 await page.goto('/');await page.selectOption('#map-select','runner');
 await expect(page.locator('.portal-row')).toHaveCount(1);
 await expect(page.locator('#portal-swatch')).toHaveCSS('background-color','rgb(36, 119, 236)');
 await expect(page.locator('#frame-info')).toContainText('robot boot pose');
 await expect(page.locator('#runner-card')).toHaveAttribute('data-position','[1,2,3]');
 await expect(page.locator('#scene-note')).toContainText('Waiting for a shared portal');
 local.local_mapping.robot_pose=null;
 await expect(page.locator('#runner-card')).toHaveAttribute('data-position','');
 await page.selectOption('#map-select','domain');
 await expect(page.locator('.portal-row')).toHaveCount(13);
 await expect(page.locator('#portal-swatch')).toHaveCSS('background-color','rgb(229, 57, 53)');
 expect(errors).toEqual([]);
});

test('second Galbot has independent geometry, pose and export selection',async({page,request})=>{
 const domain=await (await request.get('/api/domain-scene')).json();
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const base={...domain,demo:false,local_map:true,local_mapping:{robot_pose:{pose:{translation:[1,2,3],rotation_xyzw:[0,0,0,1]}}},remote_status:{status:'connected',age_ms:5}};
 const first={...base,frame:'robot-one',integrated_observations:3};
 const second={...base,frame:'robot-two',integrated_observations:7,local_mapping:{robot_pose:{pose:{translation:[4,5,6],rotation_xyzw:[0,0,0,1]}}}};
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{status:'connected',runners:[{peer_id:'second-peer',name:'Galbot Echo (.87)',status:'connected',primary:false}]}}));
 await page.route('**/api/runner-scene*',r=>r.fulfill({json:new URL(r.request().url()).searchParams.get('peer')==='second-peer'?second:first}));
 await page.goto('/');
 await expect(page.locator('#map-select option[value="runner:second-peer"]')).toHaveCount(1);
 await page.selectOption('#map-select','runner:second-peer');
 await expect(page.locator('#observation-count')).toHaveText('7');
 await expect(page.locator('#runner-card')).toHaveAttribute('data-position','[4,5,6]');
 await expect(page.locator('.export')).toHaveAttribute('href','/api/runner-snapshot?peer=second-peer');
 await expect(page.locator('#map-authority')).toHaveText('Galbot Echo (.87) mapper');
 await page.selectOption('#map-select','runner');
 await expect(page.locator('#observation-count')).toHaveText('3');
 await expect(page.locator('#runner-card')).toHaveAttribute('data-position','[1,2,3]');
 await expect(page.locator('.export')).toHaveAttribute('href','/api/runner-snapshot');
 expect(errors).toEqual([]);
});

test('Home renders both calibrated runner poses and keeps stale peers separate',async({page,request})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const domain=await (await request.get('/api/domain-scene')).json();
 const sample=(position,rotation,stamp)=>({status:'aligned',robot_pose:{capture_timestamp_ns:stamp,pose:{translation:position,rotation_xyzw:rotation}}});
 const first=sample([1,0,2],[0,0,0,1],1),second=sample([5,0,4],[0,.70710678,0,.70710678],2);
 const shared={...domain,local_map:true,demo:false,frame:'home-live:test:opengl',local_mapping:{robot_pose:first.robot_pose,runners:{one:first,two:second}},remote_status:{status:'connected',age_ms:5}};
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{runners:[{peer_id:'one',name:'Galbot One',primary:true},{peer_id:'two',name:'Galbot Echo',primary:false}]}}));
 await page.route('**/api/home-live-scene',r=>r.fulfill({json:shared}));
 await page.goto('/');await page.selectOption('#map-select','home-live');
 const one=page.locator('.runner-pose-row[data-peer="one"]'),two=page.locator('.runner-pose-row[data-peer="two"]');
 await expect(page.locator('#runner-status')).toHaveText('2 Galbots · 2 live');
 await expect(one).toHaveAttribute('data-position','[1,0,2]');await expect(two).toHaveAttribute('data-position','[5,0,4]');
 await expect(two).toHaveAttribute('data-rotation','[0,0.70710678,0,0.70710678]');
 expect(await one.locator('strong').evaluate(e=>e.style.color)).not.toBe(await two.locator('strong').evaluate(e=>e.style.color));
 await page.getByRole('button',{name:'Focus Galbot Echo',exact:false}).click();await expect(page.locator('#selection')).toContainText('5.000, 0.000, 4.000');
 await page.locator('#runner-layer').uncheck();await expect(one).toHaveAttribute('data-visible','false');await expect(two).toHaveAttribute('data-visible','false');
 await page.locator('#runner-layer').check();await expect(two).toHaveAttribute('data-visible','true');
 // Per-peer metadata changes must update even if map geometry is unchanged.
 second.robot_pose.pose.translation=[6,0,4];second.robot_pose.capture_timestamp_ns++;
 await expect(two).toHaveAttribute('data-position','[6,0,4]');await expect(one).toHaveAttribute('data-position','[1,0,2]');
 first.status='disconnected';await expect(one).toHaveAttribute('data-status','stale');await expect(two).toHaveAttribute('data-status','live');
 await page.screenshot({path:'test-results/collaborative-poses.png'});
 delete shared.local_mapping.runners.one;await expect(one).toHaveCount(0);await expect(two).toHaveAttribute('data-visible','true');
 await page.selectOption('#map-select','domain');await expect(page.locator('#runner-list')).toBeHidden();await expect(two).toHaveCount(0);
 expect(errors).toEqual([]);
});

test('AR uses calibrated Home voxels, toggles cleanly and rejects stale camera overlays',async({page})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 let available=true,arRequests=0;
 const frame={sequence:1,width:100,height:80,encoding:'luma8',data:Buffer.alloc(8000,80).toString('base64'),ar:{status:'ready',frame:'home-live:test:opengl',intrinsics:{fx:80,fy:80,cx:35,cy:32},camera_to_map:{translation:[2,0,0],rotation_xyzw:[0,0,0,1]},voxel_size:.2,voxels:[{center:[2,0,2],linear_rgb:[0,1,1]},{center:[2,0,-2]}]}};
 await page.route('**/api/sensors*',r=>{
  const url=new URL(r.request().url());
  if(!url.searchParams.has('sensor'))return r.fulfill({json:{sensors:[{id:'peer/local-camera-history',label:'Calibrated camera',ar_available:true},{id:'peer/raw',label:'Uncalibrated camera',ar_available:false}]}});
  if(url.searchParams.get('ar')==='true'){arRequests++;return available?r.fulfill({json:frame}):r.fulfill({status:422,json:{error:'Waiting for a fresh calibrated camera frame'}});}
  return r.fulfill({json:{...frame,ar:undefined}});
 });
 await page.goto('/');await page.selectOption('#sensor-select','peer/local-camera-history');
 await expect(page.locator('#sensor-ar')).not.toBeChecked();await expect(page.locator('#sensor-ar-overlay')).toBeHidden();
 await expect(page.locator('#sensor-feed-status')).toContainText('Receiving');
 await page.getByLabel('AR',{exact:true}).check();
 const overlay=page.locator('#sensor-ar-overlay');await expect(overlay).toBeVisible();await expect(overlay).toHaveAttribute('data-frame','home-live:test:opengl');
 expect(arRequests).toBeGreaterThan(0);
 // Optical center lands at (cx,cy), not the middle of the preview; behind-camera cube is clipped.
 const pixels=await overlay.evaluate(c=>{const ctx=document.createElement('canvas').getContext('2d');ctx.canvas.width=c.width;ctx.canvas.height=c.height;ctx.drawImage(c,0,0);return [ctx.getImageData(35,32,1,1).data[3],ctx.getImageData(50,40,1,1).data[3]];});
 expect(pixels[0]).toBeGreaterThan(0);expect(pixels[1]).toBe(0);
 await page.selectOption('#map-select','domain');await expect(overlay).toHaveAttribute('data-frame','home-live:test:opengl');
 // Native fisheye feeds use SDK-projected pixel/depth triangles instead of a pinhole camera.
 frame.sequence++;
 frame.ar={status:'ready',projection:'sdk-camera',frame:'home-live:test:zup',voxel_count:1,vertices:[[30,25,2,0,1,1],[40,25,2,0,1,1],[35,38,2,0,1,1]]};
 await expect(overlay).toHaveAttribute('data-sequence','2');
 const alpha=await overlay.evaluate(c=>{const ctx=document.createElement('canvas').getContext('2d');ctx.canvas.width=c.width;ctx.canvas.height=c.height;ctx.drawImage(c,0,0);return ctx.getImageData(35,30,1,1).data[3];});
 expect(alpha).toBeGreaterThan(0);
 await expect(page.locator('#sensor-ar-status')).toContainText('SDK camera');
 await page.getByLabel('AR',{exact:true}).uncheck();await expect(overlay).toBeHidden();await expect(page.locator('#sensor-ar-status')).toBeHidden();
 await page.getByLabel('AR',{exact:true}).check();await expect(overlay).toBeVisible();
 available=false;await expect(overlay).toBeHidden();await expect(page.locator('#sensor-ar-status')).toContainText('AR paused');
 await page.selectOption('#sensor-select','peer/raw');await expect(page.getByLabel('AR',{exact:true})).toBeDisabled();await expect(overlay).toBeHidden();
 await expect(page.locator('#sensor-ar-status')).toContainText('calibration');
 await page.getByRole('button',{name:'Close sensor preview'}).click();await expect(page.locator('#sensor-pip')).toBeHidden();
 expect(errors).toEqual([]);
});

test('red robot volumes replace snapshots, preserve poses and expire',async({page,request})=>{
 const domain=await (await request.get('/api/domain-scene')).json();
 const map={...domain,frame:'home-live:test:opengl',local_map:true,local_mapping:{runners:{}},remote_status:{status:'connected',age_ms:0}};
 await page.route('**/api/home-live-scene',r=>r.fulfill({json:map}));
 let robots=[{peer_id:'one',frame:map.frame,model_sha256:'model',timestamp_ns:1,sequence:1,voxel_size:.05,cells:[[0,0,0],[0,1,0]],pose:{translation:[1,2,3],rotation_xyzw:[.5,.5,.5,.5]},covered_links:['body'],missing_links:[],approximate_links:[]}];
 await page.route('**/api/robot-occupancy*',r=>r.fulfill({json:{robots}}));
 await page.goto('/');await page.selectOption('#map-select','home-live');
 const status=page.locator('#robot-volume-status');await expect(status).toHaveAttribute('data-cells','2');
 await expect(status).toContainText('red cells · 2 Hz');
 await expect(status).toHaveAttribute('data-poses',JSON.stringify([{peer:'one',pose:robots[0].pose}]));
 robots=[{...robots[0],timestamp_ns:2,sequence:2,cells:[[20,0,0]]}];
 await expect(status).toHaveAttribute('data-cells','1');
 robots=[];await expect(status).toHaveAttribute('data-cells','0');
});


test('text notes persist in the scene and render in camera AR',async({page,request})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 let sequence=0;
 await page.route('**/api/sensors*',r=>{
  if(!new URL(r.request().url()).searchParams.has('sensor'))return r.fulfill({json:{sensors:[{id:'peer/camera',label:'Test camera',ar_available:true}]}});
  return r.fulfill({json:{sequence:++sequence,width:320,height:240,encoding:'luma8',data:Buffer.alloc(320*240,80).toString('base64'),ar:{status:'ready',projection:'sdk-camera',frame:'home-live:test:zup',vertices:[],voxel_count:0,notes:[{id:'abc',text:'abc',pixel:[160,120],depth_m:2}]}}});
 });
 await request.post('/api/notes',{data:{text:'abc',portal:'UCOF1J40N7O',height_m:1}});
 await page.goto('/');
 await expect(page.locator('#text-notes')).toHaveCount(0);
 await expect(page.locator('#canvas')).toHaveAttribute('data-note-count','1');
 const scene=await (await request.get('/api/scene')).json();expect(scene.notes[0].position).toEqual([0,1,0]);
 const snapshot=await (await request.get('/api/snapshots')).json();expect(snapshot.registries.text_notes.items[0].text).toBe('abc');
 await page.selectOption('#sensor-select','peer/camera');await page.getByLabel('AR',{exact:true}).check();
 const overlay=page.locator('#sensor-ar-overlay');await expect(overlay).toHaveAttribute('data-note-count','1');
 const alpha=await overlay.evaluate(c=>{const ctx=document.createElement('canvas').getContext('2d');ctx.canvas.width=c.width;ctx.canvas.height=c.height;ctx.drawImage(c,0,0);return ctx.getImageData(160,120,1,1).data[3];});expect(alpha).toBeGreaterThan(0);
 await page.getByLabel('AR',{exact:true}).uncheck();await expect(overlay).toBeHidden();
 expect(errors).toEqual([]);
});

test('right-click labels use Home voxel hits and the displayed camera frame',async({page})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const picked=[],saved=[];let miss=false,sequence=0;
 await page.route('**/api/label-pick',async r=>{picked.push(r.request().postDataJSON());await r.fulfill({json:{hit:miss?null:{token:'hit-token',position:[1,2,3],frame:'home'},beam_diameter_m:.5}});});
 await page.route('**/api/notes/from-pick',async r=>{saved.push(r.request().postDataJSON());await r.fulfill({json:{id:'saved'}});});
 await page.route('**/api/sensors*',r=>{
  if(!new URL(r.request().url()).searchParams.has('sensor'))return r.fulfill({json:{sensors:[{id:'peer/camera',label:'Calibrated camera',ar_available:true}]}});
  return r.fulfill({json:{sequence:++sequence,camera_token:'displayed-frame',width:320,height:240,encoding:'luma8',data:Buffer.alloc(320*240,80).toString('base64')}});
 });
 await page.goto('/');await expect(page.locator('#connection')).toHaveText('Home Server online');
 await page.locator('#canvas canvas').click({button:'right',position:{x:200,y:200}});
 await expect(page.locator('#label-dialog')).toBeVisible();await expect(page.locator('#label-dialog-text')).toBeEnabled();
 expect(picked[0].source).toBe('scene');expect(picked[0].origin).toHaveLength(3);
 await page.locator('#label-dialog-text').fill('Shelf');await page.getByRole('button',{name:'Save label',exact:true}).click();
 await expect(page.locator('#label-dialog')).not.toBeVisible();expect(saved).toEqual([{token:'hit-token',text:'Shelf'}]);
 await page.selectOption('#sensor-select','peer/camera');await expect(page.locator('#sensor-feed-status')).toContainText('Receiving');
 await page.locator('#sensor-image').click({button:'right',position:{x:100,y:80}});
 await expect(page.locator('#label-dialog-text')).toBeEnabled();
 expect(picked[1].source).toBe('camera');expect(picked[1].camera_token).toBe('displayed-frame');expect(picked[1].pixel[0]).toBeGreaterThan(0);
 // The video holds the image that was clicked while naming its surface.
 const held=await page.locator('#sensor-image').evaluate(c=>c.toDataURL());
 await page.getByRole('button',{name:'Cancel',exact:true}).click();expect(saved).toHaveLength(1);expect(held).toContain('data:image/png');
 await page.locator('#sensor-close').click();
 miss=true;await page.locator('#canvas canvas').click({button:'right',position:{x:200,y:200}});
 await expect(page.locator('#label-dialog-status')).toContainText('No occupied Home voxel');await expect(page.getByRole('button',{name:'Save label',exact:true})).toBeDisabled();
 await page.keyboard.press('Escape');await expect(page.locator('#label-dialog')).not.toBeVisible();expect(saved).toHaveLength(1);
 expect(errors).toEqual([]);
});

test('selected annotations can be deleted during live updates and disappear from AR',async({page,request})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const created=await request.post('/api/notes',{data:{text:'Delete this test annotation',portal:'UCOF1J40N7O',height_m:.5}});
 expect(created.ok()).toBeTruthy();const note=await created.json();let requests=0,sequence=0;
 const notes=async()=>((await (await request.get('/api/scene')).json()).notes||[]).filter(n=>n.id===note.id);
 await page.route('**/api/home-live-scene',async r=>r.fulfill({json:{frame:'home-live:test:opengl',request_count:++requests,notes:await notes(),voxels:[],portals:[],objects:[],sources:{},integrated_observations:0,voxel_size:.1,local_map:true}}));
 await page.route('**/api/sensors*',async r=>{
  if(!new URL(r.request().url()).searchParams.has('sensor'))return r.fulfill({json:{sensors:[{id:'peer/camera',label:'Test camera',ar_available:true}]}});
  return r.fulfill({json:{sequence:++sequence,width:320,height:240,encoding:'luma8',data:Buffer.alloc(320*240,80).toString('base64'),ar:{status:'ready',projection:'sdk-camera',frame:'home-live:test:zup',vertices:[],voxel_count:0,notes:(await notes()).map(n=>({...n,pixel:[160,120],depth_m:2}))}}});
 });
 await page.goto('/');await page.selectOption('#map-select','home-live');
 await expect(page.locator('#canvas')).toHaveAttribute('data-note-count','1');
 const remove=page.getByRole('button',{name:'Delete annotation',exact:true});
 // The map change eases the orbit camera toward its target; click once it settles.
 await expect(async()=>{await page.locator('#canvas canvas').click();await expect(remove).toBeVisible({timeout:500});}).toPass({timeout:10000});
 const before=requests;await expect.poll(()=>requests).toBeGreaterThan(before+2);await expect(remove).toBeVisible();
 await page.selectOption('#sensor-select','peer/camera');await page.getByLabel('AR',{exact:true}).check();
 await expect(page.locator('#sensor-ar-overlay')).toHaveAttribute('data-note-count','1');
 const denied=await request.delete('/api/notes/'+note.id,{headers:{Origin:'https://foreign.invalid'}});expect(denied.ok()).toBeFalsy();expect(await notes()).toHaveLength(1);
 await remove.click();await expect(page.locator('#canvas')).toHaveAttribute('data-note-count','0');
 await expect(page.locator('#sensor-ar-overlay')).toHaveAttribute('data-note-count','0');
 expect(await notes()).toHaveLength(0);await expect(remove).toHaveCount(0);
 await page.reload();expect(await notes()).toHaveLength(0);
 expect(errors).toEqual([]);
});
