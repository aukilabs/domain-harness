import * as THREE from 'three';
import { icons, icon, sectionHeader, listRow, actionButton, markSelected } from './panel-ui.js';

// Semantic boxes are independent of transient voxel and robot-occupancy layers.
export function createSemanticVolumes({scene, camera, renderer, controls, panel, api, label, clear, openPanel, inspector, toast=()=>{}, flyTo=null, priorityPickObjects=()=>[], labels=null, priorityHit=()=>null}) {
 const group=new THREE.Group(),preview=new THREE.Group();scene.add(group,preview);
 let context=null,items=[],draft=null,editing=null,drawing=false,start=null,press=null,pending=false,signature='',resize=null,conflicted=false;
 const handleLayer=document.createElement('div');handleLayer.className='volume-handles';renderer.domElement.parentElement.append(handleLayer);
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
  const handle={...descriptor,button,opacity:1,live:true};handles.push(handle);
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
  // Rotate uses a standard circular arrow: counter-clockwise on the left corner, clockwise on the right.
  if(icon.startsWith('rotate')){const d=icon==='rotate-left'?'M3.5 12a8.5 8.5 0 1 0 2.6-6.1L3.5 8.5M3.5 3.5v5h5':'M20.5 12a8.5 8.5 0 1 1-2.6-6.1l2.6 2.6M20.5 3.5v5h-5';
   return `<svg width="20" height="20" viewBox="0 0 24 24" aria-hidden="true"><path d="${d}" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/></svg>`;}
  const d=segments.map(([x,y,u,v])=>`M${x} ${y}L${u} ${v}`).join('');
  return `<svg width="22" height="22" viewBox="-17 -17 34 34" aria-hidden="true"><path d="${d}" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round"/></svg>`;
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
 // Handles follow the box's size on screen continuously, with no jumps: the secondary handles fade and shrink as the
 // box gets small, the +Y handle later, the move handle never, and a "Zoom to edit" pill joins it once the box is
 // tiny. Face handles sit a fixed 26 px out along their face's on-screen normal (fading as the face turns towards
 // the camera, when it can't be dragged sensibly), rotate handles 24 px out from their corners, and a handle that
 // would land on a higher-priority one fades rather than being pushed aside. The drag maths still uses facePoint().
 const zoomPill=document.createElement('button');zoomPill.type='button';zoomPill.className='volume-zoom-pill';zoomPill.textContent='Zoom to edit';zoomPill.hidden=true;handleLayer.append(zoomPill);
 zoomPill.onclick=()=>{if(draft)focus(draft);};
 const smooth=(a,b,x)=>{const t=Math.min(1,Math.max(0,(x-a)/(b-a)));return t*t*(3-2*t);};
 const rank=h=>h.kind==='move'?0:h.kind==='rotate'?4:h.axis===1?1:h.axis===0?2:3;
 function anchor(v,h){
  if(h.kind==='move')return [localPoint(v,[0,-v.size[1]/2,v.size[2]/2]),axisVector(2,v.yaw)];
  if(h.kind==='rotate')return [localPoint(v,[h.sign*v.size[0]/2,-v.size[1]/2,v.size[2]/2]),null];
  if(h.axis===0)return [localPoint(v,[h.sign*v.size[0]/2,0,v.size[2]/2-.02]),axisVector(0,v.yaw).multiplyScalar(h.sign)];
  if(h.axis===1)return [localPoint(v,[0,v.size[1]/2,v.size[2]/2-.02]),new THREE.Vector3(0,1,0)];
  return [localPoint(v,[0,0,-v.size[2]/2]),axisVector(2,v.yaw).multiplyScalar(-1)];
 }
 let shownDraft=null;
 function tick(){
  const visible=draft&&editable()&&!drawing&&preview.visible;
  if(!visible){for(const h of handles)h.button.hidden=true;zoomPill.hidden=true;shownDraft=null;return;}
  const rect=renderer.domElement.getBoundingClientRect(),parent=handleLayer.parentElement.getBoundingClientRect(),ox=rect.left-parent.left,oy=rect.top-parent.top;
  const screen=p=>{const q=p.clone().project(camera);return {x:ox+(q.x+1)*rect.width/2,y:oy+(1-q.y)*rect.height/2,on:q.z>-1&&q.z<1};};
  // S: the box's size on screen, the square root of the area its 8 corners cover.
  let minX=Infinity,minY=Infinity,maxX=-Infinity,maxY=-Infinity;
  for(const sx of [-1,1])for(const sy of [-1,1])for(const sz of [-1,1]){const c=screen(localPoint(draft,[sx*draft.size[0]/2,sy*draft.size[1]/2,sz*draft.size[2]/2]));minX=Math.min(minX,c.x);maxX=Math.max(maxX,c.x);minY=Math.min(minY,c.y);maxY=Math.max(maxY,c.y);}
  const S=Math.sqrt(Math.max(0,(maxX-minX)*(maxY-minY))),centre={x:(minX+maxX)/2,y:(minY+maxY)/2};
  const right=new THREE.Vector3().setFromMatrixColumn(camera.matrixWorld,0).multiplyScalar(.5),placed=[],mounting=shownDraft!==draft.id;shownDraft=draft.id;
  for(const h of [...handles].sort((a,b)=>rank(a)-rank(b))){
   const [world,normal]=anchor(draft,h),at=screen(world);let x=at.x,y=at.y,opacity=h.kind==='move'?1:h.kind==='resize'&&h.axis===1?smooth(70,100,S):smooth(110,170,S);
   if(normal){const tip=screen(world.clone().addScaledVector(normal,.5)),side=screen(world.clone().add(right)),dx=tip.x-x,dy=tip.y-y,length=Math.hypot(dx,dy),metre=Math.hypot(side.x-x,side.y-y)||1;
    if(length>1e-3){x+=dx/length*26;y+=dy/length*26;}if(h.kind==='resize')opacity*=smooth(.25,.45,length/metre);}
   else{const dx=x-centre.x,dy=y-centre.y,length=Math.hypot(dx,dy)||1;x+=dx/length*24;y+=dy/length*24;}
   if(!at.on)opacity=0;
   for(const q of placed)if(q.opacity>.05)opacity*=smooth(28,40,Math.hypot(q.x-x,q.y-y));
   const active=resize?.h===h;if(active)opacity=1;placed.push({x,y,opacity});
   h.button.hidden=false;h.button.classList.toggle('active',active);
   h.button.style.transform=`translate(${x}px,${y}px) translate(-50%,-50%) scale(${active?1.08:.8+.2*opacity})`;h.button.style.opacity=String(opacity);
   h.button.style.visibility=opacity<.05?'hidden':'';
   // Only clearly visible handles take the pointer and focus, with a little hysteresis.
   const live=h.live?opacity>.55:opacity>.65;if(live!==h.live){h.live=live;h.button.style.pointerEvents=live?'':'none';h.button.inert=!live;}
   if(mounting&&opacity>.05&&!matchMedia('(prefers-reduced-motion: reduce)').matches)h.button.animate([{scale:.4},{scale:1}],{duration:280,delay:rank(h)*20,easing:'cubic-bezier(.34,1.56,.64,1)',fill:'backwards'});
   if(h.kind==='move'){const pill=1-smooth(55,70,S);zoomPill.hidden=pill<.05;zoomPill.style.opacity=String(pill);zoomPill.style.transform=`translate(${x}px,${y+28}px) translate(-50%,0)`;zoomPill.inert=pill<.6;}
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
 function finishResize(e){if(!resize||e.pointerId!==resize.pointerId)return;moveResize(e);stopResize();if(editing&&cardMode==='view')persist(false,true);else message(editing?'Handle change pending · Save volume to keep it.':'Volume adjusted. Add a title and save to create it.');}
 // Panel: create, show/hide, list. Detail and editing live in the inspector card.
 const newVolume=actionButton('New volume',{variant:'primary',iconPath:icons.plus,id:'volume-draw'});
 const reason=document.createElement('p');reason.className='panel-hint';
 const status=document.createElement('p');status.id='volume-status';status.className='form-status';status.setAttribute('role','status');
 const header=sectionHeader('Volumes',0);
 const eye=document.createElement('button');eye.type='button';eye.id='volume-visible';eye.className='icon-toggle';eye.setAttribute('aria-pressed','true');eye.setAttribute('aria-label','Show volumes in scene');eye.title='Show volumes in scene';
 eye.append(icon('M1.5 9s2.7-5.5 7.5-5.5S16.5 9 16.5 9s-2.7 5.5-7.5 5.5S1.5 9 1.5 9ZM9 11.2a2.2 2.2 0 1 0 0-4.4 2.2 2.2 0 0 0 0 4.4'));header.append(eye);
 const list=document.createElement('div');list.id='volume-list';list.className='list-group';
 const actions=document.createElement('div');actions.className='panel-actions single';actions.append(newVolume);
 panel.replaceChildren(actions,reason,status,header,list);
 // Form fields keep full accessible names; visible labels stay short to fit a 3-column grid.
 const form=document.createElement('form');form.id='volume-form';form.className='volume-form';form.noValidate=false;form.hidden=true;
 form.innerHTML='<div class="form-field"><label class="form-label" for="volume-title">Title</label><input class="form-control" id="volume-title" name="title" maxlength="120" required autocomplete="off"></div><div class="form-field"><label class="form-label" for="volume-description">Description</label><textarea class="form-control" id="volume-description" name="description" maxlength="10000" placeholder="Optional details"></textarea></div>';
 const groups=[['Size',[['width','W','Width','m'],['depth','D','Depth','m'],['height','H','Height','m']]],['Position',[['x','X','Center X','m'],['z','Z','Center Z','m'],['base','Base','Base elevation','m']]],['Rotation',[['yaw','','Rotation (degrees)','°']]]];
 for(const [legend,entries] of groups){
  const set=document.createElement('fieldset');set.className='volume-fieldset';const title=document.createElement('legend');title.className='form-label';title.textContent=legend;set.append(title);
  const grid=document.createElement('div');grid.className='volume-grid';
  for(const [name,short,full,unit] of entries){
   const field=document.createElement('label');field.className='unit-field';const caption=document.createElement('span');caption.className='unit-caption';caption.textContent=short;caption.hidden=!short;caption.setAttribute('aria-hidden','true');
   const input=document.createElement('input');input.className='form-control';input.type='number';input.inputMode='decimal';input.name=name;input.required=true;input.step='any';input.setAttribute('aria-label',full);
   input.min=['width','depth','height'].includes(name)?'.01':name==='yaw'?'-360':'-100000';input.max=name==='yaw'?'360':['width','depth','height'].includes(name)?'10000':'100000';
   input.addEventListener('input',()=>{delete input.dataset.exact;});
   const suffix=document.createElement('span');suffix.className='unit-suffix';suffix.textContent=unit;suffix.setAttribute('aria-hidden','true');
   field.append(caption,input,suffix);grid.append(field);
  }
  if(legend==='Rotation'){for(const delta of [-15,15]){const b=actionButton((delta>0?'+':'−')+'15°');b.setAttribute('aria-label',(delta>0?'Rotate +':'Rotate −')+'15 degrees');b.onclick=()=>{const input=form.elements.yaw;input.value=format(Number(input.value)+delta);delete input.dataset.exact;form.dispatchEvent(new Event('input'));};grid.append(b);}grid.classList.add('rotation');}
  set.append(grid);form.append(set);
 }
 const cancelButton=actionButton('Cancel',{id:'volume-cancel'}),saveButton=actionButton('Save volume',{variant:'primary',type:'submit'});
 const formActions=document.createElement('div');formActions.className='card-actions';formActions.append(cancelButton,saveButton);form.append(formActions);
 const $=id=>id==='volume-form'?form:document.getElementById(id);
 let cardMode=null,openedFromKeyboard=false;
 const format=n=>String(Number(Number(n).toFixed(4)));
 function message(text){status.textContent=text;}
 function alertInCard(text){if(!cardMode)return;inspector.element.querySelector('.card-alert')?.remove();const box=document.createElement('p');box.className='card-alert';box.setAttribute('role','alert');box.textContent=text;inspector.element.querySelector('.inspector-kind')?.after(box);}
 function editable(){return context?.editable && !pending;}
 function buttons(){newVolume.disabled=!editable();newVolume.setAttribute('aria-pressed',String(drawing));for(const b of form.querySelectorAll('button'))b.disabled=pending;reason.hidden=!!context?.editable;reason.textContent=context?.editable?'':'Select a local Home scene to create volumes.';}
 function mesh(volume,parent,selected=false,isDraft=false){
  // Volumes often stand on the floor, coplanar with the grid: offset the faces and lift the drawing by a few
  // millimetres (display only, the stored volume is unchanged) so the floor face and edges never z-fight.
  const box=new THREE.Mesh(new THREE.BoxGeometry(1,1,1),new THREE.MeshBasicMaterial({color:0x7c3aed,transparent:true,opacity:selected?.16:.08,depthWrite:false,side:THREE.DoubleSide,polygonOffset:true,polygonOffsetFactor:-2,polygonOffsetUnits:-2}));
  box.position.fromArray(volume.center);box.position.y+=.004;box.scale.fromArray(volume.size);box.rotation.y=volume.yaw;box.userData.volume=volume;parent.add(box);
  const edges=new THREE.EdgesGeometry(box.geometry),material=isDraft?new THREE.LineDashedMaterial({color:0x7c3aed,dashSize:.04,gapSize:.03}):new THREE.LineBasicMaterial({color:0x7c3aed,transparent:!selected,opacity:selected?1:.4});
  const outline=new THREE.LineSegments(edges,material);outline.position.copy(box.position);outline.scale.copy(box.scale);outline.rotation.copy(box.rotation);if(isDraft)outline.computeLineDistances();parent.add(outline);
  // Scene labels draw the name (hidden on the volume being edited: its card shows it); otherwise a sprite.
  const at=[volume.center[0],volume.center[1]+volume.size[1]/2+.08,volume.center[2]];
  if(labels){const anchor=new THREE.Object3D();anchor.position.fromArray(at);parent.add(anchor);labelItems.push({id:'volume:'+(isDraft&&!editing?'draft':volume.id),kind:'volume',anchor,text:volume.title||'New volume',volumeId:volume.id,hidden:selected});}
  else{const title=label(volume.title||'New volume');title.position.fromArray(at);parent.add(title);}
 }
 let labelItems=[];
 function draw(){labelItems=[];clear(group);for(const v of items)if(!(draft&&v.id===editing))mesh(v,group,v.id===editing);clear(preview);if(draft)mesh(draft,preview,true,!editing);renderer.domElement.dataset.semanticVolumeCount=String(items.length);labels?.sync('semantic-volumes',labelItems);}
 function focus(v){const distance=Math.max(3,Math.max(...v.size)*2.2),target=new THREE.Vector3(...v.center),position=target.clone().add(new THREE.Vector3(distance*.7,distance*.65,distance*.7));
  if(flyTo)flyTo(position,target);else{controls.target.copy(target);camera.position.copy(position);controls.update();}}
 const dimensions=v=>`${format(v.size[0].toFixed(1))} × ${format(v.size[2].toFixed(1))} m floor · ${format(v.size[1].toFixed(1))} m tall`;
 function showList(){
  list.replaceChildren();header.querySelector('.count-pill').textContent=String(items.length);
  const focused=list.contains(document.activeElement)?document.activeElement.dataset.volumeId:null;
  for(const v of items){
   const row=listRow({title:v.title,meta:(v.description||'').split('\n')[0]||dimensions(v),tile:icon('M2 5l7-3 7 3v8l-7 3-7-3ZM2 5l7 3 7-3M9 8v8',16),accent:'violet',onSelect:e=>{openedFromKeyboard=e?.detail===0;select(v);}});
   row.dataset.volumeId=v.id;if(v.id===editing)markSelected(row,'volumes',v.id);list.append(row);
  }
  if(!items.length){const p=document.createElement('p');p.className='empty-state';p.textContent='No semantic volumes in this scene.';list.append(p);}
  if(focused)list.querySelector(`[data-volume-id="${CSS.escape(focused)}"]`)?.focus();
 }
 function fill(v){form.elements.title.value=v.title;form.elements.description.value=v.description;const values={x:v.center[0],z:v.center[2],base:v.center[1]-v.size[1]/2,width:v.size[0],height:v.size[1],depth:v.size[2],yaw:v.yaw*180/Math.PI};for(const [name,n] of Object.entries(values)){form.elements[name].value=format(n);form.elements[name].dataset.exact=String(n);}}
 // Untouched fields keep their exact stored value; only edited fields use the displayed number.
 function values(){const n=name=>{const input=form.elements[name];return Number(input.dataset.exact??input.value);};return {id:editing||'',title:form.elements.title.value.trim(),description:form.elements.description.value,semantic_type:draft?.semantic_type||'region',center:[n('x'),n('base')+n('height')/2,n('z')],size:[n('width'),n('height'),n('depth')],yaw:n('yaw')*Math.PI/180};}
 // Inspector card: view, edit, create and delete-confirm.
 function renderCard(mode){
  cardMode=mode;const v=mode==='create'?draft:items.find(x=>x.id===editing)||draft;if(!v)return;
  const nodes=[];const chip=document.createElement('span');chip.className='inspector-kind';chip.textContent=mode==='create'?'New volume':mode==='edit'?'Edit volume':'Volume';nodes.push(chip);
  if(mode==='view'){
   // An editable volume is renamed in place: click the name, type, Enter or click away saves.
   if(context?.editable){const title=document.createElement('input');title.className='card-title-input';title.value=v.title;title.maxLength=120;title.setAttribute('aria-label','Volume name');title.title='Click to rename';
    const commit=()=>{const name=title.value.trim();if(!name||name===v.title||pending||!draft){title.value=v.title;return;}draft.title=name;form.elements.title.value=name;persist(false,true);};
    title.onkeydown=e=>{if(e.key==='Enter'){e.preventDefault();title.blur();}else if(e.key==='Escape'){e.preventDefault();e.stopPropagation();title.value=v.title;title.blur();}};
    title.onblur=commit;nodes.push(title);}
   else{const title=document.createElement('strong');title.textContent=v.title;nodes.push(title);}
   const summary=document.createElement('div');summary.className='inspector-position is-text';summary.textContent=`${dimensions(v)} · ${format((v.yaw*180/Math.PI).toFixed(0))}°`;nodes.push(summary);
   if(v.description){const d=document.createElement('p');d.className='card-description';d.textContent=v.description;nodes.push(d);}
   if(context?.editable){const hint=document.createElement('p');hint.className='card-hint';hint.textContent='Drag the handles to move, resize or rotate. Changes save when you let go.';nodes.push(hint);
    const row=document.createElement('div');row.className='card-actions';row.append(actionButton('Delete',{variant:'danger-ghost',onClick:confirmDelete}),actionButton('Edit',{onClick:()=>renderCard('edit')}));nodes.push(row);}
   else{const lock=document.createElement('p');lock.className='card-readonly';lock.append(icon(icons.lock,14),document.createTextNode('Read-only · Published volume'));nodes.push(lock);}
   form.hidden=true;nodes.push(form);
  }else{if(mode==='edit'){const hint=document.createElement('p');hint.className='card-hint';hint.textContent='Dragging the handles updates these values. Nothing changes until you save.';nodes.push(hint);}form.hidden=false;nodes.push(form);for(const input of form.querySelectorAll('input,textarea'))input.disabled=false;}
  inspector.open(nodes);
  if(mode==='view'&&openedFromKeyboard){const t=inspector.element.querySelector('.card-title,:scope>strong');if(t){t.tabIndex=-1;t.focus();}}
  if(mode!=='view')requestAnimationFrame(()=>form.elements.title.focus());
 }
 function confirmDelete(){
  const v=items.find(x=>x.id===editing);if(!v)return;cardMode='confirm';
  const row=inspector.element.querySelector('.card-actions');const box=document.createElement('div');box.className='card-confirm';box.setAttribute('role','group');box.setAttribute('aria-label','Confirm delete');
  const question=document.createElement('p');question.textContent=`Delete “${v.title}”? This can’t be undone.`;
  const keep=actionButton('Cancel',{onClick:()=>renderCard('view')}),remove=actionButton('Delete volume',{variant:'danger',onClick:()=>persist(true)});
  const buttonsRow=document.createElement('div');buttonsRow.className='card-actions';buttonsRow.append(keep,remove);box.append(question,buttonsRow);
  box.addEventListener('keydown',e=>{if(e.key==='Escape'){e.stopPropagation();renderCard('view');}});
  row.replaceWith(box);keep.focus();
 }
 function endDrawing(){stopResize();drawing=false;start=null;controls.enabled=true;renderer.domElement.style.cursor='';banner.hidden=true;tick();buttons();}
 // Resets editing state. `fromInspector` means the card was already closed by the app.
 function cancel(fromInspector=false){endDrawing();draft=null;editing=null;conflicted=false;form.hidden=true;const open=cardMode;cardMode=null;if(open&&!fromInspector)inspector.close();draw();showList();buttons();}
 function select(v){if(pending)return;cancel();openPanel('volumes');editing=v.id;draft=context?.editable?structuredClone(v):null;fill(v);focus(v);renderCard('view');message('');draw();showList();}
 function begin(at=[controls.target.x,0,controls.target.z],title=''){if(!editable())return;cancel();draft={id:'',title,description:'',center:[at[0],1,at[2]],size:[2,2,1.2],yaw:0};fill(draft);focus(draft);draw();renderCard('create');message('');}
 // Draw mode: drag the front edge on the ground; a banner explains and offers the keyboard path.
 const banner=document.createElement('div');banner.className='placement-banner';banner.setAttribute('role','status');banner.hidden=true;
 banner.innerHTML='<span>Drag on the floor to draw the front edge · Shift locks axis</span><kbd>Esc</kbd>';
 // The name is typed first, as soon as New volume is clicked; drawing the edge comes after.
 const nameField=document.createElement('input');nameField.className='placement-name';nameField.type='text';nameField.placeholder='Name this volume';nameField.setAttribute('aria-label','Volume name');nameField.maxLength=120;
 nameField.oninput=()=>{form.elements.title.value=nameField.value;if(draft)draft.title=nameField.value;};
 nameField.onkeydown=e=>{if(e.key==='Enter'){e.preventDefault();const name=nameField.value;cancel();begin(undefined,name);}};
 banner.append(nameField);
 banner.append(actionButton('Place a default box instead',{variant:'on-dark',onClick:()=>{const name=nameField.value;cancel();begin(undefined,name);}}),actionButton('Cancel',{variant:'on-dark',onClick:()=>{cancel();message('Creation cancelled.');newVolume.focus();}}));
 renderer.domElement.parentElement.append(banner);
 newVolume.onclick=()=>{if(!editable())return;if(drawing){cancel();message('Creation cancelled.');return;}cancel();draft={id:'',title:'',description:'',center:[controls.target.x,1,controls.target.z],size:[1,2,.6],yaw:0};fill(draft);drawing=true;start=null;controls.enabled=false;/* Drawing works on the floor from any angle, so the view stays where it is. */renderer.domElement.style.cursor='crosshair';banner.hidden=false;nameField.value='';requestAnimationFrame(()=>nameField.focus({preventScroll:true}));buttons();message('');};
 cancelButton.onclick=()=>{if(pending)return;if(cardMode==='edit'){const v=items.find(x=>x.id===editing);if(v){draft=structuredClone(v);fill(v);draw();}renderCard('view');message('');}else{cancel();message('Cancelled.');newVolume.focus();}};
 eye.onclick=()=>{const on=eye.getAttribute('aria-pressed')!=='true';eye.setAttribute('aria-pressed',String(on));group.visible=preview.visible=on;tick();};
 form.oninput=()=>{if(draft&&!pending){const v=values();if([...v.center,...v.size,v.yaw].every(Number.isFinite)&&v.size.every(n=>n>=.01)){draft=v;draw();}}};
 form.addEventListener('keydown',e=>{if(e.key==='Escape'&&!resize&&!drawing){e.preventDefault();e.stopPropagation();cancelButton.click();}});
 function url(id){return '/api/semantic-volumes'+(id?'/'+encodeURIComponent(id):'')+'?scene='+encodeURIComponent(context.map);}
 async function persist(remove=false,keepSelection=false){
  if(!editable()||!draft)return;
  if(conflicted){const text='This volume changed elsewhere while you were editing. Copy your edits, then reselect the volume to reload before saving.';message(text);alertInCard(text);return;}
  const selected=context,identifier=editing,body={frame:selected.frame,volume:remove?null:values()},before=new Set(items.map(v=>v.id));
  pending=true;buttons();endDrawing();message('Saving…');
  try {const response=await api(url(identifier),body,remove?'DELETE':identifier?'PATCH':'POST');if(context!==selected)return;items=response.items;signature=JSON.stringify(response);
   const saved=remove?null:(identifier?items.find(v=>v.id===identifier):items.find(v=>!before.has(v.id)))||null;
   if(saved){editing=saved.id;draft=structuredClone(saved);fill(saved);draw();showList();if(!(keepSelection&&cardMode==='edit'))renderCard('view');}else cancel();
   message(remove?'Volume deleted.':'Volume saved in this scene.');toast(remove?`Deleted “${body.volume?.title||'volume'}”`:'Volume saved');}
  catch(e){if(context===selected){message(e.message);alertInCard(e.message);}}
  finally{if(context===selected){pending=false;buttons();}}
 }
 form.onsubmit=e=>{e.preventDefault();if(form.reportValidity())persist();};
 function groundPoint(e){const rect=renderer.domElement.getBoundingClientRect(),ray=new THREE.Raycaster();ray.setFromCamera(new THREE.Vector2((e.clientX-rect.left)/rect.width*2-1,-(e.clientY-rect.top)/rect.height*2+1),camera);return ray.ray.intersectPlane(new THREE.Plane(new THREE.Vector3(0,1,0),-Number(form.elements.base.value)),new THREE.Vector3());}
 function footprint(p,shift=false){
  if(shift){if(Math.abs(p.x-start.x)>=Math.abs(p.z-start.z))p.z=start.z;else p.x=start.x;}
  const height=Number(form.elements.height.value),base=Number(form.elements.base.value),depth=Number(form.elements.depth.value),dx=p.x-start.x,dz=p.z-start.z,width=Math.hypot(dx,dz);if(width<.01)return;
  draft={...values(),center:[(start.x+p.x)/2+(dz/width)*depth/2,base+height/2,(start.z+p.z)/2-(dx/width)*depth/2],size:[width,height,depth],yaw:-Math.atan2(dz,dx)};fill(draft);draw();
 }
 const canvas=renderer.domElement;
 canvas.addEventListener('pointerdown',e=>{press=[e.clientX,e.clientY];if(!drawing||e.button!==0)return;e.stopImmediatePropagation();e.preventDefault();start=groundPoint(e);if(start)canvas.setPointerCapture(e.pointerId);},true);
 canvas.addEventListener('pointermove',e=>{if(!drawing)return;e.stopImmediatePropagation();if(start){const p=groundPoint(e);if(p)footprint(p,e.shiftKey);}},true);
 canvas.addEventListener('pointerup',e=>{if(drawing){e.stopImmediatePropagation();e.preventDefault();if(start){const p=groundPoint(e);if(p)footprint(p,e.shiftKey);if(canvas.hasPointerCapture(e.pointerId))canvas.releasePointerCapture(e.pointerId);if(!p||p.distanceTo(start)<.3){start=null;message('Drag a front edge at least 30 cm long.');return;}endDrawing();message('Front edge ready. Add a title and use the handles to adjust the volume.');focus(draft);renderCard('create');}return;}
  if(pending||e.button!==0||!press||Math.hypot(e.clientX-press[0],e.clientY-press[1])>4||!group.visible)return;
  const r=canvas.getBoundingClientRect(),ray=new THREE.Raycaster();ray.setFromCamera(new THREE.Vector2((e.clientX-r.left)/r.width*2-1,-(e.clientY-r.top)/r.height*2+1),camera);
  // A volume's own label selects it; any other label (notes are drawn through translucent volumes) and the
  // priority objects go to the normal scene picker before this capture-phase listener claims the click.
  const tagged=priorityHit(e),labelled=tagged?.kind==='volume'&&items.find(v=>v.id===tagged.volumeId);
  if(labelled){e.semanticVolume=true;select(labelled);return;}
  if(tagged&&tagged.kind!=='volume')return;
  const priority=priorityPickObjects().filter(o=>{for(let p=o;p;p=p.parent){if(!p.visible)return false;}return true;});
  if(ray.intersectObjects(priority).length)return;
  const hit=ray.intersectObjects([...group.children,...preview.children]).find(h=>h.object.userData.volume);if(hit){e.semanticVolume=true;select(hit.object.userData.volume);}
 },true);
 canvas.addEventListener('pointercancel',()=>{if(drawing){cancel();message('Creation cancelled.');}});
 document.addEventListener('keydown',e=>{if(e.key!=='Escape')return;if(resize){e.stopImmediatePropagation();e.preventDefault();stopResize(true);message('Edit cancelled.');}else if(drawing){e.stopImmediatePropagation();e.preventDefault();cancel();message('Creation cancelled.');newVolume.focus();}},true);
 return {tick,beginAt:point=>begin(point),closed(){if(cardMode)cancel(true);},pick(ray){if(drawing||resize||!group.visible)return null;return ray.intersectObjects([...group.children,...preview.children]).find(h=>h.object.userData.volume);},update(map,data){const editable=map.startsWith('local:')||map==='demo';if(!context||context.map!==map||context.frame!==data?.frame){context={map,frame:data?.frame,editable};pending=false;items=[];cancel();signature='';message('');}const collection=data?.semantic_volumes;const next=JSON.stringify(collection||null);if(next!==signature){signature=next;const previous=items.find(v=>v.id===editing);items=collection?.frame===data.frame?collection.items:[];const remote=items.find(v=>v.id===editing);if(editing&&!pending&&!resize){if(!remote){cancel();message('This volume was deleted elsewhere.');}else if(draft&&previous){const merged=mergeVolumeDraft(previous,values(),remote);draft=merged.draft;conflicted=conflicted||merged.conflicts.length>0;fill(draft);if(conflicted)message('Conflicting edits: '+merged.conflicts.join(', ')+'. Copy your edits and reselect the volume before saving.');}else if(!draft)fill(remote);}draw();showList();}buttons();},reset(){context=null;items=[];pending=false;cancel();buttons();message('Loading selected scene…');}};
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
