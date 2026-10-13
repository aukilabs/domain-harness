import * as THREE from 'three';

// Semantic boxes are independent of transient voxel and robot-occupancy layers.
export function createSemanticVolumes({scene, camera, renderer, controls, panel, api, label, clear, openPanel, priorityPickObjects=()=>[]}) {
 const group=new THREE.Group(),preview=new THREE.Group();scene.add(group,preview);
 let context=null,items=[],draft=null,editing=null,drawing=false,start=null,press=null,pending=false,signature='',resize=null,conflicted=false;
 const handleLayer=document.createElement('div');handleLayer.className='volume-handles';renderer.domElement.parentElement.append(handleLayer);
 const leaders=document.createElementNS('http://www.w3.org/2000/svg','svg');leaders.classList.add('volume-handle-leaders');handleLayer.append(leaders);
 const toolbar=document.createElement('nav');toolbar.className='volume-tool-bar';toolbar.setAttribute('aria-label','Volume editing tools');toolbar.innerHTML='<button type="button" class="volume-select-tool" aria-pressed="true"><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="m4 3 7 18 2-8 8-2Z"/></svg>Select</button><button type="button" class="volume-create-tool" aria-pressed="false"><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="m3 6 9-4 9 4-9 4ZM3 6v12l9 4 9-4V6M12 10v12M18 2v6M15 5h6"/></svg>Create volume</button>';
 renderer.domElement.parentElement.append(toolbar);
 toolbar.querySelector('.volume-select-tool').onclick=()=>{if(drawing)cancel();else endDrawing();message('Select a volume to edit it in the scene.');};
 toolbar.querySelector('.volume-create-tool').onclick=()=>{openPanel('volumes');$('volume-draw').click();};
 const handles=[];
 // Handle roles, glyphs, colors and rail layout follow Peyote's
 // AxisRailHandle, handle-icons and OutlineHandles (meter-based Home adapter).
 const descriptors=[
  {kind:'resize',axis:0,sign:-1,icon:'horizontal',name:'Resize −X face'},
  {kind:'resize',axis:0,sign:1,icon:'horizontal',name:'Resize +X face'},
  {kind:'resize',axis:1,sign:1,icon:'vertical',name:'Resize +Y face'},
  {kind:'resize',axis:2,sign:-1,icon:'depth',name:'Resize −Z face'},
  {kind:'move',axis:0,sign:0,icon:'move',name:'Move volume'},
  {kind:'rotate',axis:0,sign:-1,icon:'rotate-left',name:'Rotate left corner'},
  {kind:'rotate',axis:0,sign:1,icon:'rotate-right',name:'Rotate right corner'},
 ];
 for(const descriptor of descriptors){
  const {axis,sign,kind,name}=descriptor;
  const button=document.createElement('button');button.type='button';button.className='volume-resize-handle';button.dataset.axis=String(axis);button.dataset.kind=kind;button.dataset.sign=String(sign);
  button.setAttribute('aria-label',name);button.title=kind==='rotate'?'Rotate around the opposite corner · Shift snaps to 15°':kind==='move'?'Move along the ground · Shift snaps to 10 cm':`Resize ${['width','height','depth'][axis]} · Shift snaps to 10 cm`;button.innerHTML=handleIcon(descriptor.icon);button.hidden=true;handleLayer.append(button);
  const leader=document.createElementNS('http://www.w3.org/2000/svg','line');leader.setAttribute('stroke','#3b82f6');leader.setAttribute('stroke-width','1');leader.setAttribute('opacity','.65');leaders.append(leader);
  const handle={...descriptor,button,leader};handles.push(handle);
  button.addEventListener('pointerdown',e=>startResize(e,handle));
  button.addEventListener('pointermove',moveResize);
  button.addEventListener('pointerup',e=>finishResize(e));
  button.addEventListener('pointercancel',()=>stopResize(true));
  button.addEventListener('lostpointercapture',()=>{if(resize)stopResize(true);});
  button.addEventListener('keydown',e=>{if(!editable()||!draft||!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(e.key))return;e.preventDefault();const direction=['ArrowRight','ArrowUp'].includes(e.key)?1:-1,original=structuredClone(draft);if(kind==='resize')resizeFace(original,handle,direction*(e.shiftKey?.01:.1));else if(kind==='rotate')rotateCorner(original,handle,direction*(e.shiftKey?15:1)*Math.PI/180);else{original.center[e.key==='ArrowUp'||e.key==='ArrowDown'?2:0]+=direction*(e.shiftKey?.01:.1);setDraft(original);}message('Volume adjusted. Save volume to keep the change.');});
 }
 function handleIcon(icon){
  function arrow(dx,dy){const l=Math.hypot(dx,dy);dx/=l;dy/=l;const x=dx*12,y=dy*12;return [[-x,-y,x,y],[x,y,x-dx*5.4-dy*3.9,y-dy*5.4+dx*3.9],[x,y,x-dx*5.4+dy*3.9,y-dy*5.4-dx*3.9],[-x,-y,-x+dx*5.4-dy*3.9,-y+dy*5.4+dx*3.9],[-x,-y,-x+dx*5.4+dy*3.9,-y+dy*5.4-dx*3.9]];}
  let segments=icon==='move'?[...arrow(1,0),...arrow(0,1)]:icon==='vertical'?arrow(0,1):icon==='depth'?arrow(.707,-.707):arrow(1,0);
  if(icon.startsWith('rotate')){const direction=icon==='rotate-left'?-1:1,start=direction<0?-.25:Math.PI+.25,sweep=direction*Math.PI*1.45;segments=[];let x=Math.cos(start)*9,y=Math.sin(start)*9;for(let i=1;i<=16;i++){const a=start+sweep*i/16,nx=Math.cos(a)*9,ny=Math.sin(a)*9;segments.push([x,y,nx,ny]);x=nx;y=ny;}let dx=direction*.75,dy=.35,len=Math.hypot(dx,dy);dx/=len;dy/=len;segments.push([x,y,x-dx*5.4-dy*3.9,y-dy*5.4+dx*3.9],[x,y,x-dx*5.4+dy*3.9,y-dy*5.4-dx*3.9]);}
  const d=segments.map(([x,y,u,v])=>`M${x} ${y}L${u} ${v}`).join('');
  const rail=icon==='move'||icon.startsWith('rotate')?'M-23 0H23M0-23V23':icon==='vertical'?'M0-24V24':icon==='depth'?'M-17 17L17-17':'M-24 0H24';
  return `<svg width="48" height="48" viewBox="-36 -36 72 72" aria-hidden="true"><path d="${rail}" stroke="currentColor" stroke-width="4.5" opacity=".7"/><circle r="24" fill="currentColor" opacity=".96"/><circle r="22.8" fill="none" class="handle-ring" stroke-width="2.3"/><path d="${d}" fill="none" stroke="#27272a" stroke-width="6.6" opacity=".58" stroke-linecap="round"/><path d="${d}" fill="none" stroke="#f8fafc" stroke-width="3.1" stroke-linecap="round"/></svg>`;
 }
 function axisVector(axis,yaw){return new THREE.Vector3(...[0,1,2].map(i=>i===axis?1:0)).applyAxisAngle(new THREE.Vector3(0,1,0),yaw);}
 function localPoint(v,point){return new THREE.Vector3(...point).applyAxisAngle(new THREE.Vector3(0,1,0),v.yaw).add(new THREE.Vector3(...v.center));}
 function facePoint(v,h){
  if(h.kind==='move')return localPoint(v,[0,-v.size[1]/2+.07,v.size[2]/2+.06]);
  if(h.kind==='rotate')return localPoint(v,[h.sign*v.size[0]/2,-v.size[1]/2,v.size[2]/2+.02]);
  if(h.axis===0)return localPoint(v,[h.sign*(v.size[0]/2+.12),0,v.size[2]/2-.02]);
  if(h.axis===1)return localPoint(v,[0,v.size[1]/2+.16,v.size[2]/2-.02]);
  return localPoint(v,[0,0,-v.size[2]/2-.12]);
 }
 function tick(){
  const visible=draft&&editable()&&!drawing&&preview.visible;
  if(!visible){for(const h of handles){h.button.hidden=true;h.leader.setAttribute('visibility','hidden');}return;}
  const rect=renderer.domElement.getBoundingClientRect(),parent=handleLayer.parentElement.getBoundingClientRect();
  leaders.setAttribute('viewBox',`0 0 ${parent.width} ${parent.height}`);const placed=[];
  for(const h of handles){h.button.hidden=!visible;if(!visible)continue;const p=facePoint(draft,h).project(camera);h.button.hidden=p.z< -1||p.z>1||Math.abs(p.x)>1||Math.abs(p.y)>1;
   h.button.classList.toggle('active',resize?.h===h);
   const x=rect.left-parent.left+(p.x+1)*rect.width/2,y=rect.top-parent.top+(1-p.y)*rect.height/2;let offset=0;
   if(!h.button.hidden){while(placed.some(q=>Math.hypot(q.x-x,q.y-y-offset)<52))offset-=52;if(y+offset<26){offset=0;while(placed.some(q=>Math.hypot(q.x-x,q.y-y-offset)<52))offset+=52;}placed.push({x,y:y+offset});}
   h.button.style.left=x+'px';h.button.style.top=(y+offset)+'px';
   h.leader.setAttribute('visibility',offset&&!h.button.hidden?'visible':'hidden');h.leader.setAttribute('x1',String(x));h.leader.setAttribute('y1',String(y));h.leader.setAttribute('x2',String(x));h.leader.setAttribute('y2',String(y+offset));
  }
 }
 function pointerRay(e){const rect=renderer.domElement.getBoundingClientRect(),ray=new THREE.Raycaster();ray.setFromCamera(new THREE.Vector2((e.clientX-rect.left)/rect.width*2-1,-(e.clientY-rect.top)/rect.height*2+1),camera);return ray.ray;}
 function startResize(e,h){
  if(e.button!==0||!e.isPrimary||!editable()||!draft||drawing)return;e.preventDefault();e.stopPropagation();
  const original=structuredClone(draft),axis=axisVector(h.axis,original.yaw),face=facePoint(original,h);
  const normal=h.kind==='resize'?camera.position.clone().sub(face):new THREE.Vector3(0,1,0);if(h.kind==='resize')normal.addScaledVector(axis,-normal.dot(axis));
  if(normal.lengthSq()<1e-8){message('Orbit the view slightly to resize this face.');return;}
  const plane=new THREE.Plane().setFromNormalAndCoplanarPoint(normal.normalize(),face),point=pointerRay(e).intersectPlane(plane,new THREE.Vector3());if(!point)return;
  resize={original,h,axis,plane,point,pointerId:e.pointerId};controls.enabled=false;h.button.setPointerCapture(e.pointerId);tick();message(h.kind==='rotate'?'Rotate around the opposite bottom corner. Shift snaps to 15°. Escape cancels.':h.kind==='move'?'Move on the ground plane. Shift snaps to 10 cm. Escape cancels.':'Resize with the opposite face fixed. Shift snaps to 10 cm. Escape cancels.');
 }
 function setDraft(v){if([...v.center,...v.size,v.yaw].some(n=>!Number.isFinite(n))||v.center.some(n=>Math.abs(n)>100000))return;draft=v;fill(draft);draw();tick();}
 function rotateCorner(original,h,angle){
  const pivotLocal=[-h.sign*original.size[0]/2,-original.size[1]/2,original.size[2]/2];
  const pivot=localPoint(original,pivotLocal),yaw=Math.atan2(Math.sin(original.yaw+angle),Math.cos(original.yaw+angle));
  const offset=new THREE.Vector3(...pivotLocal).applyAxisAngle(new THREE.Vector3(0,1,0),yaw);
  setDraft({...structuredClone(original),yaw,center:pivot.sub(offset).toArray()});
 }
 function resizeFace(original,h,delta){
  const size=THREE.MathUtils.clamp(original.size[h.axis]+delta,.01,10000),change=size-original.size[h.axis];
  const center=new THREE.Vector3(...original.center).addScaledVector(axisVector(h.axis,original.yaw),h.sign*change/2);if(center.toArray().some(n=>Math.abs(n)>100000))return;
  draft=structuredClone(original);draft.size[h.axis]=size;draft.center=center.toArray();fill(draft);draw();tick();
 }
 function moveResize(e){if(!resize||e.pointerId!==resize.pointerId)return;e.preventDefault();e.stopPropagation();const p=pointerRay(e).intersectPlane(resize.plane,new THREE.Vector3());if(!p)return;const {original,h}=resize;
  if(h.kind==='move'){const delta=p.sub(resize.point);if(e.shiftKey){delta.x=Math.round(delta.x*10)/10;delta.z=Math.round(delta.z*10)/10;}const next=structuredClone(original);next.center[0]+=delta.x;next.center[2]+=delta.z;setDraft(next);return;}
  if(h.kind==='rotate'){const pivot=localPoint(original,[-h.sign*original.size[0]/2,-original.size[1]/2,original.size[2]/2]);const from=axisVector(0,original.yaw).multiplyScalar(h.sign*original.size[0]);const to=from.clone().add(p.sub(resize.point));if(to.lengthSq()<1e-8)return;let angle=Math.atan2(from.z*to.x-from.x*to.z,from.x*to.x+from.z*to.z);if(e.shiftKey)angle=Math.round(angle/(Math.PI/12))*Math.PI/12;rotateCorner(original,h,angle);return;}
  let delta=p.sub(resize.point).dot(resize.axis)*h.sign;if(e.shiftKey)delta=Math.round(delta*10)/10;resizeFace(original,h,delta);
 }
 function stopResize(restore=false){if(!resize)return;const r=resize;resize=null;if(restore){draft=r.original;fill(draft);draw();}if(r.h.button.hasPointerCapture(r.pointerId))r.h.button.releasePointerCapture(r.pointerId);controls.enabled=true;tick();}
 function finishResize(e){if(!resize||e.pointerId!==resize.pointerId)return;moveResize(e);stopResize();if(editing)persist(false,true);else message('Volume adjusted. Add a title and save to create it.');}
 panel.innerHTML='<p class="panel-hint">Named regions saved in the selected Home scene. Dimensions are in meters.</p><div class="volume-actions"><button id="volume-new" type="button">New box</button><button id="volume-draw" type="button">Draw box</button></div><label class="volume-layer"><input id="volume-visible" type="checkbox" checked> Show volumes</label><p id="volume-status" class="panel-hint" role="status"></p><div id="volume-list"></div><form id="volume-form" class="note-form" hidden><label>Title<input name="title" maxlength="120" required></label><label>Description<textarea name="description" maxlength="10000"></textarea></label><div class="volume-grid"></div><div class="volume-actions"><button type="submit">Save volume</button><button id="volume-cancel" type="button">Cancel</button><button id="volume-delete" type="button">Delete</button></div></form>';
 const $=id=>panel.querySelector('#'+id),form=$('volume-form'),status=$('volume-status'),list=$('volume-list');
 const fields=[['x','Center X'],['z','Center Z'],['base','Base elevation'],['width','Width'],['depth','Depth'],['height','Height'],['yaw','Rotation (degrees)']];
 for(const [name,title] of fields){const l=document.createElement('label');l.textContent=title;const input=document.createElement('input');input.type='number';input.name=name;input.required=true;input.step='any';input.min=['width','depth','height'].includes(name)?'.01':name==='yaw'?'-360':'-100000';input.max=name==='yaw'?'360':['width','depth','height'].includes(name)?'10000':'100000';l.append(input);form.querySelector('.volume-grid').append(l);}
 function message(text){status.textContent=text;}
 function editable(){return context?.editable && !pending;}
 function buttons(){for(const id of ['volume-new','volume-draw'])$(id).disabled=!editable();for(const b of form.querySelectorAll('button'))b.disabled=pending;toolbar.querySelector('.volume-create-tool').disabled=!editable();toolbar.querySelector('.volume-select-tool').disabled=pending;toolbar.querySelector('.volume-create-tool').setAttribute('aria-pressed',String(drawing));toolbar.querySelector('.volume-select-tool').setAttribute('aria-pressed',String(!drawing));}
 function mesh(volume,parent,selected=false){
  const box=new THREE.Mesh(new THREE.BoxGeometry(1,1,1),new THREE.MeshBasicMaterial({color:selected?0xffb45c:0x61b4ef,transparent:true,opacity:selected?.2:.1,depthWrite:false,side:THREE.DoubleSide}));
  box.position.fromArray(volume.center);box.scale.fromArray(volume.size);box.rotation.y=volume.yaw;box.userData.volume=volume;parent.add(box);
  const outline=new THREE.LineSegments(new THREE.EdgesGeometry(box.geometry),new THREE.LineBasicMaterial({color:selected?0xffb45c:0x61b4ef}));outline.position.copy(box.position);outline.scale.copy(box.scale);outline.rotation.copy(box.rotation);parent.add(outline);
  const title=label(volume.title||'New volume','#286f9e');title.position.set(volume.center[0],volume.center[1]+volume.size[1]/2+.15,volume.center[2]);parent.add(title);
 }
 function draw(){clear(group);for(const v of items)if(!(draft&&v.id===editing))mesh(v,group,v.id===editing);clear(preview);if(draft)mesh(draft,preview,true);renderer.domElement.dataset.semanticVolumeCount=String(items.length);}
 function focus(v){const distance=Math.max(3,Math.max(...v.size)*2.2);controls.target.fromArray(v.center);camera.position.copy(controls.target).add(new THREE.Vector3(distance*.7,distance*.65,distance*.7));controls.update();}
 function showList(){list.replaceChildren();for(const v of items){const b=document.createElement('button');b.className='panel-row';b.type='button';b.dataset.volumeId=v.id;b.setAttribute('aria-pressed',String(v.id===editing));const title=document.createElement('strong');title.textContent=v.title;const d=document.createElement('small');d.textContent=v.description;b.append(title,d);b.onclick=()=>select(v);list.append(b);}if(!items.length){const p=document.createElement('p');p.className='panel-hint';p.textContent='No semantic volumes in this scene.';list.append(p);}}
 function fill(v){form.elements.title.value=v.title;form.elements.description.value=v.description;const values={x:v.center[0],z:v.center[2],base:v.center[1]-v.size[1]/2,width:v.size[0],height:v.size[1],depth:v.size[2],yaw:v.yaw*180/Math.PI};for(const [name,n] of Object.entries(values))form.elements[name].value=String(n);}
 function values(){const n=name=>Number(form.elements[name].value);return {id:editing||'',title:form.elements.title.value.trim(),description:form.elements.description.value,semantic_type:draft?.semantic_type||'region',center:[n('x'),n('base')+n('height')/2,n('z')],size:[n('width'),n('height'),n('depth')],yaw:n('yaw')*Math.PI/180};}
 function endDrawing(){stopResize();drawing=false;start=null;controls.enabled=true;renderer.domElement.style.cursor='';tick();buttons();}
 function cancel(){endDrawing();draft=null;editing=null;conflicted=false;form.hidden=true;draw();showList();buttons();}
 function select(v){if(pending)return;cancel();openPanel('volumes');editing=v.id;draft=context?.editable?structuredClone(v):null;fill(v);form.hidden=false;for(const input of form.querySelectorAll('input,textarea'))input.disabled=!context?.editable;form.querySelector('.volume-actions').hidden=!context?.editable;$('volume-delete').hidden=false;message(context?.editable?'Blue handles move, resize and rotate. Changes save when you release a handle.':'Published volume · read-only');draw();showList();}
 function begin(){if(!editable())return;cancel();draft={id:'',title:'',description:'',center:[controls.target.x,1,controls.target.z],size:[1,2,.6],yaw:0};for(const input of form.querySelectorAll('input,textarea'))input.disabled=false;form.querySelector('.volume-actions').hidden=false;fill(draft);form.hidden=false;$('volume-delete').hidden=true;draw();message('Use the scene handles to position and size the volume, then save.');}
 $('volume-new').onclick=begin;
 $('volume-draw').onclick=()=>{if(!editable())return;if(drawing){cancel();message('Creation cancelled.');return;}begin();drawing=true;start=null;controls.enabled=false;const target=controls.target.clone(),distance=Math.max(3,camera.position.distanceTo(target));camera.position.copy(target).add(new THREE.Vector3(0,distance,.001));controls.update();renderer.domElement.style.cursor='crosshair';buttons();message('Drag the front edge on the ground. Shift locks it to X or Z. Escape cancels creation.');};
 $('volume-cancel').onclick=()=>{if(!pending){cancel();message('Cancelled.');}};
 $('volume-visible').onchange=()=>{group.visible=preview.visible=$('volume-visible').checked;};
 form.oninput=()=>{if(draft&&!pending){const v=values();if([...v.center,...v.size,v.yaw].every(Number.isFinite)&&v.size.every(n=>n>=.01)){draft=v;draw();}}};
 function url(id){return '/api/semantic-volumes'+(id?'/'+encodeURIComponent(id):'')+'?scene='+encodeURIComponent(context.map);}
 async function persist(remove=false,keepSelection=false){
  if(!editable()||!draft)return;
  if(conflicted){message('This volume changed elsewhere while you were editing. Copy your edits, then reselect the volume to reload before saving.');return;}
  const selected=context,identifier=editing,body={frame:selected.frame,volume:remove?null:values()};
  pending=true;buttons();endDrawing();message('Saving…');
  try {const response=await api(url(identifier),body,remove?'DELETE':identifier?'PATCH':'POST');if(context!==selected)return;items=response.items;signature=JSON.stringify(response);if(keepSelection){draft=structuredClone(items.find(v=>v.id===identifier));fill(draft);draw();showList();}else cancel();message(remove?'Volume deleted.':'Volume saved in this scene.');}
  catch(e){if(context===selected)message(e.message);}
  finally{if(context===selected){pending=false;buttons();}}
 }
 form.onsubmit=e=>{e.preventDefault();if(form.reportValidity())persist();};
 $('volume-delete').onclick=()=>{if(editing&&confirm('Delete this semantic volume?'))persist(true);};
 function groundPoint(e){const rect=renderer.domElement.getBoundingClientRect(),ray=new THREE.Raycaster();ray.setFromCamera(new THREE.Vector2((e.clientX-rect.left)/rect.width*2-1,-(e.clientY-rect.top)/rect.height*2+1),camera);return ray.ray.intersectPlane(new THREE.Plane(new THREE.Vector3(0,1,0),-Number(form.elements.base.value)),new THREE.Vector3());}
 function footprint(p,shift=false){
  if(shift){if(Math.abs(p.x-start.x)>=Math.abs(p.z-start.z))p.z=start.z;else p.x=start.x;}
  const height=Number(form.elements.height.value),base=Number(form.elements.base.value),depth=Number(form.elements.depth.value),dx=p.x-start.x,dz=p.z-start.z,width=Math.hypot(dx,dz);if(width<.01)return;
  draft={...values(),center:[(start.x+p.x)/2+(dz/width)*depth/2,base+height/2,(start.z+p.z)/2-(dx/width)*depth/2],size:[width,height,depth],yaw:-Math.atan2(dz,dx)};fill(draft);draw();
 }
 const canvas=renderer.domElement;
 canvas.addEventListener('pointerdown',e=>{press=[e.clientX,e.clientY];if(!drawing||e.button!==0)return;e.stopImmediatePropagation();e.preventDefault();start=groundPoint(e);if(start)canvas.setPointerCapture(e.pointerId);},true);
 canvas.addEventListener('pointermove',e=>{if(!drawing)return;e.stopImmediatePropagation();if(start){const p=groundPoint(e);if(p)footprint(p,e.shiftKey);}},true);
 canvas.addEventListener('pointerup',e=>{if(drawing){e.stopImmediatePropagation();e.preventDefault();if(start){const p=groundPoint(e);if(p)footprint(p,e.shiftKey);if(canvas.hasPointerCapture(e.pointerId))canvas.releasePointerCapture(e.pointerId);if(!p||p.distanceTo(start)<.3){start=null;message('Drag a front edge at least 30 cm long.');return;}endDrawing();message('Front edge ready. Add a title and use the handles to adjust the volume.');focus(draft);form.elements.title.focus();}return;}
  if(pending||e.button!==0||!press||Math.hypot(e.clientX-press[0],e.clientY-press[1])>4||!group.visible)return;
  const r=canvas.getBoundingClientRect(),ray=new THREE.Raycaster();ray.setFromCamera(new THREE.Vector2((e.clientX-r.left)/r.width*2-1,-(e.clientY-r.top)/r.height*2+1),camera);
  // Notes are drawn through translucent volumes. Let the normal scene picker
  // handle them before this capture-phase listener claims the click.
  const priority=priorityPickObjects().filter(o=>{for(let p=o;p;p=p.parent){if(!p.visible)return false;}return true;});
  if(ray.intersectObjects(priority).length)return;
  const hit=ray.intersectObjects([...group.children,...preview.children]).find(h=>h.object.userData.volume);if(hit){e.semanticVolume=true;select(hit.object.userData.volume);}
 },true);
 canvas.addEventListener('pointercancel',()=>{if(drawing){cancel();message('Creation cancelled.');}});
 document.addEventListener('keydown',e=>{if(e.key==='Escape'&&resize){stopResize(true);message('Edit cancelled.');}else if(e.key==='Escape'&&drawing){cancel();message('Creation cancelled.');}});
 return {tick,pick(ray){if(drawing||resize||!group.visible)return null;return ray.intersectObjects([...group.children,...preview.children]).find(h=>h.object.userData.volume);},update(map,data){const editable=map.startsWith('local:')||map==='demo';if(!context||context.map!==map||context.frame!==data?.frame){context={map,frame:data?.frame,editable};pending=false;items=[];cancel();signature='';message(editable?'Create a box or drag its footprint in the viewer.':'Select a local Home scene to create volumes.');}const collection=data?.semantic_volumes;const next=JSON.stringify(collection||null);if(next!==signature){signature=next;const previous=items.find(v=>v.id===editing);items=collection?.frame===data.frame?collection.items:[];const remote=items.find(v=>v.id===editing);if(editing&&!pending&&!resize){if(!remote){cancel();message('This volume was deleted elsewhere.');}else if(draft&&previous){const merged=mergeVolumeDraft(previous,values(),remote);draft=merged.draft;conflicted=conflicted||merged.conflicts.length>0;fill(draft);if(conflicted)message('Conflicting edits: '+merged.conflicts.join(', ')+'. Copy your edits and reselect the volume before saving.');}else if(!draft)fill(remote);}draw();showList();}buttons();},reset(){context=null;items=[];pending=false;cancel();buttons();message('Loading selected scene…');}};
}

// Refresh untouched fields while retaining unsaved edits; never silently overwrite a conflict.
export function mergeVolumeDraft(previous,local,remote){
 const draft=structuredClone(local),conflicts=[];
 for(const key of Object.keys(remote)){
  const equal=(a,b)=>typeof a==='number'&&typeof b==='number'?Math.abs(a-b)<1e-9:Array.isArray(a)&&Array.isArray(b)?a.length===b.length&&a.every((n,i)=>equal(n,b[i])):JSON.stringify(a)===JSON.stringify(b);
  if(equal(local[key],previous[key]))draft[key]=structuredClone(remote[key]);
  else if(!equal(remote[key],previous[key])&&!equal(remote[key],local[key]))conflicts.push(key);
 }
 return {draft,conflicts};
}
export function legacyVolumesForView(data){return data.semantic_volumes?.frame===data.frame?[]:(data.volumes||[]);}
