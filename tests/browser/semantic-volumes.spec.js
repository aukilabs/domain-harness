import {test,expect} from '@playwright/test';

test('draw, save, reload, edit, isolate and delete a semantic volume',async({page,request})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto('/');
 await page.getByRole('button',{name:'Maps',exact:true}).click();
 await page.locator('[data-map="local:humanoid-hub-conference"]').click();
 await expect(page.locator('#scene-title')).toHaveText('Humanoid Hub Conference');
 await page.getByRole('button',{name:'Volumes',exact:true}).click();
 await page.getByRole('button',{name:'New volume',exact:true}).click();
 const canvas=page.locator('#canvas canvas'),rect=await canvas.boundingBox();
 await page.mouse.move(rect.x+rect.width*.42,rect.y+rect.height*.42);
 await page.mouse.down();await page.mouse.move(rect.x+rect.width*.62,rect.y+rect.height*.62,{steps:10});await page.mouse.up();
 const form=page.locator('#volume-form');
 await expect(page.locator('#volume-status')).toContainText('Front edge ready');
 await form.getByLabel('Title',{exact:true}).fill('Meeting area');
 await form.getByLabel('Description',{exact:true}).fill('Space for the team to gather.');
 await form.getByLabel('Height',{exact:true}).fill('2.4');
 await form.getByLabel('Base elevation',{exact:true}).fill('.2');
 await page.getByRole('button',{name:'Save volume',exact:true}).click();
 await expect(page.locator('#volume-status')).toHaveText('Volume saved in this scene.');
 await expect(page.locator('#volume-list')).toContainText('Meeting area');
 const url='/api/semantic-volumes?scene=local:humanoid-hub-conference';
 const saved=await (await request.get(url)).json();expect(saved.items).toHaveLength(1);
 const v=saved.items[0];expect(v.size[0]).toBeGreaterThan(.3);expect(v.size[2]).toBe(.6);expect(v.yaw).not.toBe(0);expect(v.size[1]).toBe(2.4);expect(v.center[1]).toBe(1.4);
 const scene=await (await request.get('/api/local-scenes/humanoid-hub-conference')).json();expect(scene.semantic_volumes).toEqual(saved);
 await expect(canvas).toHaveAttribute('data-semantic-volume-count','1');
 await page.reload();await page.getByRole('button',{name:'Maps',exact:true}).click();
 await page.locator('[data-map="local:humanoid-hub-conference"]').click();
 await expect(page.locator('#scene-title')).toHaveText('Humanoid Hub Conference');
 await page.getByRole('button',{name:'Volumes',exact:true}).click();
 await page.locator(`[data-volume-id="${v.id}"]`).click();await page.getByRole('button',{name:'Edit',exact:true}).click();
 await expect(form.getByLabel('Description',{exact:true})).toHaveValue('Space for the team to gather.');
 await form.getByLabel('Title',{exact:true}).fill('Team zone');await form.getByLabel('Rotation (degrees)',{exact:true}).fill('45');
 await page.getByRole('button',{name:'Save volume',exact:true}).click();
 await expect(page.locator('#volume-list')).toContainText('Team zone');
 expect((await (await request.get(url)).json()).items[0].yaw).toBeCloseTo(Math.PI/4);
 await page.getByRole('button',{name:'Maps',exact:true}).click();await page.locator('[data-map="local:nacs"]').click();
 await expect(page.locator('#scene-title')).toHaveText('NACS');await page.getByRole('button',{name:'Volumes',exact:true}).click();
 await expect(page.locator('#volume-list')).toContainText('No semantic volumes');await expect(canvas).toHaveAttribute('data-semantic-volume-count','0');
 await page.getByRole('button',{name:'Maps',exact:true}).click();await page.locator('[data-map="local:humanoid-hub-conference"]').click();
 await expect(page.locator('#scene-title')).toHaveText('Humanoid Hub Conference');await page.getByRole('button',{name:'Volumes',exact:true}).click();
 await page.locator(`[data-volume-id="${v.id}"]`).click();
 await page.screenshot({path:'test-results/semantic-volumes.png'});
 await page.getByRole('button',{name:'Delete',exact:true}).click();await page.getByRole('button',{name:'Delete volume',exact:true}).click();
 await expect(page.locator('#volume-status')).toHaveText('Volume deleted.');
 expect((await (await request.get(url)).json()).items).toHaveLength(0);
 expect(errors).toEqual([]);
});

