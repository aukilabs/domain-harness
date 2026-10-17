import {test,expect} from '@playwright/test';
import {readFile} from 'node:fs/promises';

async function setup(page,{saved={},peers=[{peer_id:'phone-id',alias:"Dan's Phone"}],fail=false}={}){
 const calls=[],errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.route('**/*',async route=>{
  const path=new URL(route.request().url()).pathname;
  if(path==='/scene-wifi.js'||path==='/style.css')return route.fulfill({body:await readFile(new URL('../../web'+path,import.meta.url),'utf8'),contentType:path.endsWith('js')?'text/javascript':'text/css'});
  if(path==='/api/peer')return route.fulfill({json:{peers}});
  if(path.endsWith('/wifi')){
   if(route.request().method()==='POST'){calls.push({path,body:route.request().postDataJSON()});return route.fulfill({json:{}});}
   if(fail)return route.fulfill({status:503,body:'unavailable'});
   return route.fulfill({json:{ssid:null,has_password:false,hidden:false,allowed_peer_ids:[],...saved}});
  }
  return route.fulfill({contentType:'text/html',body:`<!doctype html><link rel="stylesheet" href="/style.css"><div id="settings"></div><script type="module">import{createSceneWifi}from'/scene-wifi.js';window.scene='local:test';window.ui=createSceneWifi({container:document.querySelector('#settings'),getScene:()=>window.scene,getName:()=>"My apartment"});</script>`});
 });
 await page.goto('http://wifi-fixture.test/');await page.getByRole('button',{name:'Scene Wi-Fi',exact:true}).click();
 return {calls,errors};
}
test('save a scene network with explicit host permissions, without storing it in the browser',async({page})=>{
 const {calls,errors}=await setup(page);await expect(page.getByRole('button',{name:'Save',exact:true})).toBeEnabled();
 await page.getByLabel('Network name (SSID)').fill('Demo network');await page.getByLabel('Password',{exact:true}).fill('fixture-password');
 await expect(page.getByRole('checkbox',{name:/Dan/})).not.toBeChecked();await page.getByRole('checkbox',{name:/Dan/}).check();
 await page.getByLabel('Hidden network').check();
 await page.screenshot({path:'test-results/scene-wifi-settings.png'});
 await page.getByRole('button',{name:'Save',exact:true}).click();await expect(page.getByRole('status')).toContainText('Saved.');
 expect(calls).toEqual([{path:'/api/local-scenes/test/wifi',body:{action:'save',ssid:'Demo network',password:{action:'set',value:'fixture-password'},hidden:true,allowed_peer_ids:['phone-id']}}]);
 await expect(page.getByLabel('Password',{exact:true})).toHaveValue('');
 expect(await page.evaluate(()=>[localStorage.length,sessionStorage.length])).toEqual([0,0]);expect(errors).toEqual([]);
});
test('retain a saved password, require a replacement for a new SSID, and support open networks',async({page})=>{
 const {calls}=await setup(page,{saved:{ssid:'Existing',has_password:true,allowed_peer_ids:['offline-host']},peers:[]});
 await expect(page.getByRole('button',{name:'Save',exact:true})).toBeEnabled();await expect(page.getByLabel('Password',{exact:true})).toHaveValue('');
 await expect(page.getByRole('checkbox',{name:/offline-host/})).toBeChecked();
 await page.getByLabel('Network name (SSID)').fill('Different');await page.getByRole('button',{name:'Save',exact:true}).click();
 await expect(page.getByRole('status')).toContainText('Enter a password');expect(calls).toHaveLength(0);
 await page.getByLabel('Network name (SSID)').fill('Existing');await page.getByRole('button',{name:'Save',exact:true}).click();
 await expect(page.getByRole('status')).toContainText('Saved.');expect(calls[0].body.password).toEqual({action:'keep'});
 await page.getByRole('button',{name:'Cancel',exact:true}).click();await page.getByRole('button',{name:'Scene Wi-Fi',exact:true}).click();
 await expect(page.getByRole('button',{name:'Save',exact:true})).toBeEnabled();await page.getByLabel('Security').selectOption('open');
 await expect(page.getByLabel('Password',{exact:true})).toBeHidden();await page.getByRole('button',{name:'Save',exact:true}).click();
 await expect(page.getByRole('status')).toContainText('Saved.');expect(calls[1].body.password).toEqual({action:'clear'});
});
test('catalog refresh does not dismiss editing; scene change clears the secret and closes',async({page})=>{
 const {calls}=await setup(page);await expect(page.getByRole('button',{name:'Save',exact:true})).toBeEnabled();
 await page.getByLabel('Password',{exact:true}).fill('fixture-password');await page.evaluate(()=>window.ui.sync());await expect(page.getByRole('dialog')).toBeVisible();
 expect(await page.evaluate(()=>{const input=document.querySelector('[name=password]');window.scene='local:other';window.ui.sync();return input.value;})).toBe('');
 await expect(page.getByRole('dialog')).toHaveCount(0);expect(calls).toHaveLength(0);
 await page.evaluate(()=>{window.scene='peer-scene:remote';window.ui.sync();});await expect(page.getByRole('button',{name:'Scene Wi-Fi',exact:true})).toBeDisabled();
});
test('failed settings read cannot overwrite configuration',async({page})=>{
 const {calls,errors}=await setup(page,{fail:true});await expect(page.getByRole('status')).toContainText('could not load');await expect(page.getByRole('button',{name:'Save',exact:true})).toBeDisabled();expect(calls).toHaveLength(0);expect(errors).toEqual([]);
});
test('removing settings requires confirmation and sends no credentials',async({page})=>{
 const {calls}=await setup(page,{saved:{ssid:'Existing',has_password:true,allowed_peer_ids:['phone-id']}});
 const remove=page.getByRole('button',{name:'Remove settings',exact:true});await expect(remove).toBeEnabled();
 page.once('dialog',d=>d.dismiss());await remove.click();expect(calls).toHaveLength(0);
 page.once('dialog',d=>d.accept());await remove.click();await expect(page.getByRole('status')).toContainText('Settings removed.');
 expect(calls).toEqual([{path:'/api/local-scenes/test/wifi',body:{action:'remove'}}]);
});
