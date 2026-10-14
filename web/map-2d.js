// Display coordinates are OpenGL (Y up), regardless of the source frame.
export function projectColumns(voxels, resolution, floor) {
 const columns=new Map();
 for(const voxel of voxels||[]){
  const [x,y,z]=voxel.center;
  if(![x,y,z].every(Number.isFinite)||y<=floor||y>floor+1)continue;
  const u=Math.floor(x/resolution),v=Math.floor(z/resolution);
  columns.set(`${u},${v}`,{center:[(u+.5)*resolution,floor,(v+.5)*resolution],value:100});
 }
 return [...columns.values()];
}

// Plan view palettes, read from the page theme each draw.
const PALETTES={
 light:{background:'#e8e8f0',gridMinor:'rgba(39,39,42,.06)',gridMajor:'rgba(39,39,42,.12)',occupied:'#3f3f46',free:'rgba(16,185,129,.12)',unknown:'rgba(39,39,42,.05)',
  volume:'rgba(124,58,237,.12)',volumeEdge:'#7c3aed',label:'#27272a',bubble:'#ffffff',bubbleText:'#27272a',note:'#d97706',empty:'#71717a'},
 dark:{background:'#0d0d0f',gridMinor:'rgba(255,255,255,.04)',gridMajor:'rgba(255,255,255,.08)',occupied:'#d4d4d8',free:'rgba(52,211,153,.12)',unknown:'rgba(255,255,255,.04)',
  volume:'rgba(167,139,250,.14)',volumeEdge:'#a78bfa',label:'#e4e4e7',bubble:'#27272a',bubbleText:'#fafafa',note:'#fbbf24',empty:'#a1a1aa'},
};
const NICE_LENGTHS=[.1,.2,.5,1,2,5,10,20,50,100];