test('Peyote-style move and corner rotation commit on release with a fixed hinge',async({page,request})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const local=await (await request.get('/api/local-scenes/nacs')).json(),url='/api/semantic-volumes?scene=local:nacs';
 const initial={id:'',title:'Pivot test',description:'',center:[0,1,0],size:[2,2,1],yaw:.2};
 const created=await (await request.post(url,{data:{frame:local.frame,volume:initial}})).json(),v=created.items.find(v=>v.title==='Pivot test');
 try{
  await page.goto('/');await page.getByRole('button',{name:'Maps',exact:true}).click();await page.locator('[data-map="local:nacs"]').click();await expect(page.locator('#scene-title')).toHaveText('NACS');
  await page.getByRole('button',{name:'Volumes',exact:true}).click();await page.locator(`[data-volume-id="${v.id}"]`).click();
  async function drag(name,dx,dy){const handle=page.getByRole('button',{name,exact:true});await expect(handle).toBeVisible();const b=await handle.boundingBox();await page.mouse.move(b.x+b.width/2,b.y+b.height/2);await page.mouse.down();await page.mouse.move(b.x+b.width/2+dx,b.y+b.height/2+dy,{steps:10});await page.mouse.up();await expect(page.locator('#volume-status')).toHaveText('Volume saved in this scene.');}
  await drag('Move volume',50,25);
  const moved=(await (await request.get(url)).json()).items.find(i=>i.id===v.id);expect(moved.center[0]).not.toBe(0);expect(moved.center[1]).toBe(1);expect(moved.size).toEqual(initial.size);expect(moved.yaw).toBeCloseTo(initial.yaw,3);
  const point=(item,sign)=>[item.center[0]+Math.cos(item.yaw)*sign*item.size[0]/2+Math.sin(item.yaw)*item.size[2]/2,item.center[2]-Math.sin(item.yaw)*sign*item.size[0]/2+Math.cos(item.yaw)*item.size[2]/2];
  const pivot=point(moved,1);await drag('Rotate left corner',-50,-30);
  const rotated=(await (await request.get(url)).json()).items.find(i=>i.id===v.id),afterPivot=point(rotated,1);
  expect(rotated.yaw).not.toBeCloseTo(moved.yaw,3);expect(rotated.size).toEqual(moved.size);expect(afterPivot[0]).toBeCloseTo(pivot[0],3);expect(afterPivot[1]).toBeCloseTo(pivot[1],3);
  await page.screenshot({path:'test-results/peyote-volume-editor.png'});
  await page.getByRole('button',{name:'New volume',exact:true}).click();await expect(page.getByRole('button',{name:'New volume',exact:true})).toHaveAttribute('aria-pressed','true');await page.keyboard.press('Escape');await expect(page.locator('#volume-form')).toBeHidden();await expect(page.getByRole('button',{name:'New volume',exact:true})).toHaveAttribute('aria-pressed','false');
  await page.getByRole('button',{name:'New volume',exact:true}).click();const canvas=await page.locator('#canvas canvas').boundingBox();await page.keyboard.down('Shift');await page.mouse.move(canvas.x+canvas.width*.35,canvas.y+canvas.height*.4);await page.mouse.down();await page.mouse.move(canvas.x+canvas.width*.6,canvas.y+canvas.height*.55,{steps:10});await page.mouse.up();await page.keyboard.up('Shift');
  await expect(page.locator('#volume-status')).toContainText('Front edge ready');const degrees=Number(await page.locator('#volume-form').getByLabel('Rotation (degrees)',{exact:true}).inputValue());expect(degrees/90).toBeCloseTo(Math.round(degrees/90),3);await page.getByRole('button',{name:'Cancel',exact:true}).click();
  expect(errors).toEqual([]);
 }finally{await request.delete('/api/semantic-volumes/'+v.id+'?scene=local:nacs',{data:{frame:local.frame}});}
});

test('volume writes reject wrong frames, invalid geometry and untrusted origins',async({request})=>{
 const scene=await (await request.get('/api/local-scenes/nacs')).json();
 const volume={id:'',title:'Test',description:'',center:[0,1,0],size:[1,2,1],yaw:0};
 const url='/api/semantic-volumes?scene=local:nacs';
 expect((await request.post(url,{data:{frame:'wrong',volume}})).status()).toBe(422);
 expect((await request.post(url,{data:{frame:scene.frame,volume:{...volume,size:[0,2,1]}}})).status()).toBe(422);
 expect((await request.post(url,{headers:{Origin:'https://untrusted.example'},data:{frame:scene.frame,volume}})).status()).toBe(422);
 expect((await request.post('/api/semantic-volumes?scene=runner',{data:{frame:scene.frame,volume}})).status()).toBe(422);
 expect((await (await request.get(url)).json()).items).toEqual([]);
});

