import {test,expect} from '@playwright/test';
test('Bracketbot renders its reported body, replaces snapshots and never substitutes a glyph',async({page,request})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const base=await(await request.get('/api/domain-scene')).json();
 const pose={translation:[1,2,3],rotation_xyzw:[0,0,0,1]};
 const map={...base,authority_peer_id:'bb',frame:'bracketbot-local:test:opengl',source_frame:'bracketbot-local:test:zup',local_map:true,
  local_mapping:{robot_pose:{pose,glyph_pose:pose}},remote_status:{status:'connected',age_ms:0}};
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{runners:[{peer_id:'bb',name:'Bracketbot',primary:true,status:'connected',available:true}]}}));
 await page.route('**/api/runner-scene*',r=>r.fulfill({json:map}));
 let bodies=[{peer_id:'bb',frame:map.frame,model_sha256:'model',timestamp_ns:1,sequence:1,voxel_size:.05,cells:[[0,0,0],[0,1,0]],pose,
   covered_links:['body'],missing_links:[],approximate_links:['body']}];
 await page.route('**/api/robot-occupancy*',r=>r.fulfill({json:{robots:bodies}}));
 await page.goto('/');await page.selectOption('#map-select','runner',{force:true});
 const status=page.locator('#robot-volume-status');
 await expect(status).toHaveAttribute('data-cells','2');
 await expect(page.locator('.runner-pose-row[data-peer="selected"]')).toHaveAttribute('data-visible','false');
 bodies=[{...bodies[0],sequence:2,timestamp_ns:2,cells:[[20,0,0]]}];
 await expect(status).toHaveAttribute('data-cells','1');
 const last=bodies[0];
 bodies=[];await expect(status).toHaveAttribute('data-cells','1');
 await expect(status).toHaveAttribute('data-stale','1');
 await expect(status).toContainText('last known position · tracking lost · not live');
 await expect(page.locator('.runner-pose-row[data-peer="selected"]')).toHaveAttribute('data-visible','false');
 bodies=[{...last,sequence:3,timestamp_ns:3}];
 await expect(status).toHaveAttribute('data-stale','0');
 bodies=[];map.frame='bracketbot-local:new-session:opengl';map.source_frame='bracketbot-local:new-session:zup';
 await expect(status).toHaveAttribute('data-cells','0');
 expect(errors).toEqual([]);
});
