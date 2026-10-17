import {test,expect} from '@playwright/test';
import {readFileSync} from 'node:fs';

test('robot keyboard drive uses only arrows and stops on release, blur and Escape',async({page})=>{
 const errors=[],commands=[],unexpectedWrites=[];
 page.on('pageerror',error=>errors.push(error.stack||error.message));
 const frame='local-scene:keyboard-test',robot='keyboard-test-robot';
 const scene={frame,environment_name:'Keyboard test',convention:'OpenGL · Y up · meters',voxel_size:.1,
  portals:[],objects:[],notes:[],voxels:[],sources:{},integrated_observations:0,demo:false};
 const peer={peer_id:robot,name:'Test robot',status:'connected',catalog_status:'ready',catalog:{components:[],products:[{
  manifest:{product_id:'bbos-pose-history'},manifest_hash:'pose-hash',metadata:{schema:'spatial-harness.compact-pose-contract/v1',value:{lifetime:'sampled',from_frame:{id:'bbos-base'}}}
 }]}};
 // Intercept every API request, including every write: this test never controls hardware.
 await page.route('**/api/**',route=>{
  const path=new URL(route.request().url()).pathname;
  if(route.request().method()!=='GET'){
   if(path==='/api/local-scenes/keyboard-test/drive'){
    commands.push(route.request().postDataJSON());return route.fulfill({json:{accepted:true}});
   }
   unexpectedWrites.push(path);return route.fulfill({status:400,json:{error:'Unexpected test write'}});
  }
  const replies={
   '/api/scene':scene,'/api/local-scenes/keyboard-test':scene,
   '/api/scene-sources':{local:[{id:'local:keyboard-test',name:'Keyboard test'}],runners:[]},
   '/api/peer':{enabled:true,status:'Ready',connection:{peer_id:'test-home'},peers:[peer]},
   '/api/sensors':{sensors:[],stream:'unavailable'},'/api/robot-states':{robots:[]},
   '/api/fiducials':{fiducials:[],families:[]},'/api/greet-peer':{greetings:[]},'/api/runner-pose':{status:'unavailable',pose:null}
  };
  return route.fulfill({json:replies[path]||{}});
 });
 await page.route('**/app.js',route=>route.fulfill({contentType:'text/javascript',body:readFileSync(new URL('../../web/app.js',import.meta.url),'utf8')}));
 await page.goto('/');
 await page.selectOption('#map-select','local:keyboard-test',{force:true});
 await page.getByRole('button',{name:'Devices',exact:true}).click();
 await page.locator(`.device-item[data-peer="${robot}"] .list-row`).click();
 const detail=page.locator('#device-detail');
 await expect(detail.getByRole('region',{name:'Drive',exact:true})).toContainText('Arrow keys drive the robot');
 await detail.getByRole('button',{name:'Drive with keyboard',exact:true}).click();
 await expect(detail.locator('[data-drive-status]')).toContainText('hold arrow keys');
 for(const key of ['w','a','s','d']){
  await page.keyboard.down(key);await page.waitForTimeout(250);await page.keyboard.up(key);
 }
 expect(commands).toEqual([]);
 for(const [key,forward,turn] of [['ArrowUp',1,0],['ArrowDown',-1,0],['ArrowLeft',0,1],['ArrowRight',0,-1]]){
  const before=commands.length;
  await page.keyboard.down(key);
  await expect.poll(()=>commands.slice(before).some(c=>c.forward===forward&&c.turn===turn)).toBe(true);
  await page.keyboard.up(key);
  await expect.poll(()=>commands.at(-1)).toMatchObject({forward:0,turn:0,target_peer_id:robot});
 }
 // Editing text must not produce robot movement.
 await detail.getByRole('button',{name:'Rename Test robot',exact:true}).click();
 const input=detail.locator('input');await input.focus();
 const editing=commands.length;
 await page.keyboard.down('ArrowUp');await page.waitForTimeout(250);await page.keyboard.up('ArrowUp');
 expect(commands.slice(editing).every(c=>c.forward===0&&c.turn===0)).toBe(true);
 await detail.getByRole('button',{name:'Cancel',exact:true}).click();
 await page.evaluate(()=>document.activeElement?.blur());
 await page.keyboard.down('ArrowUp');
 await expect.poll(()=>commands.at(-1)?.forward).toBe(1);
 await page.evaluate(()=>window.dispatchEvent(new Event('blur')));
 await expect.poll(()=>commands.at(-1)?.forward).toBe(0);
 await page.keyboard.up('ArrowUp');
 await page.keyboard.down('ArrowLeft');
 await expect.poll(()=>commands.at(-1)?.turn).toBe(1);
 await page.keyboard.press('Escape');await page.keyboard.up('ArrowLeft');
 await expect(detail.getByRole('button',{name:'Drive with keyboard',exact:true})).toBeVisible();
 expect(commands.at(-1)).toMatchObject({forward:0,turn:0,target_peer_id:robot});
 const stopped=commands.length;
 await page.keyboard.down('ArrowUp');await page.waitForTimeout(250);await page.keyboard.up('ArrowUp');
 expect(commands).toHaveLength(stopped);
 expect(commands.every((c,i)=>c.target_peer_id===robot&&c.sequence===i+1)).toBe(true);
 expect(unexpectedWrites).toEqual([]);expect(errors).toEqual([]);
});
