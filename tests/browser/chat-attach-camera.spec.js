import {test,expect} from 'playwright/test';
import {readFile} from 'node:fs/promises';

test('sensor POV follows full pose, pauses without guessing, detaches, and never requests video',async({page})=>{
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Home Server'}],preferred:'demo',runners:[],scenes:[],phones:[]}}));
 await page.route('**/*.js',async r=>{
  const path=new URL(r.request().url()).pathname;
  if(!/^\/[a-z0-9-]+\.js$/.test(path))return r.continue();
  return r.fulfill({contentType:'text/javascript',body:await readFile(new URL('../../web'+path,import.meta.url),'utf8')});
 });
 const sourceFrame={peer_id:'phone',frame_id:'optical',axes:{x:'right',y:'down',z:'forward'},handedness:'right',units:'meters'};
 let action,pose=[1,2,3],sequence=0,lost=false,frozen=false,videoCalls=0,followCalls=0;const requests=[];
 await page.route('**/api/sensors**',r=>{
  const q=new URL(r.request().url()).searchParams;
  if(q.get('follow')==='true'){
   followCalls++;
   if(lost)return r.fulfill({status:422,json:{error:'Sensor tracking unavailable'}});
   if(!frozen)sequence++;
   return r.fulfill({json:{frame:action.frame,sensor_id:'phone/poses',manifest_hash:'manifest',source_frame:sourceFrame,sequence,capture_timestamp_ns:sequence,clock:{session:'test'},age_ms:0,
    position:pose,look_at:[pose[0],pose[1],pose[2]-1],up:[1,0,0]}});
  }
  if(q.has('sensor'))videoCalls++;
  return r.fulfill({json:{sensors:[]}});
 });
 await page.route('**/api/chat',r=>{
  const body=r.request().postDataJSON();requests.push(body);
  action={type:'attach_camera',scene:body.scene,frame:body.viewer.frame,peer_id:'phone',alias:"Dan's Phone",frame_id:'optical',source_frame:sourceFrame,sensor_id:'phone/poses',pose_product:{manifest_hash:'manifest'}};
  const command=body.messages.at(-1).content;
  return r.fulfill({json:{reply:'Requested.',model:'test',changes:[],...(command==='Attach'?{ui_actions:[action]}:command==='Detach'?{ui_actions:[{type:'detach_camera',scene:body.scene,frame:body.viewer.frame}]}:{})}});
 });
 await page.goto('/');await expect(page.locator('#connection')).toHaveText('Home Server online');
 const send=async text=>{await page.locator('#agent-toggle').click();await page.locator('#scene-chat-input').fill(text);await page.locator('#scene-chat-form button[type="submit"]').click();};
 const panel=page.getByLabel('Attached camera',{exact:true});await expect(panel).toBeHidden();
 await send('Attach');await expect(panel).toContainText("Following · Dan's Phone");
 pose=[4,2,3];await expect.poll(()=>followCalls).toBeGreaterThan(2);
 await page.waitForTimeout(300);await send('Inspect');await expect.poll(()=>requests.length).toBe(2);
 for(let i=0;i<3;i++)expect(requests[1].viewer.camera_position[i]).toBeCloseTo(pose[i],5);
 expect(requests[1].viewer.screen_right[1]).toBeCloseTo(-1,5); // Roll is preserved.
 frozen=true;await expect(panel).toContainText('stale',{timeout:5000});
 lost=true;pose=[9,9,9];await expect(panel).toContainText('tracking unavailable');
 await send('Inspect');await expect.poll(()=>requests.length).toBe(3);
 expect(requests[2].viewer.camera_position).toEqual(requests[1].viewer.camera_position);
 await send('Detach');await expect(panel).toBeHidden();
 const stopped=followCalls;await page.waitForTimeout(500);expect(followCalls).toBe(stopped);
 expect(videoCalls).toBe(0);await expect(page.locator('#sensor-pip')).toBeHidden();
 lost=false;frozen=false;await send('Attach');await expect(panel).toContainText('Following');
 await page.locator('#agent-toggle').click();await page.locator('#scene-chat-input').focus();await page.keyboard.type('wasd');await expect(panel).toBeVisible();
 await page.locator('#scene-chat-input').evaluate(e=>e.blur());
 await page.keyboard.down('w');await expect(panel).toBeHidden();await page.waitForTimeout(250);await page.keyboard.up('w');
 const callsAfterMove=followCalls;await page.waitForTimeout(300);expect(followCalls).toBe(callsAfterMove);
 await send('Inspect');await expect.poll(()=>requests.at(-1)?.messages.at(-1)?.content).toBe('Inspect');
 expect(requests.at(-1).viewer.camera_position).not.toEqual(pose);
 await send('Attach');await expect(panel).toContainText('Following');
 await page.keyboard.press('Escape');await expect(panel).toBeHidden();
});
