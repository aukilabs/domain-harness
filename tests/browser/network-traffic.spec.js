import {test,expect} from '@playwright/test';
const phone='12D3KooWLBCGLksYWF3wxBDBSn1hN3HMsXo5dJG8r5VR64M2KpEE';
async function devices(page){
 await page.route('**/api/peer',async route=>{
  const response=await route.fetch(),body=await response.json();
  body.enabled=true;body.peers=[{peer_id:phone,name:'Test iPhone',status:'connected',catalog_status:'ready',catalog:{components:[],products:[]}}];
  await route.fulfill({json:body});
 });
}
async function openTraffic(page){
 await page.goto('/');await expect(page.locator('.agent-bar')).toBeVisible();
 await page.getByRole('button',{name:'Devices',exact:true}).click();
 await page.locator('#panel-fleet').getByRole('button',{name:'Test iPhone Live',exact:true}).click();
 await page.getByRole('button',{name:'Traffic with Home',exact:true}).click();
}
test('traffic belongs to a device, not Settings, and closing stops browser polling',async({page,request})=>{
 const response=await request.get('/api/network-traffic?peer_id='+phone);expect(response.ok()).toBeTruthy();
 const snapshot=await response.json();expect(snapshot.available).toBe(false);expect(snapshot.received_bytes).toBeUndefined();
 await devices(page);let requests=0,fail=false;
 await page.route('**/api/network-traffic?*',route=>{
  expect(new URL(route.request().url()).searchParams.get('peer_id')).toBe(phone);requests++;
  return fail?route.fulfill({status:503,body:'unavailable'}):route.fulfill({json:{session:'test',observer_peer_id:'home',remote_peer_id:phone,elapsed_ms:requests*1000,received_bytes:requests*2048,sent_bytes:requests*1024}});
 });
 await openTraffic(page);const panel=page.locator('#network-traffic');await expect(panel).toBeVisible();
 await expect(page.locator('#network-traffic-toggle')).toHaveCount(0);
 await expect(panel.locator('[data-rate="received"]')).toHaveText('2.0 KiB/s');
 await expect(panel.locator('[data-rate="sent"]')).toHaveText('1.0 KiB/s');
 await expect(panel.locator('.traffic-peer-identity')).toContainText(phone);
 await panel.getByRole('combobox',{name:'Gauge maximum'}).selectOption('1');
 await expect(panel.locator('.dial-value').first()).toHaveText('0.016 Mbps');
 const handle=panel.getByRole('button',{name:'Move network traffic panel'}),before=await panel.boundingBox();
 await handle.focus();await page.keyboard.press('ArrowLeft');expect((await panel.boundingBox()).x).toBe(before.x-10);
 fail=true;await expect(panel.getByRole('status')).toContainText('unavailable');
 await expect(panel.locator('[data-rate="received"]')).toHaveText('—');
 fail=false;await expect(panel.getByRole('status')).toContainText('Live');
 await panel.getByRole('button',{name:'Close network traffic',exact:true}).click();
 await expect(panel).toBeHidden();const count=requests;await page.waitForTimeout(1500);expect(requests).toBe(count);
 await page.getByRole('button',{name:'Traffic with Home',exact:true}).click();await expect(panel).toBeVisible();
});
test('peer launch comparison, errors and export use the selected pair',async({page})=>{
 await devices(page);
 const point={elapsed_ms:1000,interval_ms:1000,received:2000,sent:1000};
 const run={observer_peer_id:'home',remote_peer_id:phone,session:'current',started_at:'2026-10-15T12:00:00Z',last_sample_at:'2026-10-15T12:01:00Z',ended_cleanly:false,elapsed_ms:60000,received_bytes:60000,sent_bytes:30000,read_errors:3,write_errors:1,opened_streams:8,active_streams:2,peak_received:point,peak_sent:point,samples:[point],top_intervals:[point]};
 const prior={...run,session:'prior',started_at:'2026-10-14T12:00:00Z',ended_cleanly:true,peak_received:{...point,received:4000}};
 await page.route('**/api/network-traffic?*',r=>r.fulfill({json:{...run,diagnostics:{current:run,prior:[prior],saved_at:'2026-10-15T12:01:00Z',persistence_error:null}}}));
 await openTraffic(page);
 await expect(page.getByRole('combobox',{name:'Compare launch'})).toContainText('Previous launch');
 await expect(page.locator('.traffic-comparison tr').filter({hasText:'Peak receive'})).toContainText('-50.0%');
 await expect(page.locator('.traffic-comparison tr').filter({hasText:'Read I/O errors'})).toContainText('3');
 await expect(page.locator('.traffic-leaderboard tbody tr').first()).not.toContainText('This launch');
 const downloadPromise=page.waitForEvent('download');await page.getByRole('button',{name:'Export JSON'}).click();
 expect((await downloadPromise).suggestedFilename()).toMatch(/^home-traffic-.*\.json$/);
});
test('no peer measurements is displayed as unavailable, never aggregate zero',async({page})=>{
 await devices(page);await page.route('**/api/network-traffic?*',r=>r.fulfill({json:{available:false,reason:'No measured traffic for this peer in this launch'}}));
 await openTraffic(page);await expect(page.locator('#network-traffic').getByRole('status')).toContainText('No measured traffic');
 await expect(page.locator('[data-rate="received"]')).toHaveText('—');
});
