import * as THREE from 'three';
const newId=()=>crypto.randomUUID?.()||'10000000-1000-4000-8000-100000000000'.replace(/[018]/g,c=>(Number(c)^crypto.getRandomValues(new Uint8Array(1))[0]&15>>Number(c)/4).toString(16));

export function pathMeshes(paths){
 const group=new THREE.Group();
 for(const stroke of paths?.items||[]){
  const edges=stroke.segments.flatMap(s=>s.slice(1).map((b,i)=>[s[i],b])).filter(([a,b])=>Math.hypot(...a.map((v,i)=>v-b[i]))>1e-6);
  const mesh=new THREE.InstancedMesh(new THREE.CylinderGeometry(stroke.width_m/2,stroke.width_m/2,1,8),new THREE.MeshBasicMaterial({color:stroke.color}),edges.length);
  const matrix=new THREE.Matrix4(),up=new THREE.Vector3(0,1,0);
  edges.forEach(([a,b],i)=>{const start=new THREE.Vector3(...a),end=new THREE.Vector3(...b),delta=end.clone().sub(start),length=delta.length();matrix.compose(start.add(end).multiplyScalar(.5),new THREE.Quaternion().setFromUnitVectors(up,delta.normalize()),new THREE.Vector3(1,length,1));mesh.setMatrixAt(i,matrix);});group.add(mesh);
 }
 return group;
}
function clear(group){for(const node of [...group.children]){node.geometry?.dispose();node.material?.dispose();node.dispose?.();group.remove(node);}}
export function createCrayon({button,canvas,camera,controls,scene,voxels,state,save,remove}){
 const saved=new THREE.Group(),draft=new THREE.Group();scene.add(saved,draft);
 const panel=document.createElement('div');panel.className='crayon-panel';panel.hidden=true;
 panel.innerHTML='<span role="status">Drag on voxels or the floor.</span><button class="retry" hidden>Retry save</button><button class="undo">Undo last path</button><button class="done">Done</button>';canvas.parentElement.append(panel);
 const message=panel.querySelector('span'),retry=panel.querySelector('.retry'),undo=panel.querySelector('.undo');
 let enabled=false,dragging=false,busy=false,segments=[],segment=null,context=null,pending=null,signature='',lastSample=0,prior=true,paths=null;
 function render(target,value){clear(target);for(const child of [...pathMeshes(value).children])target.add(child);}
 function mode(on){if(busy||pending)return;enabled=on;button.setAttribute('aria-pressed',String(on));panel.hidden=!on;canvas.style.cursor=on?'crosshair':'';if(on){prior=controls.enabled;controls.enabled=false;}else{controls.enabled=prior;dragging=false;clear(draft);}}
 button.onclick=()=>{if(state().editable)mode(!enabled);};panel.querySelector('.done').onclick=()=>mode(false);
 async function persist(){if(!pending||busy)return;busy=true;retry.hidden=true;message.textContent='Saving shared path…';try{const target=pending.context,result=await save(target,pending.stroke);pending=null;clear(draft);if(state().map===target.map&&state().frame===target.frame){paths=result;render(saved,paths);}message.textContent='Path saved to Home.';}catch(e){message.textContent=`Not saved: ${e.message}`;retry.hidden=false;}finally{busy=false;}}
 retry.onclick=persist;
 undo.onclick=async()=>{if(busy||pending||!paths?.items?.length)return;busy=true;const target=state();try{const result=await remove(target,paths.items.at(-1).id);if(state().map===target.map&&state().frame===target.frame){paths=result;render(saved,paths);}message.textContent='Last path removed.';}catch(e){message.textContent=e.message;}finally{busy=false;}};
 function pick(e){const rect=canvas.getBoundingClientRect(),ray=new THREE.Raycaster();ray.far=40;ray.setFromCamera(new THREE.Vector2((e.clientX-rect.left)/rect.width*2-1,1-(e.clientY-rect.top)/rect.height*2),camera);
  const hit=ray.intersectObjects(voxels.children,true)[0],floor=ray.ray.intersectPlane(new THREE.Plane(new THREE.Vector3(0,1,0),0),new THREE.Vector3()),distance=floor?floor.distanceTo(ray.ray.origin):Infinity;
  if(hit&&hit.distance<=Math.min(distance,40))return hit.point.toArray();return floor&&distance<=40?floor.toArray():null;
 }
 function add(e){const point=pick(e);if(!point){segment=null;return;}if(!segment){segment=[];segments.push(segment);}const last=segment.at(-1);if(last&&Math.hypot(...point.map((v,i)=>v-last[i]))<.02)return;if(segments.reduce((n,s)=>n+s.length,0)>=4096)return;segment.push(point);render(draft,{items:[{segments:segments.filter(s=>s.length>1),width_m:.035,color:'#ffca55'}]});}
 canvas.addEventListener('pointerdown',e=>{if(!enabled||e.button!==0)return;e.stopImmediatePropagation();e.preventDefault();if(busy||pending)return;context=state();if(!context.editable)return;dragging=true;segments=[];segment=null;canvas.setPointerCapture(e.pointerId);add(e);},true);
 canvas.addEventListener('pointermove',e=>{if(!enabled)return;e.stopImmediatePropagation();if(!dragging||performance.now()-lastSample<35)return;lastSample=performance.now();add(e);},true);
 function finish(e,cancel=false){if(!enabled)return;e.stopImmediatePropagation();if(!dragging)return;dragging=false;if(canvas.hasPointerCapture(e.pointerId))canvas.releasePointerCapture(e.pointerId);if(!cancel)add(e);const valid=segments.filter(s=>s.length>=2);if(cancel||!valid.length){clear(draft);return;}pending={context,stroke:{id:newId(),title:'Crayon path',color:'#ffca55',width_m:.035,segments:valid}};persist();}
 canvas.addEventListener('pointerup',e=>finish(e),true);canvas.addEventListener('pointercancel',e=>finish(e,true),true);
 window.addEventListener('keydown',e=>{if(e.key==='Escape'&&enabled)mode(false);});
 return {isActive:()=>enabled,update(){const s=state();button.disabled=!s.editable;button.title=s.editable?'Draw shared 3D paths':'Choose a local Home scene in 3D';if(enabled&&(!s.editable||(context&&(context.map!==s.map||context.frame!==s.frame)))){dragging=false;clear(draft);enabled=false;button.setAttribute('aria-pressed','false');controls.enabled=prior;canvas.style.cursor='';panel.hidden=!pending;}const next=JSON.stringify([s.frame,s.paths]);if(signature!==next){signature=next;paths=s.paths;render(saved,paths);}undo.disabled=busy||!paths?.items?.length;}};
}
