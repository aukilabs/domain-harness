import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {chromium} from '@playwright/test';

// A handle drag that starts while an earlier change is still saving must carry on, and its change must be the one
// saved next: the queued save may not cut it off half-way (it used to stop the drag and save its midpoint).
test('a drag during a save survives the queued save and is saved when it ends', async()=>{
 const browser=await chromium.launch({channel:'chrome',headless:true});
 try {
  const page=await browser.newPage({viewport:{width:900,height:700}});
  await page.route('http://queue.test/**',async route=>{
   const path=new URL(route.request().url()).pathname;
   if(path==='/')return route.fulfill({contentType:'text/html',body:`
    <style>body{margin:0}canvas{width:600px;height:600px}.volume-handles{position:absolute;inset:0;pointer-events:none}.volume-resize-handle{position:absolute;left:0;top:0;width:44px;height:44px;pointer-events:auto}</style>
    <script type="importmap">{"imports":{"three":"/three.module.js"}}</script>
    <div style="position:relative"><canvas width="600" height="600"></canvas></div><div id="panel"></div><div id="card"></div>
    <script type="module">
    import * as THREE from 'three';
    import {createSemanticVolumes} from '/semantic-volumes.js';
    const scene=new THREE.Scene(),camera=new THREE.PerspectiveCamera(60,1,.1,100),canvas=document.querySelector('canvas');
    camera.position.set(0,6,9);const controls={target:new THREE.Vector3(),enabled:true,update(){camera.lookAt(this.target);camera.updateMatrixWorld();}};controls.update();
    // Saves stay in flight until the test releases them, one by one.
    window.saves=[];const waiting=[];window.release=()=>waiting.shift()?.();
    let stored={id:'box',title:'Box',description:'',center:[0,1,0],size:[2,2,2],yaw:0};
    const api=(url,body)=>{window.saves.push(body.volume.size[0]);return new Promise(resolve=>waiting.push(()=>{stored={...body.volume,id:'box'};resolve({items:[stored]});}));};
    const volumes=createSemanticVolumes({scene,camera,renderer:{domElement:canvas},controls,panel:document.querySelector('#panel'),api,label:()=>new THREE.Object3D(),clear:g=>g.clear(),openPanel:()=>{},
     inspector:{element:document.querySelector('#card'),open(nodes){this.element.replaceChildren(...nodes);},close(){this.element.replaceChildren();}}});
    volumes.update('local:test',{frame:'test',semantic_volumes:{frame:'test',items:[stored]}});
    scene.updateMatrixWorld(true);window.volumes=volumes;window.ready=true;
    </script>`});
   const source=['/semantic-volumes.js','/panel-ui.js'].includes(path)?new URL('../web'+path,import.meta.url):new URL('../node_modules/three/build'+path,import.meta.url);
   await route.fulfill({contentType:'text/javascript',body:await readFile(source,'utf8')});
  });
  await page.goto('http://queue.test/');await page.waitForFunction(()=>window.ready);
  // Select the box by clicking it, then lay out its handles.
  await page.mouse.click(300,300);await page.evaluate(()=>window.volumes.tick());
  const handle=async()=>{await page.evaluate(()=>window.volumes.tick());const b=await page.getByRole('button',{name:'Resize +X face',exact:true}).boundingBox();return [b.x+b.width/2,b.y+b.height/2];};
  const drag=async(dx,{release=true}={})=>{const [x,y]=await handle();await page.mouse.move(x,y);await page.mouse.down();await page.mouse.move(x+dx,y,{steps:4});if(release)await page.mouse.up();};
  await drag(40);
  assert.equal((await page.evaluate(()=>window.saves)).length,1,'First drag saves');
  await drag(40);
  assert.equal((await page.evaluate(()=>window.saves)).length,1,'Second drag queues behind the first save');
  await drag(40,{release:false});
  // The first save lands while the third drag is still held: the queued save must wait for it.
  await page.evaluate(()=>window.release());await page.waitForTimeout(100);
  assert.equal((await page.evaluate(()=>window.saves)).length,1,'Queued save waits for the drag in progress');
  await page.mouse.move(...(await (async()=>{const [x,y]=await handle();return [x+30,y];})()),{steps:3});await page.mouse.up();
  const saves=await page.evaluate(()=>window.saves);
  assert.equal(saves.length,2,'The third drag saves when it ends');
  assert.ok(saves[1]>saves[0],'The second save carries the later drags');
 } finally {await browser.close();}
});
