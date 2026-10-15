// Favicon write-on. Browsers show SVG favicons as still images, so the pen animation the rail logo plays on hover
// is drawn here frame by frame on a small canvas, once when the page is first shown; then the static SVG icon
// (brand/mckenna-favicon.svg) comes back. mark and pen are the rail logo's path data.
const HUE=[[0,'#10b981'],[.2,'#3b82f6'],[.4,'#8b5cf6'],[.6,'#ec4899'],[.8,'#f59e0b'],[1,'#06b6d4']];
export function playFaviconWrite({mark,pen,duration=1600}){
 const link=document.querySelector('link[rel="icon"]');
 if(!link||matchMedia('(prefers-reduced-motion: reduce)').matches)return;
 const still=link.getAttribute('href'),size=64,canvas=document.createElement('canvas'),ink=document.createElement('canvas');canvas.width=canvas.height=ink.width=ink.height=size;
 const ctx=canvas.getContext('2d'),inkCtx=ink.getContext('2d'),markPath=new Path2D(mark),penPath=new Path2D(pen);
 // The pen route's length, for the dash that reveals it (measured on a throwaway SVG path).
 const svg=document.createElementNS('http://www.w3.org/2000/svg','svg'),probe=document.createElementNS('http://www.w3.org/2000/svg','path');
 svg.style.cssText='position:absolute;width:0;height:0';probe.setAttribute('d',pen);svg.append(probe);document.body.append(svg);const length=probe.getTotalLength();svg.remove();
 // Same layout as the SVG icon: the 231 × 118 mark as wide as the square allows (2/256 margin), centred.
 const scale=size*252/256/231,ox=(size-231*scale)/2,oy=(size-118*scale)/2,ease=t=>t<.5?4*t*t*t:1-Math.pow(-2*t+2,3)/2;
 let start=0,frames=0;
 function frame(now){
  start||=now;const t=Math.min(1,(now-start)/duration),reveal=length*ease(t);
  // Every other frame is plenty for a 16 px tab icon.
  if(t<1&&frames++%2){requestAnimationFrame(frame);return;}
  ctx.setTransform(1,0,0,1,0,0);ctx.clearRect(0,0,size,size);
  ctx.setTransform(scale,0,0,scale,ox,oy);ctx.fillStyle='rgba(161,161,170,.45)';ctx.fill(markPath);
  // The colour mark, shown only where the pen has passed.
  inkCtx.setTransform(1,0,0,1,0,0);inkCtx.globalCompositeOperation='source-over';inkCtx.clearRect(0,0,size,size);inkCtx.setTransform(scale,0,0,scale,ox,oy);
  inkCtx.lineWidth=34;inkCtx.lineCap='round';inkCtx.lineJoin='round';inkCtx.strokeStyle='#000';inkCtx.setLineDash([reveal,length]);inkCtx.stroke(penPath);
  inkCtx.globalCompositeOperation='source-in';const hue=inkCtx.createLinearGradient(0,0,231,118);for(const [offset,colour] of HUE)hue.addColorStop(offset,colour);inkCtx.fillStyle=hue;inkCtx.fill(markPath);
  ctx.setTransform(1,0,0,1,0,0);ctx.drawImage(ink,0,0);
  link.setAttribute('href',canvas.toDataURL('image/png'));
  if(t<1)requestAnimationFrame(frame);else link.setAttribute('href',still);
 }
 // Play when the tab is actually visible, so a page opened in the background still shows it.
 const go=()=>requestAnimationFrame(frame);
 if(document.hidden)document.addEventListener('visibilitychange',function once(){if(!document.hidden){document.removeEventListener('visibilitychange',once);go();}});else go();
}
