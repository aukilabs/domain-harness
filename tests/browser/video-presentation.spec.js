import {test,expect} from '@playwright/test';
test('PiP rotates RGB and AR together; picks stay native; overrides are per feed',async({page})=>{
 let pick=null,rotation=90;
 await page.route('**/api/sensors*',r=>{
  const q=new URL(r.request().url()).searchParams;
  if(q.has('sensor'))return r.fulfill({json:{sequence:Date.now(),width:4,height:3,encoding:'luma8',data:Buffer.from([255,0,0,0,0,0,0,0,0,0,0,0]).toString('base64'),camera_token:'test-token',presentation:{rotation_clockwise_deg:rotation,source:'capture_pose'},ar:{status:'ready',projection:'sdk-camera',frame:'test',vertices:[],notes:[{text:'Test',pixel:[1,1]}],path_segments:[],voxel_count:0}}});
  return r.fulfill({json:{sensors:[{id:'p/a',peer_id:'p',label:'A',ar_available:true},{id:'q/b',peer_id:'q',label:'B',ar_available:true}]}});
 });
 await page.route('**/api/label-pick',r=>{pick=r.request().postDataJSON();return r.fulfill({json:{hit:{token:'hit',position:[0,0,0],floor:true}}});});
 await page.goto('/');
 await expect(page.locator('#sensor-select option[value="p/a"]')).toHaveCount(1);
 await page.selectOption('#sensor-select','p/a',{force:true});
 await expect(page.locator('#sensor-stage')).toHaveAttribute('data-rotation','90');
 await page.locator('#sensor-ar').check();
 await expect(page.locator('#sensor-ar-overlay')).toBeVisible();
 const control=page.getByLabel('Camera display orientation');
 for(const angle of [0,90,180,270]){
  await control.selectOption(String(angle));
  await expect(page.locator('#sensor-stage')).toHaveAttribute('data-rotation',String(angle));
  const image=await page.locator('#sensor-image').boundingBox(),overlay=await page.locator('#sensor-ar-overlay').boundingBox(),stage=await page.locator('#sensor-stage').boundingBox();
  expect(image.width/image.height).toBeCloseTo(angle%180?3/4:4/3,2);
  for(const key of ['x','y','width','height']){expect(image[key]).toBeCloseTo(stage[key],0);expect(overlay[key]).toBeCloseTo(image[key],0);}
  pick=null;
  await page.mouse.click(stage.x+stage.width*.25,stage.y+stage.height*.25,{button:'right'});
  await expect(page.locator('#label-dialog')).toBeVisible();
  await expect.poll(()=>pick?.pixel).not.toBeNull();
  const wanted={0:[1,.75],90:[1,2.25],180:[3,2.25],270:[3,.75]}[angle];
  expect(pick.pixel[0]).toBeCloseTo(wanted[0],1);expect(pick.pixel[1]).toBeCloseTo(wanted[1],1);
  await page.locator('#label-dialog-cancel').click();
 }
 rotation=0;
 await page.selectOption('#sensor-select','q/b',{force:true});
 await expect(control).toHaveValue('auto');
 await expect(page.locator('#sensor-stage')).toHaveAttribute('data-rotation','0');
 await page.selectOption('#sensor-select','p/a',{force:true});
 await expect(control).toHaveValue('270');
 await expect(page.locator('#sensor-stage')).toHaveAttribute('data-rotation','270');
 await control.selectOption('auto');
 await expect(page.locator('#sensor-stage')).toHaveAttribute('data-rotation','0');
});
