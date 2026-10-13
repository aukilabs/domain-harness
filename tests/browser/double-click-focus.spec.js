import {test,expect} from '@playwright/test';

for(const kind of ['note','volume','voxel'])test(`${kind}: single click selects; double click approaches to two metres`,async({page})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Home Server'}],preferred:'demo',runners:[],scenes:[],phones:[]}}));
 await page.route('**/api/scene',async r=>{
  const data=await (await r.fetch()).json();
  Object.assign(data,{simulation:false,portals:[],notes:[],objects:[],voxels:[],paths:null,landmarks:null,voxel_size:1});
  data.semantic_volumes={frame:data.frame,items:[]};
  if(kind==='note')data.notes=[{id:'test-note',text:'Close-up note',position:[0,.5,0],height_m:0}];
  if(kind==='volume')data.semantic_volumes.items=[{id:'test-volume',title:'Close-up volume',description:'',center:[0,.5,0],size:[1,1,1],yaw:0,semantic_type:'region'}];
  if(kind==='voxel')data.voxels=[{center:[0,.5,0],evidence:1}];
  await r.fulfill({json:data});
 });
 // Test-only inspection; no camera globals are added to the production viewer.
 await page.route('**/app.js',async r=>{
  const response=await r.fetch();await r.fulfill({response,body:(await response.text())+'\nwindow.testCamera=()=>({position:camera.position.toArray(),target:controls.target.toArray()});'});
 });
 await page.goto('/');await expect(page.locator('#connection')).toHaveText('Home Server online');
 await expect.poll(()=>page.evaluate(()=>!!window.testCamera)).toBeTruthy();
 const before=await page.evaluate(()=>window.testCamera());
 const canvas=page.locator('#canvas > canvas');let rect=await canvas.boundingBox();
 const offsetY=kind==='note'?-10:0;
 await page.mouse.click(rect.x+rect.width/2,rect.y+rect.height/2+offsetY);
 if(kind==='volume')await expect(page.locator('#volume-form')).toBeVisible();
 else await expect(page.locator('#selection')).toBeVisible();
 const selected=await page.evaluate(()=>window.testCamera());
 for(const key of ['position','target'])for(let i=0;i<3;i++)expect(selected[key][i]).toBeCloseTo(before[key][i],6);
 rect=await canvas.boundingBox();
 // Target the canvas, not the volume editor's overlaid drag handles.
 await canvas.dispatchEvent('dblclick',{button:0,clientX:rect.x+rect.width/2,clientY:rect.y+rect.height/2+offsetY});
 await expect.poll(async()=>{const v=await page.evaluate(()=>window.testCamera());return Math.hypot(...v.position.map((n,i)=>n-v.target[i]));}).toBeCloseTo(2,6);
 expect(errors).toEqual([]);
});
