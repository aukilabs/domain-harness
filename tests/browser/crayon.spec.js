import {test,expect} from '@playwright/test';
test.beforeEach(async({request})=>{
 const scene='local:humanoid-hub-conference';
 const paths=await (await request.get('/api/paths?scene='+scene)).json();
 for(const path of paths.items)await request.delete('/api/paths/'+path.id+'?scene='+scene);
});
test('shares one growing stroke before release, without Done',async({page,request})=>{
 const url='/api/paths?scene=local:humanoid-hub-conference';
 await page.goto('/');await page.getByRole('button',{name:'Maps',exact:true}).click();
 await page.locator('[data-map="local:humanoid-hub-conference"]').click();await page.locator('#crayon').click();
 const rect=await page.locator('#canvas > canvas').boundingBox();
 await page.mouse.move(rect.x+rect.width*.3,rect.y+rect.height*.7);await page.mouse.down();
 await page.mouse.move(rect.x+rect.width*.4,rect.y+rect.height*.7,{steps:12});
 await expect.poll(async()=>(await (await request.get(url)).json()).items.length).toBe(1);
 const first=(await (await request.get(url)).json()).items[0];
 // Still holding the pointer: a second part extends the same persisted stroke.
 await page.mouse.move(rect.x+rect.width*.6,rect.y+rect.height*.7,{steps:18});
 await expect.poll(async()=>(await (await request.get(url)).json()).items[0].segments[0].length).toBeGreaterThan(first.segments[0].length);
 expect((await (await request.get(url)).json()).items.map(p=>p.id)).toEqual([first.id]);
 await page.mouse.up();await expect(page.locator('.crayon-panel [role=status]')).toHaveText('Path saved to Home.');
 await expect(page.locator('#crayon')).toHaveAttribute('aria-pressed','true');
 await page.getByRole('button',{name:'Undo last path'}).click();
 await expect.poll(async()=>(await (await request.get(url)).json()).items).toEqual([]);
});
test('draw floor path, persist across reload, isolate scenes, undo',async({page,request})=>{
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto('/');await page.getByRole('button',{name:'Maps',exact:true}).click();
 await page.locator('[data-map="local:humanoid-hub-conference"]').click();
 await expect(page.locator('#scene-title')).toHaveText('Humanoid Hub Conference');
 await page.locator('#crayon').click();await expect(page.locator('#crayon')).toHaveAttribute('aria-pressed','true');
 const canvas=page.locator('#canvas > canvas'),rect=await canvas.boundingBox();
 await page.mouse.move(rect.x+rect.width*.4,rect.y+rect.height*.7);await page.mouse.down();
 await page.mouse.move(rect.x+rect.width*.6,rect.y+rect.height*.7,{steps:20});await page.mouse.up();
 await expect(page.locator('.crayon-panel [role=status]')).toHaveText('Path saved to Home.');
 const url='/api/paths?scene=local:humanoid-hub-conference';const saved=await (await request.get(url)).json();
 expect(saved.items).toHaveLength(1);expect(saved.items[0].segments[0].length).toBeGreaterThan(1);
 expect(saved.items[0].segments.flat().every(p=>Math.abs(p[1])<1e-6)).toBeTruthy();
 await page.getByRole('button',{name:'Done',exact:true}).click();
 await page.mouse.click(rect.x+rect.width*.5,rect.y+rect.height*.7);
 await expect(page.locator('#selection')).toHaveAttribute('data-path-id',saved.items[0].id);
 await expect(page.locator('#selection').getByRole('button',{name:'Delete path'})).toBeVisible();
 await page.reload();await page.getByRole('button',{name:'Maps',exact:true}).click();await page.locator('[data-map="local:humanoid-hub-conference"]').click();
 expect((await (await request.get(url)).json()).items).toEqual(saved.items);
 expect((await (await request.get('/api/paths?scene=local:nacs')).json()).items).toEqual([]);
 await page.locator('#crayon').click();await page.getByRole('button',{name:'Undo last path'}).click();
 await expect(page.locator('.crayon-panel [role=status]')).toHaveText('Last path removed.');
 expect((await (await request.get(url)).json()).items).toEqual([]);
 expect(errors).toEqual([]);
});

