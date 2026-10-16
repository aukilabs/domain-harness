import test from 'node:test';
import assert from 'node:assert/strict';
import {bubbleLayout,bubbleColors} from '../web/note-bubble.js';
test('peer bubbles invert the scene note palette',()=>{
 const scene=bubbleColors(),peer=bubbleColors(true);
 assert.equal(peer.background,scene.foreground);
 assert.equal(peer.foreground,scene.background);
});
test('bubbles grow with content instead of a fixed width',()=>{
 const measure=text=>text.length*16;
 assert.ok(bubbleLayout('Dan',measure).width<bubbleLayout('Wash your hands',measure).width);
 assert.equal(bubbleLayout('Dan',measure).height,bubbleLayout('Wash your hands',measure).height);
 const wrapped=bubbleLayout('long note text that should wrap across lines',measure,160);assert.ok(wrapped.lines.length>1);
});
test('titles show a ghost through the scene and full strength only where unobstructed',async()=>{
 const THREE=await import('three');const {ghostWhenHidden,GHOST_OPACITY}=await import('../web/note-bubble.js');
 const plain=new THREE.Sprite(new THREE.SpriteMaterial());plain.renderOrder=80;const sprite=ghostWhenHidden(plain);
 const [twin]=sprite.children;
 assert.equal(sprite.material.depthTest,false);assert.equal(sprite.material.opacity,GHOST_OPACITY);
 assert.equal(twin.material.depthTest,true);assert.equal(twin.material.opacity,1);
 assert.equal(twin.material.depthWrite,false);assert.equal(twin.renderOrder,81);assert.ok(twin.userData.ghostTwin);
 let disposed=false;twin.material.addEventListener('dispose',()=>disposed=true);sprite.material.dispose();assert.ok(disposed);
});
