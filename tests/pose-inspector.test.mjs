import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {chromium} from '@playwright/test';

test('pose inspector shows native data, pauses display and stops polling on close',async()=>{
 const browser=await chromium.launch({channel:'chrome',headless:true});
 try {
  const page=await browser.newPage();
  await page.goto('about:blank');
  const source=await readFile(new URL('../web/pose-inspector.js',import.meta.url),'utf8');
  await page.evaluate(async source=>{
   window.calls=0;window.lost=false;
   window.fetch=async()=>{window.calls++;return {ok:true,json:async()=>({
    stream:'live',age_ms:12,received_hz:30,sequence:String(window.calls),timestamp_ns:'1791917200123456789',
    history:[{sequence:'9007199254740993',timestamp_ns:'1791917200123456001',observation:{payload:{tracking:{status:'lost'}}}}],
    resolved:{contract:{from_frame:{frame_id:'base'},to_frame:{frame_id:'robot-world',units:'meters',axes:{x:'right',y:'forward',z:'up'},handedness:'right'},session_id:'session',clock:{id:'capture'}},clock_definition:{clock_id:'capture'}},
    observation:{payload:{
     tracking:window.lost?{status:'lost'}:{status:'valid',pose:{translation:[1,2,3],rotation_xyzw:[0,0,0,1]}}}}
   })};};
   const {createPoseInspector}=await import(URL.createObjectURL(new Blob([source],{type:'text/javascript'})));
   createPoseInspector(document.body)('peer',{manifest:{product_id:'pose'},manifest_hash:'hash'},'Robot');
  },source);
  await page.waitForFunction(()=>document.body.textContent.includes('valid · live'));
  assert.match(await page.locator('body').innerText(),/1.0000, 2.0000, 3.0000 meters/);
  await page.getByText('Raw observation',{exact:true}).click();
  assert.match(await page.locator('body').innerText(),/1791917200123456789/);
  await page.getByLabel('Pose log',{exact:true}).selectOption('0');
  assert.match(await page.locator('body').innerText(),/lost · historical/);
  assert.match(await page.locator('body').innerText(),/Sequence: 9007199254740993/);
  assert.equal(JSON.parse(await page.locator('details pre').textContent()).timestamp_ns,'1791917200123456001');
  await page.getByRole('button',{name:'Resume display'}).click();
  await page.waitForFunction(()=>document.body.textContent.includes('valid · live'));
  await page.getByRole('button',{name:'Pause display'}).click();
  const before=await page.evaluate(()=>window.calls);
  await page.evaluate(()=>window.lost=true);
  await page.waitForFunction(n=>window.calls>n,before);
  assert.match(await page.locator('body').innerText(),/valid · live/);
  await page.getByRole('button',{name:'Resume display'}).click();
  await page.waitForFunction(()=>document.body.textContent.includes('lost · live'));
  await page.evaluate(()=>{
   const previous=window.fetch;
   window.fetch=async()=>{
    const response=await previous(),data=await response.json();
    data.observation.payload.contract=data.resolved.contract;
    delete data.resolved;
    return {ok:true,json:async()=>data};
   };
  });
  await page.waitForFunction(()=>document.body.textContent.includes('Unsupported pose contract'));
  const raw=await page.locator('details pre').textContent();
  assert.equal(JSON.parse(raw).payload.contract,undefined);
  await page.getByRole('button',{name:'Close',exact:true}).click();
  const closed=await page.evaluate(()=>window.calls);
  await page.waitForTimeout(600);
  assert.equal(await page.evaluate(()=>window.calls),closed);
 } finally {await browser.close();}
});
