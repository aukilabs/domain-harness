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