// The 2D plan: occupancy (or voxel columns in a 1 m band above the floor) with volumes, paths and notes drawn
// on top, in the same visual language as the 3D view. Left-drag pans, the wheel zooms toward the cursor.
export function createMap2D(host,onToggle){
 const panel=document.createElement('div');panel.className='map2d';panel.hidden=true;
 const canvas=document.createElement('canvas');canvas.className='map2d-canvas';canvas.setAttribute('aria-label','2D occupancy map');canvas.setAttribute('role','img');panel.append(canvas);
 // Floor height, legend and scale share one strip; the app shows it in the bottom bar while the plan is open.
 const strip=document.createElement('div');strip.className='map2d-strip';
 strip.innerHTML=`<div class="map2d-floor" role="group" aria-label="Floor height">
  <span class="map2d-label">Floor</span>
  <button type="button" class="map2d-step" data-step="-0.1" aria-label="Lower the floor by 10 cm">−</button>
  <label><span class="visually-hidden">2D map floor height</span><input type="number" step="0.05" value="0" aria-label="2D map floor height"></label><span class="map2d-unit">m</span>
  <button type="button" class="map2d-step" data-step="0.1" aria-label="Raise the floor by 10 cm">+</button>
 </div>
 <div class="map2d-legend" aria-label="Legend"><span><i data-kind="occupied"></i>Occupied</span><span><i data-kind="free"></i>Free</span><span><i data-kind="volume"></i>Volume</span></div>
 <div class="map2d-scale" aria-hidden="true"><span class="map2d-scale-bar"></span><span class="map2d-scale-label"></span></div>`;
 panel.append(strip);host.append(panel);
 const floor=strip.querySelector('input'),scaleBar=strip.querySelector('.map2d-scale-bar'),scaleLabel=strip.querySelector('.map2d-scale-label');
 let data=null,scale=50,center=[0,0],drag=null,active=false,frame=0;
 const request=()=>{if(!frame)frame=requestAnimationFrame(()=>{frame=0;draw();});};
 floor.addEventListener('input',request);
 for(const step of strip.querySelectorAll('.map2d-step'))step.onclick=()=>{floor.value=(Number(floor.value)+Number(step.dataset.step)).toFixed(2);request();};
 const palette=()=>PALETTES[document.documentElement.dataset.theme==='light'?'light':'dark'];
 new MutationObserver(request).observe(document.documentElement,{attributes:true,attributeFilter:['data-theme']});
 function tiles(){return data?.occupancy_tiles||projectColumns(data?.voxels,data?.voxel_size||.1,Number(floor.value));}
 function draw(){
  if(!active)return;
  const {width,height}=host.getBoundingClientRect(),ratio=Math.min(devicePixelRatio||1,2);
  canvas.width=Math.round(width*ratio);canvas.height=Math.round(height*ratio);
  const ctx=canvas.getContext('2d'),colors=palette();ctx.setTransform(ratio,0,0,ratio,0,0);
  ctx.fillStyle=colors.background;ctx.fillRect(0,0,width,height);
  const point=p=>[width/2+(p[0]-center[0])*scale,height/2+(p[2]-center[1])*scale];
  // Grid: 1 m lines, with 5 m lines stronger; fine lines drop out when they would be too dense.
  for(const [step,color] of [[1,colors.gridMinor],[5,colors.gridMajor]]){
   if(step*scale<8)continue;ctx.strokeStyle=color;ctx.lineWidth=1;ctx.beginPath();
   const left=center[0]-width/2/scale,top=center[1]-height/2/scale;
   for(let x=Math.ceil(left/step)*step;x<left+width/scale;x+=step){const sx=Math.round(point([x,0,0])[0])+.5;ctx.moveTo(sx,0);ctx.lineTo(sx,height);}
   for(let z=Math.ceil(top/step)*step;z<top+height/scale;z+=step){const sy=Math.round(point([0,0,z])[1])+.5;ctx.moveTo(0,sy);ctx.lineTo(width,sy);}
   ctx.stroke();
  }
  const size=(data?.voxel_size||.1)*scale+.5,cells=tiles();
  for(const tile of cells){const [x,y]=point(tile.center);ctx.fillStyle=tile.value===100?colors.occupied:tile.value===0?colors.free:colors.unknown;ctx.fillRect(x-size/2,y-size/2,size,size);}
  // Volumes: rotated footprints with their titles.
  ctx.font='600 11px system-ui,-apple-system,sans-serif';ctx.textAlign='center';ctx.textBaseline='middle';
  for(const v of data?.semantic_volumes?.items||[]){
   const [x,y]=point(v.center);ctx.save();ctx.translate(x,y);ctx.rotate(-v.yaw);
   const w=v.size[0]*scale,d=v.size[2]*scale;ctx.fillStyle=colors.volume;ctx.strokeStyle=colors.volumeEdge;ctx.lineWidth=1.25;
   ctx.beginPath();ctx.roundRect(-w/2,-d/2,w,d,3);ctx.fill();ctx.stroke();ctx.restore();
   if(v.title&&Math.min(v.size[0],v.size[2])*scale>28){ctx.fillStyle=colors.volumeEdge;ctx.fillText(v.title,x,y);}
  }
  // Paths in their own colours.
  ctx.lineCap='round';ctx.lineJoin='round';
  for(const path of data?.paths?.items||[]){ctx.strokeStyle=path.color||'#2563eb';ctx.lineWidth=Math.max(2,(path.width_m||.06)*scale);
   for(const segment of path.segments||[]){ctx.beginPath();segment.forEach((p,i)=>{const [x,y]=point(p);if(i)ctx.lineTo(x,y);else ctx.moveTo(x,y);});ctx.stroke();}}
  // Notes as bubbles, like the 3D view: a dot on the spot and a rounded label above it.
  ctx.font='500 12px system-ui,-apple-system,sans-serif';ctx.textBaseline='middle';ctx.textAlign='left';
  for(const note of data?.notes||[]){
   if(!note.position)continue;const [x,y]=point(note.position),text=note.details?.title||note.text||'Note',tw=ctx.measureText(text).width,bw=tw+20,bh=24,bx=x-bw/2,by=y-bh-10;
   ctx.fillStyle=colors.note;ctx.beginPath();ctx.arc(x,y,4,0,Math.PI*2);ctx.fill();
   ctx.save();ctx.shadowColor='rgba(24,24,27,.22)';ctx.shadowBlur=10;ctx.shadowOffsetY=3;ctx.fillStyle=colors.bubble;ctx.beginPath();ctx.roundRect(bx,by,bw,bh,12);ctx.fill();ctx.restore();
   ctx.fillStyle=colors.bubbleText;ctx.fillText(text,bx+10,by+bh/2);
  }
  if(!cells.length){ctx.fillStyle=colors.empty;ctx.textAlign='center';ctx.font='500 13px system-ui,-apple-system,sans-serif';ctx.fillText('No occupancy observed in this height band. Try another floor height.',width/2,height/2);}
  // Scale bar: the longest round length that fits in about 120 px.
  const length=[...NICE_LENGTHS].reverse().find(m=>m*scale<=120)||NICE_LENGTHS[0];
  scaleBar.style.width=Math.round(length*scale)+'px';scaleLabel.textContent=length<1?`${Math.round(length*100)} cm`:`${length} m`;
 }
 function fit(){const points=tiles().map(t=>t.center);if(!points.length){center=[0,0];scale=50;request();return;}let minX=Infinity,maxX=-Infinity,minZ=Infinity,maxZ=-Infinity;for(const p of points){minX=Math.min(minX,p[0]);maxX=Math.max(maxX,p[0]);minZ=Math.min(minZ,p[2]);maxZ=Math.max(maxZ,p[2]);}center=[(minX+maxX)/2,(minZ+maxZ)/2];scale=Math.min(host.clientWidth/Math.max(2,maxX-minX+1),host.clientHeight/Math.max(2,maxZ-minZ+1))*.8;request();}
 canvas.onpointerdown=e=>{drag=[e.clientX,e.clientY];canvas.setPointerCapture(e.pointerId);canvas.classList.add('dragging');};
 canvas.onpointermove=e=>{if(!drag)return;center[0]-=(e.clientX-drag[0])/scale;center[1]-=(e.clientY-drag[1])/scale;drag=[e.clientX,e.clientY];request();};
 canvas.onpointerup=canvas.onpointercancel=()=>{drag=null;canvas.classList.remove('dragging');};
 // Zoom toward the cursor: the point under it stays put.
 canvas.addEventListener('wheel',e=>{e.preventDefault();const box=canvas.getBoundingClientRect(),mx=e.clientX-box.left-box.width/2,my=e.clientY-box.top-box.height/2;
  const before=[center[0]+mx/scale,center[1]+my/scale];scale=Math.max(5,Math.min(1000,scale*Math.exp(-e.deltaY*.001)));center=[before[0]-mx/scale,before[1]-my/scale];request();},{passive:false});
 new ResizeObserver(request).observe(host);
 // The plan cross-fades in and out over the 3D view.
 const reduce=()=>matchMedia('(prefers-reduced-motion: reduce)').matches;
 function show(on){
  if(on){panel.hidden=false;if(!reduce())panel.animate([{opacity:0,transform:'scale(1.02)'},{opacity:1,transform:'none'}],{duration:320,easing:'cubic-bezier(.2,.8,.2,1)'});return;}
  if(reduce()){panel.hidden=true;return;}
  panel.animate([{opacity:1},{opacity:0}],{duration:200,easing:'ease-in'}).finished.then(()=>{if(!active)panel.hidden=true;});
 }
 return {toggle(){active=!active;show(active);onToggle(active);if(active)fit();},update(value){const changed=data?.frame!==value.frame;data=value;if(changed&&active)fit();else request();},fit,isActive:()=>active,controlStrip:strip};
}
