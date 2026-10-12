import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {chromium} from '@playwright/test';

test('volume picking defers to visible notes but retains volume-only and hidden-note selection', async()=>{
 const browser=await chromium.launch({channel:'chrome',headless:true});
 try {
  const page=await browser.newPage({viewport:{width:900,height:700}});
  await page.route('http://picking.test/**',async route=>{
   const path=new URL(route.request().url()).pathname;
   if(path==='/')return route.fulfill({contentType:'text/html',body:`
    <style>body{margin:0}canvas{width:600px;height:600px}</style>
    <script type="importmap">{"imports":{"three":"/three.module.js"}}</script>
    <div><canvas width="600" height="600"></canvas></div><div id="panel"></div>
    <script type="module">
    import * as THREE from 'three';
    import {createSemanticVolumes} from '/semantic-volumes.js';
    const scene=new THREE.Scene(),camera=new THREE.PerspectiveCamera(60,1,.1,100);
    const canvas=document.querySelector('canvas'),notes=new THREE.Group();scene.add(notes);
    const note=new THREE.Sprite(new THREE.SpriteMaterial());note.scale.set(.4,.4,1);notes.add(note);
    const controls={target:new THREE.Vector3(),enabled:true,update(){camera.lookAt(this.target);camera.updateMatrixWorld();}};
    window.reset=()=>{window.scrollTo(0,0);camera.position.set(0,0,8);controls.target.set(0,0,0);controls.update();scene.updateMatrixWorld(true);window.picked=null;};
    window.notes=notes;
    const volumes=createSemanticVolumes({scene,camera,renderer:{domElement:canvas},controls,panel:document.querySelector('#panel'),
     api:()=>{throw Error('Unexpected write')},label:()=>new THREE.Object3D(),clear:g=>g.clear(),openPanel:()=>{},priorityPickObjects:()=>[note]});
    volumes.update('local:test',{frame:'test',semantic_volumes:{frame:'test',items:[{id:'box',title:'Box',description:'',center:[0,0,0],size:[4,4,4],yaw:0}]}});
    canvas.addEventListener('pointerup',e=>window.picked=e.semanticVolume?'volume':'normal');
    reset();window.ready=true;
    </script>`});
   const source=path==='/semantic-volumes.js'
    ?new URL('../web/semantic-volumes.js',import.meta.url)
    :new URL('../node_modules/three/build'+path,import.meta.url);
   await route.fulfill({contentType:'text/javascript',body:await readFile(source,'utf8')});
  });
  await page.goto('http://picking.test/');await page.waitForFunction(()=>window.ready);
  await page.mouse.click(300,300);
  assert.equal(await page.evaluate(()=>window.picked),'normal','Note inside box must reach normal picker');
  await page.evaluate(()=>{reset();notes.visible=false;});
  await page.mouse.click(300,300);
  assert.equal(await page.evaluate(()=>window.picked),'volume','Hidden ancestor must not steal volume clicks');
  await page.locator('#volume-cancel').click();
  await page.evaluate(()=>{reset();notes.visible=true;});
  await page.mouse.click(420,300);
  assert.equal(await page.evaluate(()=>window.picked),'volume','Volume remains selectable away from note');
  await page.locator('#volume-cancel').click();
  await page.evaluate(()=>reset());
  await page.mouse.move(420,300);await page.mouse.down();await page.mouse.move(440,300);await page.mouse.up();
  assert.equal(await page.evaluate(()=>window.picked),'normal','Orbit drag must not select a volume');
 } finally {await browser.close();}
});
