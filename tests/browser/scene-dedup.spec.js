import {test,expect} from '@playwright/test';

test('named runner replaces only its matching discovered scene',async({page})=>{
 let available=true;
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{
  local:[{id:'demo',name:'Home Server'}],
  runners:[{peer_id:'robot-peer',name:'bot1 · Bracketbot 0184',primary:true,status:'connected',available}],
  scenes:[
   {id:'peer-scene:robot-peer/canonical-scene',peer_id:'robot-peer',name:'canonical-scene'},
   {id:'peer-scene:robot-peer/occupancy-2d',peer_id:'robot-peer',name:'occupancy-2d'},
   {id:'peer-scene:other-peer/canonical-scene',peer_id:'other-peer',name:'canonical-scene'},
  ],
 }}));
 await page.goto('/');
 await expect(page.locator('#map-select option[value="runner"]')).toHaveText('bot1 · Bracketbot 0184');
 await expect(page.locator('#map-select option[value="peer-scene:robot-peer/canonical-scene"]')).toHaveCount(0);
 await expect(page.locator('#map-select option[value="peer-scene:robot-peer/occupancy-2d"]')).toHaveCount(1);
 await expect(page.locator('#map-select option[value="peer-scene:other-peer/canonical-scene"]')).toHaveCount(1);
 available=false;
 await expect(page.locator('#map-select option[value="peer-scene:robot-peer/canonical-scene"]')).toHaveCount(1);
 await expect(page.locator('#map-select option[value="runner"]')).toHaveCount(0);
});
