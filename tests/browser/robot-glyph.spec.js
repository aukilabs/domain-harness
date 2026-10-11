import { test, expect } from '@playwright/test';

test('independent and shared maps apply SDK glyph poses while retaining body poses', async ({ page, request }) => {
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const base=await (await request.get('/api/domain-scene')).json();
 const body={translation:[1,2,3],rotation_xyzw:[-Math.SQRT1_2,0,0,Math.SQRT1_2]};
 const bracketbot={capture_timestamp_ns:1,pose:body,glyph_pose:{...body,rotation_xyzw:[-.5,.5,.5,.5]}};
 const ros={capture_timestamp_ns:1,pose:{...body,translation:[4,2,3]},glyph_pose:{...body,translation:[4,2,3]}};
 const local={...base,frame:'bracketbot-local:test:opengl',local_map:true,demo:false,
  local_mapping:{robot_pose:bracketbot},remote_status:{status:'connected',age_ms:5}};
 const shared={...local,frame:'home-live:test:opengl',local_mapping:{runners:{
  bb:{status:'aligned',robot_pose:bracketbot},ros:{status:'aligned',robot_pose:ros}}}};
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'home-live',name:'Shared map'}],runners:[
  {peer_id:'bb',name:'Bracketbot',primary:true,status:'connected',available:true},
  {peer_id:'ros',name:'ROS robot',primary:false,status:'connected',available:true}]}}));
 await page.route('**/api/runner-scene*',r=>r.fulfill({json:local}));
 await page.route('**/api/home-live-scene',r=>r.fulfill({json:shared}));
 await page.goto('/');await page.selectOption('#map-select','runner',{force:true});
 await expect(page.locator('#runner-card')).toHaveAttribute('data-rotation','[-0.5,0.5,0.5,0.5]');
 await expect(page.locator('#runner-card')).toHaveAttribute('data-position','[1,2,3]');
 await page.selectOption('#map-select','home-live',{force:true});
 const bb=page.locator('.runner-pose-row[data-peer="bb"]');
 const other=page.locator('.runner-pose-row[data-peer="ros"]');
 await expect(bb).toHaveAttribute('data-rotation','[-0.5,0.5,0.5,0.5]');
 await expect(other).toHaveAttribute('data-rotation',JSON.stringify(ros.pose.rotation_xyzw));
 // A pose update must keep using the presentation transform, independently per peer.
 bracketbot.glyph_pose.rotation_xyzw=[0,1,0,0];bracketbot.capture_timestamp_ns++;
 await expect(bb).toHaveAttribute('data-rotation','[0,1,0,0]');
 await expect(other).toHaveAttribute('data-position','[4,2,3]');
 expect(bracketbot.pose.rotation_xyzw).toEqual(body.rotation_xyzw);
 expect(errors).toEqual([]);
});
