import { test, expect } from '@playwright/test';
// Devices is a list; choosing a device opens its detail view in the panel.
async function openDevice(page,id){if(await page.locator('#device-detail').count())await page.getByRole('button',{name:'All devices',exact:true}).click();await page.locator(`#fleet-sensors .device-item[data-peer="${id}"] .list-row`).click();return page.locator(`#device-detail[data-peer="${id}"]`);}

test('retained occupancy remains usable while nonexistent voxel scenes are removed',async({page,request})=>{
 const base=await(await request.get('/api/scene')).json();
 let offline=false,disconnected=false;
 await page.route('**/api/scene-sources',r=>offline?r.abort():r.fulfill({json:{local:[{id:'demo',name:'Home Server'},{id:'sim-voxels',name:'Robot voxels',available:!disconnected},{id:'sim-occupancy',name:'Home occupancy',available:true,status:disconnected?'retained':'live'}],runners:[]}}));
 await page.route('**/api/simulation/occupancy',r=>offline?r.abort():r.fulfill({json:{...base,simulation:true,simulation_status:disconnected?'disconnected':'running',local_mapping:{},occupancy_tiles:[],occupancy_status:disconnected?'stale':'ready'}}));
 await page.goto('/');await page.getByRole('button',{name:'Maps',exact:true}).click();
 await page.locator('[data-map="sim-occupancy"]').click();
 offline=true;
 await expect(page.locator('#scene-availability')).toContainText('Home offline');
 await expect(page.locator('[data-map="sim-occupancy"]')).toBeDisabled();
 offline=false;disconnected=true;
 await expect(page.locator('#scene-availability')).toContainText('Retained on Home');
 await expect(page.locator('[data-map="sim-voxels"]')).toHaveCount(0);
 await expect(page.locator('[data-map="sim-occupancy"]')).toBeEnabled();
 await expect(page.locator('[data-map="sim-occupancy"]')).toContainText('retained');
});

test('Devices exposes real-inventory fields, component catalogs and observation counts',async({page})=>{
 const peer={peer_id:'robot-peer-1',name:'Robot 1',status:'connected',received_observations:4,catalog:{components:[{manifest:{component_id:'fake-voxel-source'}}],products:[{manifest:{peer_id:'robot-peer-1',product_id:'fake-voxels-scene',producer:{component_id:'fake-voxel-source'}}}]}};
 await page.route('**/api/peer',r=>r.fulfill({json:{enabled:true,status:'Simulation · running',connection:{peer_id:'home-peer'},peers:[peer]}}));
 await page.goto('/');await page.getByRole('button',{name:'Devices',exact:true}).click();
 const robot=await openDevice(page,'robot-peer-1');
 await expect(robot).toContainText('robot-peer-1');
 await expect(robot).toContainText('Component · fake-voxel-source');
 await expect(robot.locator('.capability-facts dt:text-is("Product") + dd')).toContainText('fake-voxels-scene');
 await expect(robot).toContainText('4 observations received over P2P');
 peer.received_observations=5;
 await expect(robot).toContainText('5 observations received over P2P');
 peer.status='disconnected';await expect(robot).toContainText('disconnected');
});

test('Home switches between simulated voxels and occupancy without mixing maps', async ({ page, request }) => {
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const base=await (await request.get('/api/scene')).json();
 const source={...base,simulation:true,simulation_status:'running',simulation_detail:'Loopback test peers',demo:false,portals:[],objects:[],notes:[],landmarks:null,
  sources:{simulation:1},integrated_observations:1,voxel_size:.1,voxels:[{center:[0,.25,0],evidence:1}],
  local_mapping:{robot_pose:{pose:{translation:[1.8,0,0],rotation_xyzw:[0,0,0,1]}}}};
 const output={...source,voxels:[],occupancy_status:'ready',occupancy_tiles:[
  {center:[0,0,0],value:100},{center:[.1,0,0],value:0},{center:[.2,0,0],value:-1}]};
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Home Server'},{id:'sim-voxels',name:'Simulated robot · voxels'},{id:'sim-occupancy',name:'Simulated robot · occupancy 2D'}],runners:[]}}));
 await page.route('**/api/simulation/voxels',r=>r.fulfill({json:source}));
 await page.route('**/api/simulation/occupancy',r=>r.fulfill({json:output}));
 await page.goto('/');
 await page.getByRole('button',{name:'Maps',exact:true}).click();
 await page.locator('[data-map="sim-voxels"]').click();
 await expect(page.locator('#scene-title')).toHaveText('Simulated robot · voxels');
 await expect(page.locator('#voxel-count')).toHaveText('1');
 await expect(page.locator('#runner-card')).toHaveAttribute('data-status','live');
 await expect(page.locator('#runner-card')).toHaveAttribute('data-position','[1.8,0,0]');
 await page.locator('[data-map="sim-occupancy"]').click();
 await expect(page.locator('#canvas')).toHaveAttribute('data-occupancy-count','3');
 await expect(page.locator('#portal-info')).toContainText('unknown (-1)');
 await expect(page.locator('#inspect-snapshot')).toHaveAttribute('href','/api/simulation/occupancy?snapshot=true');
 output.local_mapping.robot_pose.pose.translation=[0,0,1.8];
 await expect(page.locator('#runner-card')).toHaveAttribute('data-position','[0,0,1.8]');
 output.simulation_status='failed';
 await expect(page.locator('#runner-card')).toHaveAttribute('data-status','stale');
 await expect(page.locator('#mapping-status')).toContainText('failed');
 output.simulation_status='running';
 output.local_mapping.runners={
  r1:{name:'Robot 1',robot_pose:{pose:{translation:[1.8,0,0],rotation_xyzw:[0,0,0,1]}}},
  r2:{name:'Robot 2',robot_pose:{pose:{translation:[-1.8,0,0],rotation_xyzw:[0,0,0,1]}}}
 };
 await expect(page.locator('#runner-card')).toHaveAttribute('data-pose-count','2');
 await expect(page.locator('#runner-list [data-peer="r1"]')).toHaveAttribute('data-position','[1.8,0,0]');
 await expect(page.locator('#runner-list [data-peer="r2"]')).toHaveAttribute('data-position','[-1.8,0,0]');
 output.local_mapping.runners.r2.robot_pose.pose.translation=[-1.5,0,.5];
 await expect(page.locator('#runner-list [data-peer="r2"]')).toHaveAttribute('data-position','[-1.5,0,0.5]');
 await page.locator('[data-map="demo"]').click();
 await expect(page.locator('#canvas')).toHaveAttribute('data-occupancy-count','0');
 expect(errors).toEqual([]);
});
