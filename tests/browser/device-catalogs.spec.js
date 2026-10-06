import {test,expect} from '@playwright/test';
test('normal Devices shows local and remote catalogs and clears unavailable entries',async({page})=>{
 const peer={peer_id:'android-peer',status:'connected',catalog_status:'loading'};
 const catalog={components:[{manifest:{component_id:'android-local-mapper'}}],products:[{manifest:{product_id:'phone-local-voxels'}}]};
 let peers=[peer];
 await page.route('**/api/peer',r=>r.fulfill({json:{enabled:true,status:'Ready',connection:{peer_id:'home-peer'},catalog:{components:[{manifest:{component_id:'home-phone-local-scenes'}}],products:[]},peers}}));
 await page.goto('/');await page.getByRole('button',{name:'Devices',exact:true}).click();
 const phone=page.locator('#fleet-sensors > details[data-peer="android-peer"]');
 await phone.locator('summary').first().click();await expect(phone).toContainText('Loading component catalog');
 peer.catalog_status='ready';peer.catalog=catalog;
 await expect(phone).toContainText('Component · android-local-mapper');await expect(phone).toContainText('Product · phone-local-voxels');
 const home=page.locator('#fleet-sensors > details[data-peer="home-peer"]');await home.locator('summary').first().click();await expect(home).toContainText('Component · home-phone-local-scenes');
 peer.catalog_status='unavailable';delete peer.catalog;
 await expect(phone).toContainText('Component catalog unavailable');await expect(phone.locator('[data-component]')).toHaveCount(0);
 peers=[];await expect(phone).toHaveCount(0);
});
