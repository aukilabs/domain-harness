import {test,expect} from '@playwright/test';

test('PiP label stays upright and anchored after each display quarter turn',async({page})=>{
 await page.goto('/');
 const bounds=await page.evaluate(async()=>{
  const {createArOverlay}=await import('/ar.js');
  const canvas=document.createElement('canvas');document.body.append(canvas);
  const overlay=createArOverlay(canvas),result=[];
  const frame={width:640,height:480,sequence:1,ar:{status:'ready',frame:'home',notes:[{text:'Upright label',pixel:[320,240]}],path_segments:[]}};
  overlay.show(frame);
  for(const angle of [0,90,180,270]){
   overlay.setRotation(angle);
   const c=document.createElement('canvas');c.width=angle%180?480:640;c.height=angle%180?640:480;
   const ctx=c.getContext('2d');ctx.translate(c.width/2,c.height/2);ctx.rotate(angle*Math.PI/180);ctx.drawImage(canvas,-320,-240);
   const data=ctx.getImageData(0,0,c.width,c.height).data;let minX=c.width,minY=c.height,maxX=0,maxY=0;
   for(let y=0;y<c.height;y++)for(let x=0;x<c.width;x++)if(data[(y*c.width+x)*4+3]>100){minX=Math.min(minX,x);maxX=Math.max(maxX,x);minY=Math.min(minY,y);maxY=Math.max(maxY,y);}
   result.push({width:maxX-minX,height:maxY-minY,x:(maxX+minX)/2-c.width/2,y:(maxY+minY)/2-c.height/2});
  }
  overlay.hide();return result;
 });
 for(const b of bounds){expect(b.width).toBeGreaterThan(b.height*2);expect(Math.abs(b.x)).toBeLessThan(2);expect(Math.abs(b.y)).toBeLessThan(2);expect(Math.abs(b.width-bounds[0].width)).toBeLessThan(3);expect(Math.abs(b.height-bounds[0].height)).toBeLessThan(3);}
});

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
