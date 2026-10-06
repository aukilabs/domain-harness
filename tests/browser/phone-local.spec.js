import {test,expect} from '@playwright/test';
test('phone local map is separate, correctly labelled, and removed when unavailable',async({page})=>{
 let online=true; const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Home Server'}],runners:[],phones:online?[{id:'phone:test-phone',name:'Android · local voxels (unaligned)'}]:[]}}));
 await page.route('**/api/phone-local-scene?peer=test-phone',r=>r.fulfill({json:{frame:'phone-local:test:session',local_map:true,voxel_size:.1,portals:[],objects:[],notes:[],sources:{},integrated_observations:0,voxels:[{center:[0,0,-1]}]}}));
 await page.goto('/');await page.getByRole('button',{name:'Maps',exact:true}).click();
 await page.locator('[data-map="phone:test-phone"]').click();
 await expect(page.locator('#scene-title')).toHaveText('Android · local voxel map');
 await expect(page.locator('#voxel-count')).toHaveText('1');
 await expect(page.locator('#scene-note')).toContainText('Real ARCore depth');
 online=false;await expect(page.locator('[data-map="phone:test-phone"]')).toHaveCount(0);
 expect(errors).toEqual([]);
});
