import {test,expect} from '@playwright/test';
import {readFile} from 'node:fs/promises';

test('network scene rows show the peer alias, with product details in a tooltip',async({page})=>{
 await page.route('**/app.js',async r=>r.fulfill({contentType:'text/javascript',body:await readFile(new URL('../../web/app.js',import.meta.url),'utf8')}));
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Home Server'}],runners:[],scenes:[
  {id:'peer-scene:robot-peer/canonical-scene',peer_id:'robot-peer',name:'canonical-scene',peer_alias:'Steve the Robot'},
  {id:'peer-scene:robot-peer/occupancy-2d',peer_id:'robot-peer',name:'occupancy-2d',peer_alias:'Steve the Robot'},
  {id:'peer-scene:other-peer/canonical-scene',peer_id:'other-peer',name:'canonical-scene'},
 ]}}));
 await page.goto('/');
 const row=page.locator('[data-map="peer-scene:robot-peer/canonical-scene"]');
 await expect(row).toHaveText('Steve the Robot');
 await expect(row).toHaveAttribute('title','canonical-scene · robot-peer');
 await expect(page.locator('[data-map="peer-scene:robot-peer/occupancy-2d"]')).toHaveAttribute('title','occupancy-2d · robot-peer');
 await expect(page.locator('[data-map="peer-scene:other-peer/canonical-scene"]')).toHaveText('canonical-scene · her-peer');
 await expect(page.locator('#map-select option[value="peer-scene:robot-peer/canonical-scene"]')).toHaveText('Steve the Robot');
});