test('face handles resize rotated boxes directly in the scene and keep opposite faces fixed',async({page,request})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const local=await (await request.get('/api/local-scenes/humanoid-hub-conference')).json();
 const url='/api/semantic-volumes?scene=local:humanoid-hub-conference';
 const original={id:'',title:'Handle test',description:'Resizable region',center:[0,1,0],size:[2,2,2],yaw:Math.PI/4};
 const collection=await (await request.post(url,{data:{frame:local.frame,volume:original}})).json();
 const v=collection.items.find(v=>v.title==='Handle test');
 try{
  await page.goto('/');await page.getByRole('button',{name:'Maps',exact:true}).click();await page.locator('[data-map="local:humanoid-hub-conference"]').click();
  await expect(page.locator('#scene-title')).toHaveText('Humanoid Hub Conference');await page.getByRole('button',{name:'Volumes',exact:true}).click();await page.locator(`[data-volume-id="${v.id}"]`).click();
  const form=page.locator('#volume-form'),width=form.getByLabel('Width',{exact:true});
  const handle=page.getByRole('button',{name:'Resize +X face',exact:true});await expect(handle).toBeVisible();
  const rect=await handle.boundingBox();await page.mouse.move(rect.x+rect.width/2,rect.y+rect.height/2);await page.mouse.down();await page.mouse.move(rect.x+rect.width/2+70,rect.y+rect.height/2,{steps:10});await page.mouse.up();
  await expect(page.locator('#volume-status')).toHaveText('Volume saved in this scene.');
  expect(Number(await width.inputValue())).toBeGreaterThan(2);
  const newWidth=Number(await width.inputValue()),x=Number(await form.getByLabel('Center X',{exact:true}).inputValue()),z=Number(await form.getByLabel('Center Z',{exact:true}).inputValue());
  expect(x-Math.cos(v.yaw)*newWidth/2).toBeCloseTo(-Math.cos(v.yaw),3);
  expect(z+Math.sin(v.yaw)*newWidth/2).toBeCloseTo(Math.sin(v.yaw),3);
  const top=page.getByRole('button',{name:'Resize +Y face',exact:true}),r=await top.boundingBox();
  await expect(top).toBeInViewport();
  expect(await page.evaluate(({x,y})=>document.elementFromPoint(x,y)?.closest('button')?.getAttribute('aria-label'),{x:r.x+r.width/2,y:r.y+r.height/2})).toBe('Resize +Y face');
  await page.mouse.move(r.x+r.width/2,r.y+r.height/2);await page.mouse.down();await page.mouse.move(r.x+r.width/2,r.y+r.height/2-60,{steps:10});await page.mouse.up();
  await expect(page.locator('#volume-status')).toHaveText('Volume saved in this scene.');
  expect(Number(await form.getByLabel('Height',{exact:true}).inputValue())).toBeGreaterThan(2);
  expect(Number(await form.getByLabel('Base elevation',{exact:true}).inputValue())).toBeCloseTo(0,3);
  // The +X handle sits well below the card after both drags; it must stay usable, not be hidden by the card's column.
  await expect(handle).toBeVisible();const before=await width.inputValue(),r2=await handle.boundingBox();await page.mouse.move(r2.x+r2.width/2,r2.y+r2.height/2);await page.mouse.down();await page.mouse.move(r2.x+r2.width/2+40,r2.y+r2.height/2,{steps:5});await page.keyboard.press('Escape');await page.mouse.up();await expect(width).toHaveValue(before);
  await page.screenshot({path:'test-results/semantic-volume-handles.png'});
  await page.getByRole('button',{name:'Edit',exact:true}).click();await page.getByRole('button',{name:'Save volume',exact:true}).click();await expect(page.locator('#volume-status')).toHaveText('Volume saved in this scene.');
  const saved=(await (await request.get(url)).json()).items.find(item=>item.id===v.id);expect(saved.size[0]).toBeCloseTo(newWidth,3);expect(saved.size[1]).toBeGreaterThan(2);expect(saved.yaw).toBeCloseTo(v.yaw,3);
  await expect(page.locator('#volume-form')).toBeHidden();expect(errors).toEqual([]);
 }finally{await request.delete('/api/semantic-volumes/'+v.id+'?scene=local:humanoid-hub-conference',{data:{frame:local.frame}});}
});
