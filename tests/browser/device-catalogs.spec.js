import {test,expect} from '@playwright/test';
// Devices is a list; choosing a device opens its detail view in the panel.
async function openDevice(page,id){if(await page.locator('#device-detail').count())await page.getByRole('button',{name:'All devices',exact:true}).click();await page.locator(`#fleet-sensors .device-item[data-peer="${id}"] .list-row`).click();return page.locator(`#device-detail[data-peer="${id}"]`);}
test('Maps lists a phone-owned Catalog map once, independently of the push cache',async({page})=>{
 const peer='android-peer',id='peer-scene:'+peer+'/phone-local-voxels';
 let phones=[{id:'phone:'+peer,name:'Android cached map',peer_id:peer}],requests=0;
 await page.route('**/api/scene-sources',r=>{
  requests++;
  return r.fulfill({json:{local:[{id:'demo',name:'Home'}],phones,scenes:[{id,name:'phone-local-voxels',peer_id:peer}],runners:[]}});
 });
 await page.route('**/api/sensors?scene=true&sensor=*',r=>r.fulfill({json:{
  product_id:'phone-local-voxels',authority_peer_id:peer,frame:'phone-local:android-peer:session',
  convention:'OpenGL · Y up · meters',local_map:true,voxel_size:0.1,
  portals:[],objects:[],notes:[],sources:{},integrated_observations:0,voxels:[{center:[0.05,0.05,0.05]}]
 }}));
 await page.goto('/');
 const network=page.locator('#map-select optgroup[label="Network"] option');
 await expect(network).toHaveCount(1);
 await expect(network).toHaveAttribute('value',id);
 await expect(network).toContainText('Phone · local map');
 await page.selectOption('#map-select',id,{force:true});
 await expect(page.locator('#scene-title')).toHaveText('Phone · local voxel map');
 await expect(page.locator('#scene-badge')).toHaveText('PHONE-LOCAL MAP');
 await expect(page.locator('#voxel-count')).toHaveText('1');
 const before=requests;
 phones=[];
 await expect.poll(()=>requests).toBeGreaterThan(before);
 await expect(network).toHaveCount(1);
 await expect(network).toHaveAttribute('value',id);
});
test('Devices groups types and puts camera feeds first',async({page})=>{
 const component=(id,payload)=>({manifest_hash:id,manifest:{component_id:id,observables:[]},current_outputs:{out:{manifest:{payload}}}});
 const catalog={components:[component('z-camera',{kind:'camera'}),component('a-map',{modality:'scenegraph'}),component('b-pose',{datatype:'auki.pose-update/v1'}),component('a-camera',{kind:'camera'}),component('camera-name-only',{datatype:'custom/v1'})],products:[{manifest:{product_id:'camera-history',producer:{component_id:'z-camera',component_manifest_hash:'z-camera',slot:'out'}}}]};
 let count=1;
 await page.route('**/api/peer',r=>r.fulfill({json:{enabled:true,status:'Ready',connection:{peer_id:'home'},peers:[{peer_id:'robot',name:'Robot',status:'connected',catalog_status:'ready',catalog,received_observations:count}]}}));
 await page.route('**/api/sensors*',r=>r.fulfill({json:new URL(r.request().url()).searchParams.has('sensor')?{sequence:1,width:1,height:1,encoding:'rgb8',data:'AAAA'}:{sensors:[{id:'robot/camera',peer_id:'robot',label:'Front camera'}]}}));
 await page.goto('/');await page.getByRole('button',{name:'Devices',exact:true}).click();
 const robot=await openDevice(page,'robot');
 const feeds=robot.getByRole('region',{name:'Camera feeds',exact:true});await expect(feeds.getByRole('button',{name:'Front camera',exact:true})).toBeVisible();
 expect(await robot.locator(':scope > section').evaluateAll(nodes=>nodes.map(n=>n.getAttribute('aria-label')))).toEqual(['Camera feeds','Capabilities']);
 expect(await robot.locator('.device-capabilities > section').evaluateAll(nodes=>nodes.map(n=>n.getAttribute('aria-label')))).toEqual(['Cameras','Maps','Poses & odometry','Other']);
 const cameras=robot.getByRole('region',{name:'Cameras',exact:true});
 expect(await cameras.locator('summary').allTextContents()).toEqual(['Component · a-camera','Component · z-camera']);
 await expect(cameras.locator('[data-component="z-camera"] .capability-facts dt:text-is("Product") + dd')).toContainText('camera-history');
 await expect(robot.getByRole('region',{name:'Other',exact:true})).toContainText('camera-name-only');
 await cameras.getByRole('button',{name:'Technical details for Z camera',exact:true}).click();count=2;
 await expect(robot.locator('[data-observations]')).toHaveAttribute('data-observations','2');
 await expect(cameras.locator('[data-component="z-camera"] details')).toHaveAttribute('open','');
 await feeds.getByRole('button',{name:'Front camera',exact:true}).click();await expect(page.locator('#sensor-pip')).toBeVisible();
 await expect(feeds.getByRole('button',{name:'Front camera',exact:true})).toHaveAttribute('aria-pressed','true');
});
test('normal Devices shows local and remote catalogs and clears unavailable entries',async({page})=>{
 const peer={peer_id:'android-peer',status:'connected',catalog_status:'loading'};
 const catalog={components:[{manifest:{component_id:'phone-local-mapper'}}],products:[{manifest:{product_id:'phone-local-voxels'}}]};
 let peers=[peer];
 await page.route('**/api/peer',r=>r.fulfill({json:{enabled:true,status:'Ready',connection:{peer_id:'home-peer'},catalog:{components:[{manifest:{component_id:'home-phone-local-scenes'}}],products:[]},peers}}));
 await page.goto('/');await page.getByRole('button',{name:'Devices',exact:true}).click();
 let phone=await openDevice(page,'android-peer');await expect(phone).toContainText('Loading component catalog');
 peer.catalog_status='ready';peer.catalog=catalog;
 await expect(phone).toContainText('Component · phone-local-mapper');await expect(phone).toContainText('Product · phone-local-voxels');
 peer.catalog_cached=true;peer.catalog_refresh_status='unavailable';
 await expect(phone).toContainText('Catalog refresh delayed');
 await expect(phone).toContainText('Product · phone-local-voxels');
 delete peer.catalog_cached;delete peer.catalog_refresh_status;
 const home=await openDevice(page,'home-peer');await expect(home).toContainText('Component · home-phone-local-scenes');
 peer.catalog_status='unavailable';delete peer.catalog;
 phone=await openDevice(page,'android-peer');await expect(phone).toContainText('Component catalog unavailable');await expect(phone.locator('[data-component]')).toHaveCount(0);
 peers=[];await expect(phone).toHaveCount(0);
});
