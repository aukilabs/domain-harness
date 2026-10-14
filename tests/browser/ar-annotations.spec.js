import {test,expect} from '@playwright/test';

test('PiP renders outlined Crayon ribbons and notes, never voxels',async({page})=>{
 await page.goto('/');
 const result=await page.evaluate(async()=>{
  const {createArOverlay,pathRibbonPositions}=await import('/ar.js');
  const canvas=document.createElement('canvas');canvas.style.cssText='width:400px;height:300px';document.body.append(canvas);
  const overlay=createArOverlay(canvas);
  const frame={width:400,height:300,sequence:1,ar:{status:'ready',frame:'home',projection:'sdk-camera',voxel_count:1,vertices:[[0,0,1,1,0,0],[400,0,1,1,0,0],[0,300,1,1,0,0]],notes:[],path_segments:[]}};
  const pixels=()=>{const c=document.createElement('canvas');c.width=400;c.height=300;const ctx=c.getContext('2d');ctx.drawImage(canvas,0,0);return ctx.getImageData(0,0,400,300).data;};
  overlay.show(frame);const blank=pixels().every(n=>n===0);
  frame.ar.path_segments=[{color:'#ffff00',points:[[50,150,2],[350,150,2]]}];
  frame.ar.notes=[{text:'Hello',pixel:[200,60]}];overlay.show(frame);
  const data=pixels(),pixel=(x,y)=>Array.from(data.slice((y*400+x)*4,(y*400+x)*4+4));
  const shown={...canvas.dataset},center=pixel(200,150),outline=pixel(200,153);
  const ribbon=pathRibbonPositions([[0,0,1],[100,0,1]],400,300,5);
  overlay.hide();return {blank,shown,center,outline,ribbon,hidden:canvas.hidden};
 });
 expect(result.blank).toBe(true);
 expect(result.shown).toMatchObject({voxelCount:'0',noteCount:'1',pathCount:'1'});
 expect(result.center.slice(0,3)).toEqual([255,255,0]);
 expect(result.outline[3]).toBe(255);expect(result.outline[0]).toBeLessThan(80);
 expect(result.ribbon).toHaveLength(18);expect(result.hidden).toBe(true);
});
