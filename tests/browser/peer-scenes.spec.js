import {test,expect} from '@playwright/test';
test('discovered scene product is selectable and removed when unavailable',async({page})=>{
 let online=true;const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const id='peer-scene:robot-peer/local-scene';
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Home Server'}],scenes:online?[{id,name:'local-scene',peer_id:'robot-peer'}]:[]}}));
 await page.route('**/api/sensors?scene=true&sensor=*',r=>r.fulfill({json:{frame:'simulator-local:test:opengl',local_map:true,product_id:'local-scene',authority_peer_id:'robot-peer',convention:'OpenGL',local_mapping:{detail:'Independent robot mapping'},voxel_size:.1,portals:[],objects:[],notes:[],sources:{},integrated_observations:3,voxels:[{center:[0,0,-1]}]}}));
 await page.goto('/');await page.getByRole('button',{name:'Maps',exact:true}).click();
 await page.locator('[data-map="'+id+'"]').click();
 await expect(page.locator('#scene-title')).toHaveText('local-scene');
 await expect(page.locator('#voxel-count')).toHaveText('1');
 await expect(page.locator('#scene-badge')).toHaveText('ROBOT-LOCAL MAP');
 await expect(page.locator('#portal-info')).toContainText('not merged into Home');
 online=false;await expect(page.locator('[data-map="'+id+'"]')).toHaveCount(0);
 expect(errors).toEqual([]);
});

test('discovered occupancy product renders the existing tile layer',async({page})=>{
 const id='peer-scene:robot-peer/occupancy-2d';const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Home Server'}],scenes:[{id,name:'occupancy-2d',peer_id:'robot-peer'}]}}));
 await page.route('**/api/sensors?scene=true&sensor=*',r=>r.fulfill({json:{frame:'simulator-local:test:opengl',local_map:true,product_id:'occupancy-2d',authority_peer_id:'robot-peer',convention:'OpenGL',layer_type:'occupancy_2d',occupancy_status:'ready',occupancy_counts:{occupied:1,free:1,unknown:1},voxel_size:.1,portals:[],objects:[],notes:[],sources:{},voxels:[],occupancy_tiles:[{center:[0,0,-1],value:100},{center:[.1,0,-1],value:0},{center:[.2,0,-1],value:-1}]}}));
 await page.goto('/');await page.getByRole('button',{name:'Maps',exact:true}).click();
 await page.locator('[data-map="'+id+'"]').click();
 await expect(page.locator('#scene-badge')).toHaveText('OCCUPANCY 2D');
 await expect(page.locator('#canvas')).toHaveAttribute('data-occupancy-count','3');
 await expect(page.locator('#mapping-detail')).toHaveText('1 occupied · 1 free · 1 unknown');
 await expect(page.locator('#updated')).toHaveText('Occupancy · ready');
 expect(errors).toEqual([]);
});
