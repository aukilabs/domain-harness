import {test,expect} from '@playwright/test';
import {readFile} from 'node:fs/promises';

// Uses the existing Home only to serve the page. Chat replies are fixtures;
// nothing is saved, no camera is opened, and no second Home is launched.
test('Qwen explanation toggles on right click and survives conversation redraws',async({page})=>{
 const errors=[];page.on('pageerror',error=>errors.push(error.message));
 await page.route('**/api/scene-sources',r=>r.fulfill({json:{local:[{id:'demo',name:'Home Server'}],preferred:'demo',runners:[],scenes:[],phones:[]}}));
 await page.route('**/api/scene',r=>r.fulfill({json:{frame:'fixture-frame',voxels:[],voxel_size:.1,portals:[],notes:[],objects:[],sources:{},integrated_observations:0}}));
 await page.route('**/api/peer',r=>r.fulfill({json:{enabled:false,peers:[]}}));
 await page.route('**/api/sensors**',r=>r.fulfill({json:{sensors:[]}}));
 await page.route('**/style.css',async r=>r.fulfill({contentType:'text/css',body:await readFile(new URL('../../web/style.css',import.meta.url),'utf8')}));
 await page.route('**/*.js',async r=>{
  const path=new URL(r.request().url()).pathname;
  if(!/^\/[a-z0-9-]+\.js$/.test(path))return r.continue();
  return r.fulfill({contentType:'text/javascript',body:await readFile(new URL('../../web'+path,import.meta.url),'utf8')});
 });
 const requests=[];
 const conclusion='The office and lounge are about 10.07 m apart, center to center.';
 const explanation='Compared their centers in Home coordinates: the 3D distance is 10.0675 m.\n<img src=x onerror="window.injected=true">';
 await page.route('**/api/chat',r=>{
  requests.push(r.request().postDataJSON());
  return r.fulfill({json:requests.length===1?{reply:conclusion,explanation,model:'fixture',changes:[]}:{reply:'Yes.',model:'fixture',changes:[]}});
 });
 await page.goto('/');
 await page.waitForFunction(()=>document.querySelector('#scene-chat-form').onsubmit!==null);
 await page.locator('#agent-toggle').click();
 const submit=async question=>{
  await page.locator('#scene-chat-input').fill(question);
  await page.locator('#scene-chat-form button[type="submit"]').click();
 };
 await submit('How far is the office from the lounge?');
 const row=page.locator('.scene-chat-message.assistant').first();
 const answer=row.locator('.scene-chat-text'),details=row.locator('.scene-chat-explanation');
 await expect(answer).toHaveText(conclusion);await expect(answer).toHaveAttribute('aria-expanded','false');
 await expect(details).toBeHidden();await expect(row).not.toContainText('No scene changes were saved');
 await answer.click({button:'right'});await expect(details).toBeVisible();await expect(details).toHaveText(explanation);
 await expect(answer).toHaveAttribute('aria-expanded','true');
 await expect(details.locator('img')).toHaveCount(0);expect(await page.evaluate(()=>window.injected)).toBeUndefined();
 await details.click({button:'right'});await expect(details).toBeHidden();
 await answer.focus();await page.keyboard.press('Enter');await expect(details).toBeVisible();
 await page.keyboard.press('Space');await expect(details).toBeHidden();
 await answer.click({button:'right'});await submit('Are you sure?');
 await expect(page.locator('.scene-chat-message.assistant')).toHaveCount(2);
 await expect(details).toBeVisible(); // Expansion belongs to the message, not a transient DOM node.
 expect(requests).toHaveLength(2);
 const previous=requests[1].messages.find(m=>m.role==='assistant');
 expect(previous).toEqual({role:'assistant',content:conclusion});
 await expect(page.locator('.scene-chat-message.assistant').last().locator('[aria-expanded]')).toHaveCount(0);
 // No-explanation and user messages keep normal context-menu behavior.
 const contextPrevented=await page.locator('.scene-chat-message.user').first().evaluate(el=>{
  const e=new MouseEvent('contextmenu',{bubbles:true,cancelable:true});el.dispatchEvent(e);return e.defaultPrevented;
 });
 expect(contextPrevented).toBe(false);
 await answer.click({button:'right'});await expect(details).toBeHidden();
 expect(requests).toHaveLength(2); // Toggling never sends another model request.
 expect(errors).toEqual([]);
});