test('delete a chosen path from Notes, preserving other paths across reload',async({page,request})=>{
 const scene='local:humanoid-hub-conference',url='/api/paths?scene='+scene;
 const frame=(await (await request.get(url)).json()).frame;
 const paths=['First stroke','Second stroke'].map((title,i)=>({id:crypto.randomUUID(),title,color:'#ffca55',width_m:.035,segments:[[[i,0,0],[i+1,0,1]]]}));
 for(const stroke of paths){const response=await request.post(url,{data:{frame,stroke}});expect(response.ok()).toBeTruthy();}
 await page.goto('/');await page.getByRole('button',{name:'Maps',exact:true}).click();
 await page.locator('[data-map="local:humanoid-hub-conference"]').click();
 await page.getByRole('button',{name:'Notes',exact:true}).click();
 await page.getByRole('button',{name:'First stroke · 1',exact:true}).click();
 await expect(page.locator('#selection')).toHaveAttribute('data-path-id',paths[0].id);
 // The sidebar has a separate deletion action from the viewer's selection card.
 await page.locator('#panel-notes').getByRole('button',{name:'Delete path',exact:true}).click();
 await expect.poll(async()=>(await (await request.get(url)).json()).items.map(p=>p.id)).toEqual([paths[1].id]);
 await expect(page.locator('#selection')).toHaveAttribute('data-path-id',paths[1].id);
 await page.reload();await page.getByRole('button',{name:'Notes',exact:true}).click();
 await expect(page.getByRole('button',{name:'First stroke · 1',exact:true})).toHaveCount(0);
 expect((await (await request.get(url)).json()).items.map(p=>p.id)).toEqual([paths[1].id]);
 await page.getByRole('button',{name:'Maps',exact:true}).click();
 await page.locator('[data-map="local:humanoid-hub-conference"]').click();
 await page.getByRole('button',{name:'Notes',exact:true}).click();
 await page.getByRole('button',{name:'Second stroke · 1',exact:true}).click();
 await page.locator('#selection').getByRole('button',{name:'Delete path',exact:true}).click();
 await expect.poll(async()=>(await (await request.get(url)).json()).items).toEqual([]);
});

test('Backspace advances through paths, protects typing, and keeps selection on errors',async({page,request})=>{
 const scene='local:humanoid-hub-conference',url='/api/paths?scene='+scene;
 const frame=(await (await request.get(url)).json()).frame;
 const paths=[0,1,2,3].map(i=>({id:crypto.randomUUID(),title:'Keyboard '+i,color:'#ffca55',width_m:.035,segments:[[[i,0,0],[i+1,0,1]]]}));
 for(const stroke of paths)expect((await request.post(url,{data:{frame,stroke}})).ok()).toBeTruthy();
 await page.goto('/');await page.getByRole('button',{name:'Maps',exact:true}).click();
 await page.locator('[data-map="local:humanoid-hub-conference"]').click();
 await page.getByRole('button',{name:'Notes',exact:true}).click();
 await page.getByRole('button',{name:'Keyboard 1 · 2',exact:true}).click();
 await page.locator('#agent-toggle').click();const input=page.locator('#scene-chat-input');await input.fill('abc');await input.press('Backspace');
 await expect(input).toHaveValue('ab');expect((await (await request.get(url)).json()).items).toHaveLength(4);
 await input.evaluate(el=>el.blur());
 await page.route('**/api/paths/*',async route=>{
  if(route.request().method()==='DELETE')await route.fulfill({status:503,contentType:'application/json',body:'{"error":"Try again"}'});
  else await route.continue();
 });
 await page.keyboard.press('Backspace');await expect(page.locator('#selection [role=status]')).toHaveText('Try again');
 await expect(page.locator('#selection')).toHaveAttribute('data-path-id',paths[1].id);
 await page.unroute('**/api/paths/*');
 for(let i=0;i<3;i++)await page.keyboard.press('Backspace');
 await expect.poll(async()=>(await (await request.get(url)).json()).items.map(p=>p.id)).toEqual([paths[0].id]);
 await expect(page.locator('#selection')).toHaveAttribute('data-path-id',paths[0].id);
 await expect(page.locator('#panel-notes h3')).toHaveText('Keyboard 0');
 await page.keyboard.press('Backspace');
 await expect.poll(async()=>(await (await request.get(url)).json()).items).toEqual([]);
 await expect(page.locator('#selection')).toBeHidden();
});
