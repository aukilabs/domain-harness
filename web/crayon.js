import * as THREE from 'three';
const newId=()=>crypto.randomUUID?.()||'10000000-1000-4000-8000-100000000000'.replace(/[018]/g,c=>(Number(c)^crypto.getRandomValues(new Uint8Array(1))[0]&15>>Number(c)/4).toString(16));

export function pathMeshes(paths){
 const group=new THREE.Group();
 for(const stroke of paths?.items||[]){
  const edges=stroke.segments.flatMap(s=>s.slice(1).map((b,i)=>[s[i],b])).filter(([a,b])=>Math.hypot(...a.map((v,i)=>v-b[i]))>1e-6);
  const mesh=new THREE.InstancedMesh(new THREE.CylinderGeometry(stroke.width_m/2,stroke.width_m/2,1,8),new THREE.MeshBasicMaterial({color:stroke.color}),edges.length);
  mesh.userData={kind:'path',path:stroke};
  const matrix=new THREE.Matrix4(),up=new THREE.Vector3(0,1,0);
  edges.forEach(([a,b],i)=>{const start=new THREE.Vector3(...a),end=new THREE.Vector3(...b),delta=end.clone().sub(start),length=delta.length();matrix.compose(start.add(end).multiplyScalar(.5),new THREE.Quaternion().setFromUnitVectors(up,delta.normalize()),new THREE.Vector3(1,length,1));mesh.setMatrixAt(i,matrix);});group.add(mesh);
 }
 return group;
}
function clear(group){for(const node of [...group.children]){node.geometry?.dispose();node.material?.dispose();node.dispose?.();group.remove(node);}}
export function createCrayon({button,canvas,camera,controls,scene,voxels,state,save,remove}){
 const saved=new THREE.Group(),draft=new THREE.Group();scene.add(saved,draft);
 const panel=document.createElement('div');panel.className='crayon-panel';panel.hidden=true;
 panel.innerHTML='<span role="status">Draw to share live. Release to finish a stroke.</span><button class="retry" hidden>Retry save</button><button class="undo">Undo last path</button><button class="done">Done</button>';canvas.parentElement.append(panel);
 const message=panel.querySelector('span'),retry=panel.querySelector('.retry'),undo=panel.querySelector('.undo');
 let enabled=false,dragging=false,busy=false,segments=[],segment=null,context=null,signature='',lastSample=0,prior=true,paths=null,selectedId=null;
 let strokeId=null,created=false,pending=null,failed=null,timer=null;
 const sameScene=target=>state().map===target.map&&state().frame===target.frame;
 function render(target,value){clear(target);for(const child of [...pathMeshes(value).children])target.add(child);}
 function highlight(){for(const mesh of saved.children){
  mesh.material.color.set(mesh.userData.path.id===selectedId?'#ffffff':mesh.userData.path.color);
  mesh.visible=!(mesh.userData.path.id===strokeId&&(dragging||pending||busy||failed));
 }}
 function mode(on){if(busy||pending||failed||dragging)return;enabled=on;button.setAttribute('aria-pressed',String(on));panel.hidden=!on;canvas.style.cursor=on?'crosshair':'';if(on){prior=controls.enabled;controls.enabled=false;}else{controls.enabled=prior;clear(draft);}}
 button.onclick=()=>{if(state().editable)mode(!enabled);};panel.querySelector('.done').onclick=()=>mode(false);
 function queue(){
  const valid=segments.filter(s=>s.length>=2);if(!valid.length)return;
  pending={context,stroke:{id:strokeId,title:'Crayon path',color:'#ffca55',width_m:.035,segments:valid.map(s=>s.map(p=>[...p]))}};
  if(!timer&&!failed)timer=setTimeout(()=>{timer=null;persist();},200);
 }
 async function persist(){
  if(busy||failed||!pending)return;
  const request=pending;pending=null;busy=true;retry.hidden=true;
  message.textContent='Sharing live…';
  try{
   const result=await save(request.context,request.stroke,created);created=true;
   if(sameScene(request.context)){paths=result;render(saved,paths);}
  }catch(e){
   failed=request;message.textContent='Live sharing paused: '+e.message;retry.hidden=false;
  }finally{
   busy=false;
   if(!failed&&!dragging&&!pending){clear(draft);strokeId=null;message.textContent='Path saved to Home.';}
   highlight();
   if(!failed&&pending&&!dragging){clearTimeout(timer);timer=null;persist();}
   else if(!failed&&pending&&!timer)timer=setTimeout(()=>{timer=null;persist();},200);
  }
 }
 retry.onclick=async()=>{
  if(busy||!failed)return;
  // Retry the exact failed request before the latest coalesced update.
  const latest=pending;pending=failed;failed=null;
  await persist();if(latest&&!pending)pending=latest;
  if(pending&&!failed)persist();
 };
 undo.onclick=async()=>{if(busy||pending||failed||dragging||!paths?.items?.length)return;busy=true;const target=state();try{const result=await remove(target,paths.items.at(-1).id);if(sameScene(target)){paths=result;render(saved,paths);}message.textContent='Last path removed.';}catch(e){message.textContent=e.message;}finally{busy=false;highlight();}};
 function pick(e){const rect=canvas.getBoundingClientRect(),ray=new THREE.Raycaster();ray.far=40;ray.setFromCamera(new THREE.Vector2((e.clientX-rect.left)/rect.width*2-1,1-(e.clientY-rect.top)/rect.height*2),camera);
  const hit=ray.intersectObjects(voxels.children,true)[0],floor=ray.ray.intersectPlane(new THREE.Plane(new THREE.Vector3(0,1,0),0),new THREE.Vector3()),distance=floor?floor.distanceTo(ray.ray.origin):Infinity;
  if(hit&&hit.distance<=Math.min(distance,40))return hit.point.toArray();return floor&&distance<=40?floor.toArray():null;
 }
 function add(e){
  const point=pick(e);if(!point){segment=null;return;}if(!segment){segment=[];segments.push(segment);}
  const last=segment.at(-1);if(last&&Math.hypot(...point.map((v,i)=>v-last[i]))<.02)return;
  if(segments.reduce((n,s)=>n+s.length,0)>=4096)return;
  segment.push(point);render(draft,{items:[{segments:segments.filter(s=>s.length>1),width_m:.035,color:'#ffca55'}]});queue();highlight();
 }
 canvas.addEventListener('pointerdown',e=>{
  if(!enabled||e.button!==0)return;e.stopImmediatePropagation();e.preventDefault();
  if(busy||pending||failed)return;context=state();if(!context.editable)return;
  dragging=true;segments=[];segment=null;strokeId=newId();created=false;canvas.setPointerCapture(e.pointerId);add(e);
 },true);
 canvas.addEventListener('pointermove',e=>{if(!enabled)return;e.stopImmediatePropagation();if(!dragging||performance.now()-lastSample<35)return;lastSample=performance.now();add(e);},true);
 function finish(e,cancel=false){
  if(!enabled)return;e.stopImmediatePropagation();if(!dragging)return;
  if(!cancel)add(e);dragging=false;if(canvas.hasPointerCapture(e.pointerId))canvas.releasePointerCapture(e.pointerId);
  // Pointer cancellation ends a live stroke; already-published points remain shared.
  clearTimeout(timer);timer=null;if(!busy&&!failed)persist();
  if(!busy&&!pending&&!failed){clear(draft);strokeId=null;highlight();if(created)message.textContent='Path saved to Home.';}
 }
 canvas.addEventListener('pointerup',e=>finish(e),true);canvas.addEventListener('pointercancel',e=>finish(e,true),true);
 window.addEventListener('keydown',e=>{if(e.key==='Escape'&&enabled)mode(false);});
 return {objects:()=>saved.children,select(id){selectedId=id;highlight();},isActive:()=>enabled,update(){
  const s=state();button.disabled=!s.editable;button.title=s.editable?'Draw shared 3D paths':'Choose a local Home scene in 3D';
  if(enabled&&(!s.editable||(context&&!sameScene(context)))){
   dragging=false;clear(draft);enabled=false;button.setAttribute('aria-pressed','false');controls.enabled=prior;canvas.style.cursor='';panel.hidden=!(pending||busy||failed);
   clearTimeout(timer);timer=null;persist();
  }
  const next=JSON.stringify([s.frame,s.paths]);if(signature!==next){signature=next;paths=s.paths;render(saved,paths);highlight();}
  undo.disabled=busy||dragging||!!pending||!!failed||!paths?.items?.length;
 }};
}
