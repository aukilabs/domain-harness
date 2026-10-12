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

export function createMap2D(host,onToggle){
 const panel=document.createElement('div');panel.hidden=true;
 panel.style.cssText='position:absolute;inset:0;z-index:5;background:#101820';
 const canvas=document.createElement('canvas');canvas.setAttribute('aria-label','2D occupancy map');canvas.style.cssText='width:100%;height:100%;touch-action:none';panel.append(canvas);
 const settings=document.createElement('div');settings.style.cssText='position:absolute;bottom:12px;left:12px;background:#192630;padding:10px;color:#eaf0f4;font:12px system-ui';
 settings.innerHTML='<label>Floor Y (m) <input type="number" step="0.05" value="0" aria-label="2D map floor height" style="width:65px"></label><div>Coral: occupied · dark: unknown · band: floor to +1 m</div><div>Drag to pan · scroll to zoom</div>';
 panel.append(settings);host.append(panel);
 let data=null,scale=50,center=[0,0],drag=null,active=false;
 const floor=settings.querySelector('input');floor.oninput=draw;
 function tiles(){return data?.occupancy_tiles||projectColumns(data?.voxels,data?.voxel_size||.1,Number(floor.value));}
 function draw(){
  if(!active)return;
  const {width,height}=host.getBoundingClientRect();canvas.width=Math.round(width);canvas.height=Math.round(height);
  const ctx=canvas.getContext('2d');ctx.fillStyle='#101820';ctx.fillRect(0,0,width,height);
  const point=p=>[width/2+(p[0]-center[0])*scale,height/2+(p[2]-center[1])*scale];
  const size=(data?.voxel_size||.1)*scale;
  for(const tile of tiles()){const [x,y]=point(tile.center);ctx.fillStyle=tile.value===100?'#ee795e':tile.value===0?'#82b9a0':'#343c49';ctx.fillRect(x-size/2,y-size/2,size,size);}
  ctx.font='12px system-ui';
  for(const note of data?.notes||[]){if(!note.position)continue;const [x,y]=point(note.position);ctx.fillStyle='#fff';ctx.beginPath();ctx.arc(x,y,4,0,Math.PI*2);ctx.fill();ctx.fillText(note.text||'Note',x+7,y-7);}
  if(!tiles().length){ctx.fillStyle='#bbc8d2';ctx.fillText('No observed occupancy in this height band. Check the floor height.',20,70);}
 }
 function fit(){const points=tiles().map(t=>t.center);if(!points.length){center=[0,0];scale=50;draw();return;}let minX=Infinity,maxX=-Infinity,minZ=Infinity,maxZ=-Infinity;for(const p of points){minX=Math.min(minX,p[0]);maxX=Math.max(maxX,p[0]);minZ=Math.min(minZ,p[2]);maxZ=Math.max(maxZ,p[2]);}center=[(minX+maxX)/2,(minZ+maxZ)/2];scale=Math.min(host.clientWidth/Math.max(2,maxX-minX+1),host.clientHeight/Math.max(2,maxZ-minZ+1))*.8;draw();}
 canvas.onpointerdown=e=>{drag=[e.clientX,e.clientY];canvas.setPointerCapture(e.pointerId);};canvas.onpointermove=e=>{if(!drag)return;center[0]-=(e.clientX-drag[0])/scale;center[1]-=(e.clientY-drag[1])/scale;drag=[e.clientX,e.clientY];draw();};canvas.onpointerup=canvas.onpointercancel=()=>drag=null;
 canvas.addEventListener('wheel',e=>{e.preventDefault();scale=Math.max(5,Math.min(1000,scale*Math.exp(-e.deltaY*.001)));draw();},{passive:false});
 new ResizeObserver(draw).observe(host);
 return {toggle(){active=!active;panel.hidden=!active;onToggle(active);if(active)fit();},update(value){const changed=data?.frame!==value.frame;data=value;if(changed&&active)fit();else draw();},fit,isActive:()=>active};
}
