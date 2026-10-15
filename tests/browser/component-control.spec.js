import {test,expect} from '@playwright/test';
import {readFileSync} from 'node:fs';
// Devices is a list; choosing a device opens its detail view in the panel.
async function openDevice(page,id){if(await page.locator('#device-detail').count())await page.getByRole('button',{name:'All devices',exact:true}).click();await page.locator(`#fleet-sensors .device-item[data-peer="${id}"] .list-row`).click();return page.locator(`#device-detail[data-peer="${id}"]`);}
test('Devices discovers publication controls for an arbitrary peer and waits for confirmation',async({page})=>{
 await page.route('**/app.js',r=>r.fulfill({contentType:'text/javascript',body:readFileSync(new URL('../../web/app.js',import.meta.url),'utf8')}));
 const catalog={components:[{manifest_hash:'component-hash',manifest:{component_id:'custom-sensor',observables:[],operables:[{name:'set_enabled',instruction:'spatial-harness.set-enabled/v1',result:'spatial-harness.component-enabled-state/v1'}]}}],products:[{manifest_hash:'state-hash',manifest:{product_id:'custom-state',producer:{component_id:'custom-sensor',component_manifest_hash:'component-hash'}},metadata:{schema:'spatial-harness.component-enabled-state/v1',source_sequence:1,value:{enabled:true}}}]};
 await page.route('**/api/peer',r=>r.fulfill({json:{enabled:true,status:'Ready',connection:{peer_id:'home'},peers:[{peer_id:'arbitrary-peer',name:'Independent sensor',catalog_status:'ready',catalog}]}}));
 await page.route('**/api/sensors*',r=>r.fulfill({json:{sensors:[]}}));
 let pending;
 await page.route('**/api/component-enabled',r=>{pending=r;});
 await page.goto('/');await page.getByRole('button',{name:'Devices',exact:true}).click();
 const peer=await openDevice(page,'arbitrary-peer');
 const component=peer.locator('[data-component="custom-sensor"]');
 const toggle=component.getByRole('switch',{name:'Custom sensor',exact:true});await expect(toggle).toHaveAttribute('aria-checked','true');await toggle.click();
 await expect.poll(()=>!!pending).toBe(true);
 expect(pending.request().postDataJSON()).toEqual({peer_id:'arbitrary-peer',component_id:'custom-sensor',manifest_hash:'component-hash',enabled:false});
 await expect(toggle).toBeDisabled();
 await pending.fulfill({json:{enabled:false}});
 await expect(toggle).toBeEnabled();await expect(toggle).toHaveAttribute('aria-checked','false');
 await expect(component.getByRole('status')).toHaveText('Disabled');
 pending=null;await toggle.click();
 await expect.poll(()=>!!pending).toBe(true);
 await pending.fulfill({status:422,json:{error:'Not authorized'}});
 await expect(component.getByRole('status')).toHaveText('Not authorized');
 await expect(toggle).toBeDisabled();
});
