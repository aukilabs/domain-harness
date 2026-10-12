import test from 'node:test';
import assert from 'node:assert/strict';
import {northTurn} from '../web/compass.js';
const near=(a,b)=>assert.ok(Math.abs(a-b)<1e-9,`${a} is not ${b}`);
test('the marker sits at the top when the screen looks north',()=>{
 near(northTurn([0,-1],[0,-1],[1,0]),0);
});
test('north to the right of the screen turns the marker clockwise',()=>{
 near(northTurn([1,0],[0,-1],[1,0]),90);
 near(northTurn([-1,0],[0,-1],[1,0]),-90);
 near(Math.abs(northTurn([0,1],[0,-1],[1,0])),180);
});
test('orbiting the camera turns the marker the other way',()=>{
 // The screen now looks east (+X), with south (+Z) on its right: north is to the left.
 near(northTurn([0,-1],[1,0],[0,1]),-90);
});
