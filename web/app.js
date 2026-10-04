import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { createArOverlay } from './ar.js';
const $=id=>document.getElementById(id);
// Organize existing live controls into the mockup's rail and exclusive panels.
const sidebar=document.querySelector('aside'),shell=document.querySelector('.shell');
const rail=document.createElement('nav');rail.className='icon-rail';rail.setAttribute('aria-label','Workspace panels');shell.prepend(rail);
const panelNames={profile:'Profile',maps:'Maps',notes:'Notes',fleet:'Devices',settings:'Settings'};
const iconPaths={profile:'M9 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6M3 16c0-7 12-7 12 0',maps:'M2 4l5-2 4 2 5-2v12l-5 2-4-2-5 2ZM7 2v12M11 4v12',notes:'M4 2h10v14H4ZM6 6h6M6 9h6M6 12h4',fleet:'M4 6h10v9H4ZM6 2h6v4M7 9h.1M11 9h.1M7 12h4M1 10h3M14 10h3',settings:'M9 1v3M9 14v3M1 9h3M14 9h3M3 3l2 2M13 13l2 2M3 15l2-2M13 5l2-2M12 9a3 3 0 1 0-6 0 3 3 0 0 0 6 0'};
const panelBodies={};let activePanel='profile';
for(const [id,name] of Object.entries(panelNames)){
 const button=document.createElement('button');button.type='button';button.dataset.panel=id;button.title=name;button.setAttribute('aria-label',name);button.setAttribute('aria-controls','panel-'+id);
 button.innerHTML=`<svg width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"><path d="${iconPaths[id]}"/></svg>`;
 button.onclick=()=>openPanel(activePanel===id?null:id);rail.append(button);
 const panel=document.createElement('section');panel.id='panel-'+id;panel.className='workspace-panel';panel.setAttribute('aria-label',name);panel.innerHTML=`<div class="panel-heading">${name==='Maps'?'Scenegraphs':name}<button aria-label="Close ${name} panel">×</button></div><div class="panel-body"></div>`;panel.querySelector('button').onclick=()=>openPanel(null);panelBodies[id]=panel.lastElementChild;sidebar.append(panel);
}
function moveTo(panel,selector){document.querySelectorAll(selector).forEach(el=>panelBodies[panel].append(el));}
moveTo('profile','aside > .eyebrow, aside > h1, aside > .intro, aside > .network-status:not(#runner-card):not(#mapping-card)');
moveTo('maps','#map-select, #mapping-card, #domain-info, .sidebar-footer');
moveTo('fleet','#runner-card, #sensor-select, #sensor-catalog-status');
$('runner-card').hidden=true;
$('sensor-catalog-status').hidden=true;
moveTo('settings','#demo-controls, aside > .section-label, aside > .layer, #robot-volume-status');
sidebar.querySelectorAll(':scope > .map-label').forEach(el=>el.remove());
function openPanel(id){activePanel=id;shell.classList.toggle('panel-collapsed',!id);for(const key of Object.keys(panelNames)){$('panel-'+key).hidden=key!==id;rail.querySelector(`[data-panel="${key}"]`).setAttribute('aria-expanded',String(key===id));} }
openPanel('profile');
const themeButton=document.createElement('button');themeButton.id='theme-toggle';themeButton.textContent='☼';themeButton.title='Switch to light theme';themeButton.setAttribute('aria-label',themeButton.title);rail.append(themeButton);
const mapsList=document.createElement('div');mapsList.id='scenegraph-list';panelBodies.maps.prepend(mapsList);$('map-select').hidden=true;
const fleetList=document.createElement('div');fleetList.id='fleet-sensors';panelBodies.fleet.append(fleetList);$('sensor-select').hidden=true;
const notesBody=panelBodies.notes;
let fleetNetwork=null,fleetSignature='';
const scene=new THREE.Scene();scene.background=new THREE.Color('#0d0d0f');scene.fog=new THREE.Fog('#0d0d0f',10,26);
const camera=new THREE.PerspectiveCamera(42,1,.01,100);
const renderer=new THREE.WebGLRenderer({antialias:true,preserveDrawingBuffer:true});renderer.setPixelRatio(Math.min(devicePixelRatio,2));renderer.shadowMap.enabled=true;renderer.shadowMap.type=THREE.PCFSoftShadowMap;renderer.outputColorSpace=THREE.SRGBColorSpace;renderer.toneMapping=THREE.ACESFilmicToneMapping;renderer.toneMappingExposure=1.1;
$('canvas').append(renderer.domElement);renderer.domElement.setAttribute('aria-label','3D voxel map');
const controls=new OrbitControls(camera,renderer.domElement);controls.enableDamping=true;controls.minDistance=1.2;controls.maxDistance=60;controls.maxPolarAngle=Math.PI*.49;
let activeMap='demo',current=null,busy=false,refreshing=false,lastScene=null,selectable=[],selectedNoteId=null,deletingNoteId=null;
function isRunner(map=activeMap){return map==='runner'||map.startsWith('runner:');}
function runnerUrl(endpoint,map=activeMap){return endpoint+(map.startsWith('runner:')?'?peer='+encodeURIComponent(map.slice(7)):'');}
const runnerNames=new Map(),peerNames=new Map();
let primaryRunnerPeer=null;
function runnerDisplay(){return runnerNames.get(activeMap)||'Galbot';}
async function runnerCatalog(){
 try {
  const data=await api('/api/scene-sources');
  const local=document.createElement('optgroup');local.label='Local';
  for(const map of data.local||[])local.append(new Option(map.name,map.id));
  const network=document.createElement('optgroup');network.label='Network';
  runnerNames.clear();peerNames.clear();primaryRunnerPeer=null;
  for(const runner of data.runners||[]) {
   if(!runner.available||!['connected','stale'].includes(runner.status))continue;
   if(runner.primary)primaryRunnerPeer=runner.peer_id;
   const value=runner.primary?'runner':'runner:'+runner.peer_id;
   runnerNames.set(value,runner.name);peerNames.set(runner.peer_id,runner.name);
   network.append(new Option(runner.name||runner.peer_id,value));
  }
  // A primary scene may be present without a named runner configuration.
  if(data.available&&['connected','stale'].includes(data.status)&&!Array.from(network.children).some(o=>o.value==='runner')){
   network.append(new Option(data.peer_id||'Network scenegraph','runner'));
  }
  const select=$('map-select');select.replaceChildren(local,network);
  if(Array.from(select.options).some(o=>o.value===activeMap))select.value=activeMap;
  else if(select.options.length){select.value=select.options[0].value;select.dispatchEvent(new Event('change'));}
 }catch{}
 setTimeout(runnerCatalog,5000);
}

function domainCenter(){return new THREE.Box3().setFromPoints((current?.portals||[]).map(p=>new THREE.Vector3(...p.pose_in_map.translation))).getCenter(new THREE.Vector3());}
function fit(){if(current?.portals.length){const c=domainCenter();const box=new THREE.Box3().setFromPoints(current.portals.map(p=>new THREE.Vector3(...p.pose_in_map.translation)));const size=box.getSize(new THREE.Vector3());const distance=Math.max(size.x/Math.max(camera.aspect,.2),size.z)*1.65+3;camera.position.copy(c).add(new THREE.Vector3(0,distance,distance*.25));controls.target.copy(c);}else{camera.position.set(3.6,3.5,4.6);controls.target.set(0,.5,0);}controls.update();}
fit();$('demo-view').onclick=()=>{camera.position.set(-.5,4.5,5.5);controls.target.set(-3.7,.5,2.1);controls.update();};$('fit').onclick=fit;$('top').onclick=()=>{fit();const c=controls.target.clone();const height=camera.position.distanceTo(c);camera.position.copy(c).add(new THREE.Vector3(0,height,.001));controls.update();};
scene.add(new THREE.HemisphereLight(0xffffff,0x1a2438,1.7));
const sun=new THREE.DirectionalLight(0xffffff,2.4);sun.position.set(-3,7,4);sun.castShadow=true;sun.shadow.mapSize.set(2048,2048);Object.assign(sun.shadow.camera,{left:-5,right:5,top:5,bottom:-5,near:.1,far:20});sun.shadow.bias=-.0005;scene.add(sun);
const ground=new THREE.Mesh(new THREE.PlaneGeometry(200,200),new THREE.MeshStandardMaterial({color:'#101014',roughness:1}));ground.rotation.x=-Math.PI/2;ground.position.y=-.15;ground.receiveShadow=true;scene.add(ground);
const grid=new THREE.GridHelper(6,30,0x4f8ef7,0x24242b);grid.position.y=-.14;grid.material.transparent=true;grid.material.opacity=.28;scene.add(grid);
const groups=Object.fromEntries(['voxels','portals','objects','rays','notes'].map(k=>[k,new THREE.Group()]));Object.values(groups).forEach(g=>scene.add(g));
for(const input of document.querySelectorAll('[data-layer]'))input.onchange=()=>{groups[input.dataset.layer].visible=input.checked;};

const boxGeometry=new THREE.BoxGeometry(1,1,1), cubeMaterial=new THREE.MeshStandardMaterial({roughness:.82,metalness:0});
function clear(group){for(const child of [...group.children]){group.remove(child);if(child.isInstancedMesh)child.dispose();child.traverse(o=>{if(o.geometry&&o.geometry!==boxGeometry)o.geometry.dispose();if(o.material&&o.material!==cubeMaterial){for(const m of Array.isArray(o.material)?o.material:[o.material]){m.map?.dispose();m.dispose();}}});}}
function label(text,color='#36594a'){
 const c=document.createElement('canvas');c.width=384;c.height=96;const ctx=c.getContext('2d');ctx.fillStyle='#fcfdf6';ctx.beginPath();ctx.roundRect(4,8,376,76,18);ctx.fill();ctx.font='500 30px system-ui';ctx.textAlign='center';ctx.fillStyle=color;ctx.fillText(text,192,56,356);const tex=new THREE.CanvasTexture(c);tex.colorSpace=THREE.SRGBColorSpace;const sprite=new THREE.Sprite(new THREE.SpriteMaterial({map:tex,depthTest:false}));sprite.scale.set(.65,.1625,1);return sprite;
}
function build(data){
 const first=current===null||current?.frame!==data.frame;data.receivedAt=performance.now();current=data;selectable=[];Object.values(groups).forEach(clear);
 const voxels=new THREE.InstancedMesh(boxGeometry,cubeMaterial,data.voxels.length);voxels.castShadow=true;voxels.receiveShadow=true;
 const matrix=new THREE.Matrix4(),color=new THREE.Color();
 data.voxels.forEach((v,i)=>{const scale=data.voxel_size*.95;matrix.compose(new THREE.Vector3(...v.center),new THREE.Quaternion(),new THREE.Vector3(scale,scale,scale));voxels.setMatrixAt(i,matrix);const furnishing=data.synthetic_layout?.boxes.findLast(b=>v.center.every((n,j)=>n>=b.min[j]-.051&&n<=b.max[j]+.051));if(v.linear_rgb){color.setRGB(...v.linear_rgb,THREE.LinearSRGBColorSpace);}else color.set(furnishing?.color||(v.center[1]<0?'#c6cbbb':'#a7b4ac'));voxels.setColorAt(i,color);});
 voxels.userData.kind='voxels';groups.voxels.add(voxels);selectable.push(voxels);
 for(const p of data.portals){
  const imported=p.anchor_id.startsWith('home:');const blue=data.local_map&&!imported;const code=imported?p.anchor_id.slice(5):p.anchor_id;
  const g=new THREE.Group();g.position.fromArray(p.pose_in_map.translation);const [w,x,y,z]=p.pose_in_map.rotation_wxyz;g.quaternion.set(x,y,z,w);
  const plate=new THREE.Mesh(new THREE.PlaneGeometry(p.side_length_m,p.side_length_m),new THREE.MeshBasicMaterial({color:blue?'#2477ec':'#e53935',wireframe:imported&&data.portals.some(o=>o.anchor_id===code),side:THREE.DoubleSide,depthTest:false,depthWrite:false}));plate.renderOrder=61;plate.userData={kind:'portal',portal:p};g.add(plate);selectable.push(plate);
  groups.portals.add(g);
 }
 for(const note of data.notes||[]){
  const tag=label(noteTitle(note),'#a12522');tag.position.fromArray(note.position);tag.material.depthWrite=false;tag.material.sizeAttenuation=false;tag.scale.set(.16,.04,1);tag.renderOrder=80;
  tag.userData={kind:'note',note};groups.notes.add(tag);selectable.push(tag);
 }
 $('canvas').dataset.noteCount=String((data.notes||[]).length);
 for(const o of data.objects){
  const point=new THREE.Mesh(new THREE.SphereGeometry(.04,20,14),new THREE.MeshStandardMaterial({color:'#dd895d',emissive:'#65361b',emissiveIntensity:.15}));point.position.fromArray(o.position);point.userData={kind:'object',object:o};groups.objects.add(point);selectable.push(point);
  const ring=new THREE.Mesh(new THREE.TorusGeometry(.13,.009,8,48),new THREE.MeshBasicMaterial({color:'#d79367'}));ring.rotation.x=-Math.PI/2;ring.position.copy(point.position);ring.position.y+=.008;groups.objects.add(ring);
  const tag=label(o.label,'#a56845');tag.position.copy(point.position).add(new THREE.Vector3(0,.32,0));groups.objects.add(tag);
  const geo=new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(...o.ray_origin),new THREE.Vector3(...o.position)]);const line=new THREE.Line(geo,new THREE.LineDashedMaterial({color:'#7898bd',dashSize:.05,gapSize:.035}));line.computeLineDistances();groups.rays.add(line);
 }
 if(data.integrated_observations){
  for(const [peer,p] of Object.entries(data.sensor_positions||{})){
   if(!data.sources[peer])continue;
   const marker=new THREE.Mesh(new THREE.OctahedronGeometry(.045),new THREE.MeshStandardMaterial({color:'#7f9aba'}));marker.position.fromArray(p);groups.rays.add(marker);
  }
 }
 $('empty').hidden=data.portals.length>0||data.voxels.length>0;if(activeMap==='home-live')$('empty').textContent='Waiting for Home to observe UCOF1J40N7O';else $('empty').textContent='No geometry yet';$('voxel-count').textContent=data.voxels.length.toLocaleString();$('observation-count').textContent=data.integrated_observations;$('object-count').textContent=data.objects.length;
 for(const [peer,id] of [['iphone','phone-state'],['bracketbot','bot-state']]){const n=data.sources[peer];$(id).textContent=n?'Contributed':'Ready';$(id).classList.toggle('active',Boolean(n));}
 $('load').innerHTML=data.integrated_observations?'Reload test contributions <span>↗</span>':'Load two test peers <span>↗</span>';
 $('project').disabled=busy||!data.voxels.length;$('replay').disabled=busy||!data.sources.iphone;
 $('updated').textContent=`${data.integrated_observations} accepted observations`;
 if(selectedNoteId){const note=(data.notes||[]).find(n=>n.id===selectedNoteId);if(note){if($('selection').dataset.noteId!==note.id)showNoteSelection(note);}else{selectedNoteId=null;$('selection').hidden=true;}}else $('selection').hidden=true;if(!selectedNoteId&&data.objects.length){const o=data.objects[0];showSelection(o.label,`[${o.position.map(v=>v.toFixed(2)).join(', ')}] m`,'Projected onto occupied voxel surface');}
 updateMapUI();if(first)fit();
}
function focusNote(note){const p=new THREE.Vector3(...note.position);controls.target.copy(p);camera.position.copy(p).add(new THREE.Vector3(.5,.4,2));controls.update();showNoteSelection(note);}
function showNoteSelection(note){
 showSelection(noteTitle(note),`[${note.position.map(v=>v.toFixed(2)).join(', ')}] m`,note.phone_pose?'Live phone position':note.anchor_surface?'Placed in Home':`${note.height_m} m above ${note.anchor_robot?note.text+' base':note.anchor_portal} · Home up axis`);
 selectedNoteId=note.id;$('selection').dataset.noteId=note.id;
 if(note.details?.description){const description=document.createElement('p');description.textContent=note.details.description;$('selection').append(description);}
 if(note.details?.photo)$('selection').append(notePhoto(note));
 if(note.anchor_robot||note.phone_pose||isRunner()||activeMap==='domain')return;
 const button=document.createElement('button');button.className='delete-note';button.textContent='Delete annotation';button.disabled=deletingNoteId===note.id;
 const status=document.createElement('div');status.setAttribute('role','status');
 button.onclick=async()=>{
  if(deletingNoteId)return;deletingNoteId=note.id;button.disabled=true;status.textContent='Deleting…';
  try{await api('/api/notes/'+encodeURIComponent(note.id),undefined,'DELETE');
   if(selectedNoteId===note.id){selectedNoteId=null;$('selection').hidden=true;}
   hideAr();lastScene=null;await refresh();
  }catch(e){status.textContent=e.message;button.disabled=false;}
  finally{deletingNoteId=null;}
 };
 $('selection').append(button,status);
}
function focusPortal(p){const center=new THREE.Vector3(...p.pose_in_map.translation);camera.position.copy(center).add(new THREE.Vector3(.35,.65,.65));controls.target.copy(center);controls.update();showSelection(p.anchor_id,`[${p.pose_in_map.translation.map(v=>v.toFixed(4)).join(', ')}] m`,`${Math.round(p.side_length_m*100)} cm · ${p.anchor_id.startsWith('home:')?'Home reference aligned via measured portal':current?.local_map?'measured camera observation':'stored height, not mounting height'}`);}
function updateMapUI(){
 const local=Boolean(current?.local_map)||((isRunner()||activeMap==='home-live')&&!current);
 $('frame-info').textContent=activeMap==='home-live'?'Live origin: UCOF1J40N7O · shared Home frame':local?'Local origin: robot boot pose · fixed frame': activeMap==='domain'?'Stored domain frame · OpenGL · Y up · meters':'Home origin: UCOF1J40N7O · OpenGL · Y up · meters';
 $('portal-info').textContent=activeMap==='home-live'?'Blue: measured portals. Each robot calibrates through its own observed canonical portal.':local?'Blue: camera observations. Red: Home references aligned through a shared portal. The robot boot origin stays fixed.':'Stored Y = 0 for every portal. Actual mounting heights are unverified.';
 $('portal-swatch').style.background=local?'#2477ec':'#e53935';
 const domain=activeMap==='domain';$('domain-info').hidden=false;$('demo-controls').hidden=domain||!current?.demo;
 $('scene-title').textContent=activeMap==='home-live'?'Home live map':isRunner()?runnerDisplay()+' scenegraph':domain?'13 stored domain portals':(current?.demo?'Domain + synthetic scene':'Canonical portals');
 $('map-authority').textContent=isRunner()?runnerDisplay()+' mapper':'Home Server';
 $('scene-badge').textContent=activeMap==='home-live'?'HOME LIVE MAP':isRunner()?'GALBOT SCENEGRAPH':domain?'STORED DOMAIN POSES':(current?.demo?'DOMAIN + SYNTHETIC SCENE':'CANONICAL PORTALS');
 $('scene-note').textContent=activeMap==='home-live'?'Mapped on Home from both robots’ raw observations, each calibrated through known portal poses. UCOF1J40N7O defines the fixed origin.':local?(current?.local_mapping?.alignment_to_home?`Home references aligned via ${current.local_mapping.alignment_to_home.anchor}. Updated from its latest observed pose.`:'Robot-space map. Waiting for a shared portal to align Home references.'):domain?'Supplied poses and sizes. Grid is a coordinate reference, not a measured floor.':(current?.demo?'Illustrative L-shaped workspace fitted to the marker footprint. Furniture and walls are synthetic; stored portal poses are unchanged.':'Canonical portal record with supplied poses and sizes. Live geometry is in Home live map.');
 document.querySelector('.flow').hidden=domain||!current?.demo;
 $('demo-view').hidden=!current?.demo;
 document.querySelector('#inspect-snapshot').href=document.querySelector('.export').href=activeMap==='home-live'?'/api/home-live-snapshot':isRunner()?runnerUrl('/api/runner-snapshot'):domain?'/api/domain-snapshot':'/api/snapshots';
 ground.visible=Boolean(current?.demo);scene.fog=null;grid.scale.setScalar(3);grid.position.x=local?0:-5;grid.position.z=local?0:3;
 const list=$('portal-list');list.replaceChildren();{for(const p of (current?.portals||[]).filter(p=>p.anchor_id.length===11||p.anchor_id.startsWith('home:'))){const button=document.createElement('button');button.className='portal-row';button.textContent=p.anchor_id;const size=document.createElement('span');size.textContent=`${Math.round(p.side_length_m*100)} cm`;button.append(size);button.onclick=()=>focusPortal(p);list.append(button);}}
}
$('map-select').onchange=async()=>{activeMap=$('map-select').value;selectedNoteId=null;lastScene=null;current=null;hideRobots();selectable=[];Object.values(groups).forEach(clear);$('selection').hidden=true;updateMapUI();$('scene-title').textContent='Loading selected scene…';$('voxel-count').textContent='—';$('observation-count').textContent='—';$('object-count').textContent='—';try{await refresh();}catch{$('scene-title').textContent=isRunner()?'Waiting for Galbot scenegraph':'Scene unavailable';}};
function showSelection(title,body,note){selectedNoteId=null;const el=$('selection');delete el.dataset.noteId;el.replaceChildren();const strong=document.createElement('strong');strong.textContent=title;const line=document.createElement('div');line.textContent=body;const small=document.createElement('small');small.textContent=note;el.append(strong,line,small);el.hidden=false;}
let pointerStart;
renderer.domElement.addEventListener('pointerdown',e=>{pointerStart=[e.clientX,e.clientY];});
renderer.domElement.addEventListener('pointerup',e=>{
 if(e.button!==0)return;
 if(!current||!pointerStart||Math.hypot(e.clientX-pointerStart[0],e.clientY-pointerStart[1])>4)return;
 const r=renderer.domElement.getBoundingClientRect(), ray=new THREE.Raycaster();ray.setFromCamera(new THREE.Vector2((e.clientX-r.left)/r.width*2-1,-(e.clientY-r.top)/r.height*2+1),camera);
 const hits=ray.intersectObjects(selectable.filter(o=>{for(let p=o;p;p=p.parent){if(!p.visible)return false;}return true;}));if(!hits.length){selectedNoteId=null;$('selection').hidden=true;return;}const h=hits.find(h=>h.object.userData.kind==='note')||hits.find(h=>h.object.userData.kind==='portal')||hits[0],d=h.object.userData;
 if(d.kind==='voxels'){const v=current.voxels[h.instanceId];showSelection('Occupied voxel',`[${v.center.map(n=>n.toFixed(2)).join(', ')}] m`,`Evidence ${v.evidence} · 10 cm cell`);}
 if(d.kind==='note')focusNote(d.note);
 if(d.kind==='object'){const o=d.object;showSelection(o.label,`[${o.position.map(n=>n.toFixed(2)).join(', ')}] m`,`Source: ${o.source_peer}`);}
 if(d.kind==='portal'&&(d.portal.anchor_id.length===11||d.portal.anchor_id.startsWith('home:'))){focusPortal(d.portal);return;}
 if(d.kind==='portal')showSelection(`Portal ${d.portal.payload.split(':').at(-1)}`,`${d.portal.side_length_m.toFixed(2)} m encoded size`,'Manually supplied · no backend lookup');
});
async function api(path,body,method){const r=await fetch(path,body?{method:method||'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}:method?{method}:undefined);const text=await r.text();let result;try{result=JSON.parse(text);}catch{throw new Error(text);}if(!r.ok)throw new Error(result.error||text);return result;}
async function refresh(){if(refreshing)return;refreshing=true;refreshPeer();refreshMapping();try{const selected=activeMap;const data=await api(selected==='home-live'?'/api/home-live-scene':isRunner(selected)?runnerUrl('/api/runner-scene',selected):selected==='domain'?'/api/domain-scene':'/api/scene');if(selected!==activeMap)return;const content=JSON.stringify({...data,remote_status:undefined});if(content!==lastScene){lastScene=content;build(data);}else {data.receivedAt=performance.now();current=data;}$('connection').textContent='Home Server online';}catch(e){$('connection').textContent=isRunner()?'Runner scene unavailable':'Connection lost';throw e;}finally{refreshing=false;}}
async function action(fn){if(busy)return;busy=true;document.querySelectorAll('#load,#project,#replay').forEach(b=>b.disabled=true);$('notice').classList.remove('error');$('notice').textContent='Integrating…';try{await fn();await refresh();}catch(e){$('notice').textContent=e.message;$('notice').classList.add('error');}finally{busy=false;$('load').disabled=false;$('project').disabled=!current?.voxels.length;$('replay').disabled=!current?.sources.iphone;}}
$('load').onclick=()=>action(async()=>{const f=await api('/api/fixtures');for(const c of f.contributions)await api('/api/contributions',c);$('notice').textContent='Two scans aligned through Portal A. Ready to project a detection.';});
$('project').onclick=()=>action(async()=>{const f=await api('/api/fixtures');const result=await api('/api/detections',f.detection);$('notice').textContent=result.applied?'Book placed at the first occupied voxel hit.':'This detection is already placed. No duplicate object created.';});
$('replay').onclick=()=>action(async()=>{const f=await api('/api/fixtures');const r=await api('/api/contributions',f.contributions[0]);$('notice').textContent=r.applied?'Contribution accepted.':'Replay ignored. Voxel evidence and object count are unchanged.';});
new ResizeObserver(()=>{const {width,height}=$('canvas').getBoundingClientRect();renderer.setSize(width,height);camera.aspect=width/height;camera.updateProjectionMatrix();}).observe($('canvas'));
renderer.setAnimationLoop(()=>{controls.update();renderer.render(scene,camera);});
refresh().catch(e=>{$('notice').textContent=e.message;$('notice').classList.add('error');});setInterval(()=>{if(!busy)refresh().catch(()=>{});},500);

async function refreshPeer(){try{const peer=await api('/api/peer');$('p2p-status').textContent=peer.enabled?'P2P · '+peer.status:'P2P disabled';$('p2p-detail').textContent=peer.connection?peer.connection.peer_id:'Configure a peer to join the network.';$('pairing-download').hidden=peer.status!=='Ready';fleetNetwork=peer;renderFleetRows();}catch{$('p2p-status').textContent='P2P unavailable';$('pairing-download').hidden=true;fleetNetwork=null;renderFleetRows();}}

// Dynamic telemetry is independent of scene updates and synthetic peer scans.
const robots=new Map(),poseUpdates=new Map();
const poseColors=['#e49b32','#985cdb','#219b96'];
let runner=null,runnerBusy=false,lastRunnerReply=0;
function hideRobots(){for(const glyph of robots.values())glyph.group.visible=false;}
function glyphFor(key,name){
 let glyph=robots.get(key);
 if(!glyph){
  const group=new THREE.Group();scene.add(group);
  const color=poseColors[robots.size%poseColors.length];
  const material=new THREE.MeshStandardMaterial({color,transparent:true,opacity:.82,roughness:.6,depthTest:false});
  // Robot-local ROS body axes: +X forward, +Y left, +Z up; illustrative pose glyph.
  const body=new THREE.Mesh(new THREE.BoxGeometry(.42,.32,.18),material);body.position.z=.11;body.renderOrder=5;group.add(body);
  const arrow=new THREE.ArrowHelper(new THREE.Vector3(1,0,0),new THREE.Vector3(0,0,.25),.8,color,.2,.12);arrow.traverse(o=>{if(o.material){o.material.depthTest=false;o.renderOrder=6;}});group.add(arrow);
  glyph={group,material,arrow,color};robots.set(key,glyph);
 }
 if(glyph.name!==name){
  if(glyph.tag){glyph.group.remove(glyph.tag);glyph.tag.material.map.dispose();glyph.tag.material.dispose();}
  glyph.tag=label(name,glyph.color);glyph.tag.position.set(0,0,.6);glyph.group.add(glyph.tag);glyph.name=name;
 }
 return glyph;
}
function focusPose(name,state){
 if(!state.pose)return;
 const target=new THREE.Vector3(...state.pose.translation).add(new THREE.Vector3(0,.3,0));camera.position.copy(target).add(new THREE.Vector3(2,2.4,2.7));controls.target.copy(target);controls.update();
 showSelection(name+' · '+state.status,`[${state.pose.translation.map(n=>n.toFixed(3)).join(', ')}] m`,'Calibrated robot base · arrow points forward · illustrative pose glyph');
}
function renderRunners(entries){
 const keys=new Set(entries.map(e=>e.key));
 for(const [key,glyph] of robots)if(!keys.has(key)){scene.remove(glyph.group);clear(glyph.group);robots.delete(key);poseUpdates.delete(key);}
 const list=$('runner-list');
 const signature=JSON.stringify(entries.map(e=>[e.key,e.name]));
 if(list.dataset.signature!==signature){
  list.replaceChildren();list.dataset.signature=signature;
  for(const entry of entries){const row=document.createElement('div');row.className='runner-pose-row';row.dataset.peer=entry.key;
   const title=document.createElement('strong'),position=document.createElement('small'),button=document.createElement('button');button.className='text-button';button.textContent='Focus '+entry.name+' ↗';row.append(title,position,button);list.append(row);}
 }
 entries.forEach((entry,i)=>{
  const glyph=glyphFor(entry.key,entry.name),state=entry.state;
  glyph.tag.visible=activeMap!=='home-live';
  glyph.state=state;glyph.group.visible=Boolean(state.pose)&&$('runner-layer').checked;
  if(state.pose){glyph.group.position.fromArray(state.pose.translation);glyph.group.quaternion.fromArray(state.pose.rotation_xyzw);}
  glyph.material.color.set(state.status==='live'?glyph.color:'#84908f');glyph.material.opacity=state.status==='live'?.82:.35;glyph.arrow.setColor(state.status==='live'?glyph.color:'#84908f');
  const row=list.children[i];row.dataset.status=state.status;row.dataset.visible=String(glyph.group.visible);
  row.dataset.position=state.pose?JSON.stringify(glyph.group.position.toArray()):'';row.dataset.rotation=state.pose?JSON.stringify(glyph.group.quaternion.toArray()):'';
  row.children[0].textContent=entry.name+' · '+state.status;row.children[0].style.color=state.status==='live'?glyph.color:'#84908f';
  row.children[1].textContent=state.pose?`[${state.pose.translation.map(n=>n.toFixed(3)).join(', ')}] m${state.status==='stale'?' · last known':''}`:'Waiting for calibrated pose';
  row.children[2].disabled=!state.pose;row.children[2].onclick=()=>focusPose(entry.name,state);
 });
 list.hidden=entries.length<2;
 $('runner-card').dataset.poseCount=String(entries.filter(e=>e.state.pose).length);
}
function showRunner(state){
 runner=state;renderRunners([{key:'selected',name:runnerDisplay(),state}]);
 $('runner-status').textContent=runnerDisplay()+' · '+state.status;
 $('runner-card').dataset.status=state.status;
 $('runner-position').textContent=state.pose?`[${state.pose.translation.map(n=>n.toFixed(3)).join(', ')}] m${state.status==='stale'?' · last known':''}`:'No calibrated pose available';
 $('runner-detail').textContent=state.status==='disabled'?'Configure a runner peer to show its pose.':state.detail||'';
 $('focus-runner').textContent='Focus Galbot ↗';$('focus-runner').disabled=!state.pose;
 $('focus-runner').onclick=()=>focusPose(runnerDisplay(),state);
 const glyph=robots.get('selected');
 $('runner-card').dataset.position=state.pose?JSON.stringify(glyph.group.position.toArray()):'';
 $('runner-card').dataset.rotation=state.pose?JSON.stringify(glyph.group.quaternion.toArray()):'';
}
function showHomeRunners(local,fresh){
 const now=performance.now();
 const entries=Object.entries(local.runners).map(([key,value])=>{
  const sample=value.robot_pose,stamp=sample?JSON.stringify([sample.clock,sample.capture_timestamp_ns]):null;
  let update=poseUpdates.get(key);if(!update||update.stamp!==stamp){update={stamp,at:now};poseUpdates.set(key,update);}
  const pose=sample?.pose||null;
  return {key,name:peerNames.get(key)||'Galbot '+key.slice(-6),state:{pose,status:pose?(fresh&&value.status==='aligned'&&now-update.at<10000?'live':'stale'):'waiting'}};
 });
 renderRunners(entries);
 $('runner-list').hidden=false;
 const live=entries.filter(e=>e.state.status==='live').length;
 $('runner-status').textContent=`${entries.length} Galbots · ${live} live`;
 $('runner-card').dataset.status=live===entries.length?'live':'stale';
 $('runner-card').dataset.position='';$('runner-card').dataset.rotation='';
 $('runner-position').textContent='Poses in Home’s shared portal frame';$('runner-detail').textContent='Each arrow points forward.';
 $('focus-runner').textContent='Focus all Galbots ↗';$('focus-runner').disabled=!entries.some(e=>e.state.pose);
 $('focus-runner').onclick=()=>{
  const points=entries.filter(e=>e.state.pose).map(e=>new THREE.Vector3(...e.state.pose.translation));if(!points.length)return;
  const box=new THREE.Box3().setFromPoints(points),center=box.getCenter(new THREE.Vector3()),size=box.getSize(new THREE.Vector3());
  const distance=Math.max(4,size.length()*1.5/Math.min(camera.aspect,1));controls.target.copy(center);camera.position.copy(center).add(new THREE.Vector3(0,distance,distance*.6));controls.update();
 };
}
$('runner-layer').onchange=()=>{for(const glyph of robots.values())glyph.group.visible=Boolean(glyph.state?.pose)&&$('runner-layer').checked;refreshRunner();};
async function refreshRunner(){
 if((isRunner()||activeMap==='home-live')) {
  const local=current?.local_mapping, remote=current?.remote_status;
  const fresh=remote?.status==='connected' && remote.age_ms<10000 && performance.now()-(current?.receivedAt||0)<5000;
  if(activeMap==='home-live'&&local?.runners){showHomeRunners(local,fresh);return;}
  showRunner({status:fresh&&local?.robot_pose?'live':'stale',pose:local?.robot_pose?.pose||null,detail:local?(activeMap==='home-live'?'Pose in Home’s observed portal frame':'Pose in the robot boot frame'):'Waiting for independent robot pose'});return;
 }
 if(runnerBusy)return;runnerBusy=true;try{const state=await api('/api/runner-pose');lastRunnerReply=performance.now();if(!(isRunner()||activeMap==='home-live'))showRunner(state);}catch{if(!(isRunner()||activeMap==='home-live'))showRunner({...runner,status:'unavailable',pose:null,detail:'Home telemetry connection lost'});}finally{runnerBusy=false;}}
runnerCatalog();refreshRunner();setInterval(refreshRunner,250);
setInterval(()=>{if(!(isRunner()||activeMap==='home-live')&&lastRunnerReply&&performance.now()-lastRunnerReply>3000&&runner?.status==='live')showRunner({...runner,status:'stale',detail:'Home telemetry has stopped updating'});},500);

async function refreshMapping(){if(activeMap==='home-live'){try{const state=await api('/api/home-live-status');$('mapping-status').textContent='Home map · '+state.status;$('mapping-detail').textContent=state.detail||'';}catch{$('mapping-status').textContent='Home map · unavailable';}return;}if(activeMap==='demo'&&!current?.demo){$('mapping-status').textContent='Canonical reference';$('mapping-detail').textContent='13 supplied portal poses and physical sizes';return;}if(isRunner()){try{const sources=await api('/api/scene-sources');const remote=activeMap.startsWith('runner:')?(sources.runners||[]).find(r=>r.peer_id===activeMap.slice(7))||{status:'unavailable'}:sources;$('mapping-status').textContent='Galbot map · '+remote.status;$('mapping-detail').textContent=remote.detail||'';}catch{$('mapping-status').textContent='Galbot map · unavailable';}return;}try{const mapping=await api('/api/mapping');$('mapping-status').textContent='LiDAR · '+mapping.status+(mapping.rgbd?' / RGB-D · '+mapping.rgbd.status:'');$('mapping-detail').textContent=(mapping.detail||'')+(mapping.rgbd?' · '+(mapping.rgbd.detail||`${mapping.rgbd.colored_voxels} colored voxels`):'');$('mapping-card').dataset.status=mapping.status;}catch{$('mapping-status').textContent='LiDAR · unavailable';}}

// Prefer live topography; canonical scenes can also use the reference floor.
const labelDialog=$('label-dialog'),labelInput=$('label-dialog-text'),labelStatus=$('label-dialog-status'),labelSave=$('label-dialog-save');
let placement=null,placementGeneration=0,displayedCamera=null,labelFields=null;
function placementError(message){labelStatus.textContent=message;labelSave.disabled=true;}
async function beginLabel(request,error){
 const generation=++placementGeneration;placement=null;labelInput.value='';labelInput.disabled=false;labelSave.disabled=true;
 $('label-note-fields').replaceChildren();labelFields=noteFields($('label-note-fields'),null);
 labelStatus.textContent='Finding a surface · 50 cm beam…';if(!labelDialog.open)labelDialog.showModal();
 labelInput.focus();
 if(error){placementError(error);return;}
 try{const result=await api('/api/label-pick',request);if(generation!==placementGeneration||!labelDialog.open)return;
  if(!result.hit){placementError('No surface within 40 metres. Close and right-click nearer the floor.');return;}
  placement=result.hit;labelStatus.textContent=`${placement.floor?'Reference floor (Y = 0; unmeasured)':'Voxel surface'} · [${placement.position.map(v=>v.toFixed(2)).join(', ')}] m`;
  labelInput.disabled=false;labelSave.disabled=false;labelInput.focus();
 }catch(e){if(generation===placementGeneration)placementError(e.message);}
}
$('label-dialog-cancel').onclick=()=>labelDialog.close();
labelDialog.addEventListener('close',()=>{placementGeneration++;placement=null;});
$('label-dialog-form').onsubmit=async e=>{e.preventDefault();if(!placement||labelSave.disabled)return;labelSave.disabled=true;
 try{const details=await labelFields.read(labelInput.value);const note=await api('/api/notes/from-pick',{token:placement.token,text:labelInput.value,details});labelDialog.close();lastScene=null;await refresh();sidebarNoteId=note.id;noteView='detail';openPanel('notes');renderNotes();}
 catch(e){labelStatus.textContent=e.message;labelSave.disabled=false;}
};
renderer.domElement.addEventListener('contextmenu',e=>{e.preventDefault();
 if(!current){beginLabel(null,'Wait for Home’s map to load.');return;}
 const rect=renderer.domElement.getBoundingClientRect(),ray=new THREE.Raycaster();
 ray.setFromCamera(new THREE.Vector2((e.clientX-rect.left)/rect.width*2-1,1-(e.clientY-rect.top)/rect.height*2),camera);
 beginLabel({source:'scene',frame:current.frame,origin:ray.ray.origin.toArray(),direction:ray.ray.direction.toArray()});
});
$('sensor-stage').addEventListener('contextmenu',e=>{e.preventDefault();e.stopPropagation();
 const f=displayedCamera;
 if(!f?.token||Date.now()-f.receivedAt>10000){beginLabel(null,f?.error||'Waiting for a fresh capture-matched camera pose. Try again when calibrated video arrives.');return;}
 const rect=$('sensor-image').getBoundingClientRect();
 beginLabel({source:'camera',camera_token:f.token,pixel:[(e.clientX-rect.left)/rect.width*f.width,(e.clientY-rect.top)/rect.height*f.height]});
});

// Camera previews are independent of the selected map and never control the robot.
const sensorSelect=document.querySelector('#sensor-select'),pip=document.querySelector('#sensor-pip'),sensorCanvas=document.querySelector('#sensor-image');
let sensorGeneration=0,lastSensorSequence=null,lastSensorChange=0;
const arToggle=$('sensor-ar'),arStatus=$('sensor-ar-status');
let arOverlay=null,arLastSuccess=0;
const arAvailable=new Map();
function hideAr(){arOverlay?.hide();arLastSuccess=0;}
function updateArAvailability(){
 const supported=arAvailable.get(sensorSelect.value)===true;
 arToggle.disabled=!supported;
 arStatus.hidden=!sensorSelect.value||(!arToggle.checked&&supported);
 if(!supported)arStatus.textContent='AR needs capture-matched camera calibration and transforms';
}
function restartSensor(){
 updateArAvailability();
 const generation=++sensorGeneration;displayedCamera=null;lastSensorSequence=null;lastSensorChange=0;hideAr();
 if(arToggle.checked){arStatus.hidden=false;arStatus.textContent='AR · waiting for a matched image and pose…';}
 watchSensor(generation);
}
arToggle.addEventListener('change',restartSensor);
setInterval(()=>{if(!labelDialog.open&&arToggle.checked&&arLastSuccess&&Date.now()-arLastSuccess>3000){hideAr();arStatus.hidden=false;arStatus.textContent='AR paused · camera frames are stale';}},500);
async function sensorCatalog(){
 try{
  const response=await fetch('/api/sensors');const data=await response.json();if(!response.ok)throw Error(data.error);
  const selected=sensorSelect.value;const options=[new Option('None','')];
  arAvailable.clear();
  for(const s of data.sensors){arAvailable.set(s.id,s.ar_available===true);const option=new Option(s.label+(s.ar_available?' · AR':''),s.id);option.dataset.peer=s.peer_id;options.push(option);}
  if(selected&&!data.sensors.some(s=>s.id===selected))options.push(new Option(document.querySelector('#sensor-title').textContent+' · offline',selected));
  sensorSelect.replaceChildren(...options);sensorSelect.value=selected;updateArAvailability();
  document.querySelector('#sensor-catalog-status').textContent=data.sensors.length?`${data.sensors.length} published camera feeds`:'No camera feeds available · retrying';
 }catch(e){document.querySelector('#sensor-catalog-status').textContent='Camera peers unavailable · retrying';}
 setTimeout(sensorCatalog,10000);
}
async function watchSensor(generation){
 if(generation!==sensorGeneration||!sensorSelect.value)return;
 const status=document.querySelector('#sensor-feed-status');
 try{
  const response=await fetch('/api/sensors?sensor='+encodeURIComponent(sensorSelect.value)+(arToggle.checked?'&ar=true':'')+(arAvailable.get(sensorSelect.value)?'&pick=true':''));const f=await response.json();if(!response.ok)throw Error(f.error);
  if(generation!==sensorGeneration)return;
  if(f.sequence!==lastSensorSequence&&!labelDialog.open){
   const bytes=Uint8Array.from(atob(f.data),c=>c.charCodeAt(0));
   if(!Number.isInteger(f.width)||!Number.isInteger(f.height)||f.width<1||f.height<1||f.width>4096||f.height>4096)throw Error('Invalid image dimensions');
   const bitmap=(f.encoding==='jpeg'||f.encoding==='jpg')?await createImageBitmap(new Blob([bytes],{type:'image/jpeg'})):null;
   if(generation!==sensorGeneration){bitmap?.close();return;}
   if(labelDialog.open){bitmap?.close();setTimeout(()=>watchSensor(generation),100);return;}
   sensorCanvas.style.aspectRatio=`${f.width}/${f.height}`;sensorCanvas.width=f.width;sensorCanvas.height=f.height;const ctx=sensorCanvas.getContext('2d');
   if(bitmap){ctx.drawImage(bitmap,0,0,f.width,f.height);bitmap.close();}else{
    const channels=f.encoding==='rgb8'?3:f.encoding==='luma8'?1:0;
    if(!channels||bytes.length!==f.width*f.height*channels)throw Error('Unsupported camera frame');
    const image=ctx.createImageData(f.width,f.height);
    for(let i=0;i<f.width*f.height;i++){image.data[i*4]=bytes[i*channels];image.data[i*4+1]=bytes[i*channels+(channels===3?1:0)];image.data[i*4+2]=bytes[i*channels+(channels===3?2:0)];image.data[i*4+3]=255;}ctx.putImageData(image,0,0);
   }
   displayedCamera={token:f.camera_token,error:f.pick_error,width:f.width,height:f.height,receivedAt:Date.now()};
   if(arToggle.checked){arOverlay??=createArOverlay($('sensor-ar-overlay'));arOverlay.show(f);arLastSuccess=Date.now();arStatus.textContent=['sdk-camera','sdk-fisheye'].includes(f.ar?.projection)?'AR · Home live voxels + notes · SDK camera projection':'AR · Home live voxels + notes · synchronized mapping camera';}
   lastSensorSequence=f.sequence;lastSensorChange=Date.now();
  }
  status.textContent=Date.now()-lastSensorChange>3000?'Stale · no new frames':'Receiving · frame '+f.sequence;
 }catch(e){if(generation===sensorGeneration){status.textContent='Unavailable · '+e.message;if(!labelDialog.open)hideAr();if(arToggle.checked){arStatus.hidden=false;arStatus.textContent='AR paused · '+e.message;}}}
 if(generation===sensorGeneration)setTimeout(()=>watchSensor(generation),100);
}
sensorSelect.addEventListener('change',()=>{
 pip.hidden=!sensorSelect.value;arToggle.checked=false;updateArAvailability();
 sensorCanvas.getContext('2d').clearRect(0,0,sensorCanvas.width,sensorCanvas.height);
 document.querySelector('#sensor-title').textContent=sensorSelect.selectedOptions[0].textContent;
 document.querySelector('#sensor-feed-status').textContent='Waiting for a frame…';restartSensor();
});
document.querySelector('#sensor-close').addEventListener('click',()=>{sensorSelect.value='';sensorSelect.dispatchEvent(new Event('change'));});
sensorCatalog();

// Transient body occupancy: keep cube axes in robot.base, transform the entire
// group with the SDK-composed capture-time pose. Never add cells to topography.
const robotVolumes=new Map();let volumeBusy=false;
function removeVolume(key){const v=robotVolumes.get(key);if(v){scene.remove(v.group);clear(v.group);robotVolumes.delete(key);}}
function resetVolumes(){for(const key of [...robotVolumes.keys()])removeVolume(key);}
$('map-select').addEventListener('change',resetVolumes);
$('robot-volume-layer').onchange=()=>{for(const v of robotVolumes.values())v.group.visible=$('robot-volume-layer').checked;};
async function refreshRobotVolumes(){
 if(volumeBusy)return;
 if(!(activeMap==='home-live'||isRunner())){resetVolumes();$('robot-volume-status').textContent='Robot volume · choose a live map';return;}
 const selected=activeMap,frame=current?.frame;
 const peer=selected==='runner'?primaryRunnerPeer:selected.startsWith('runner:')?selected.slice(7):null;
 if(isRunner()&&!peer)return;
 volumeBusy=true;
 try{
  const data=await api('/api/robot-occupancy'+(peer?'?peer='+encodeURIComponent(peer):''));
  if(selected!==activeMap||frame!==current?.frame)return;
  const entries=(data.robots||[]).filter(r=>r.frame===frame);
  const keys=new Set(entries.map(r=>r.peer_id));for(const key of [...robotVolumes.keys()])if(!keys.has(key))removeVolume(key);
  for(const r of entries){
   const signature=JSON.stringify([r.model_sha256,r.timestamp_ns,r.sequence]);let v=robotVolumes.get(r.peer_id);
   if(!v||v.signature!==signature){
    removeVolume(r.peer_id);
    const group=new THREE.Group(),material=new THREE.MeshStandardMaterial({color:'#ff2020',emissive:'#a00000',emissiveIntensity:.3,roughness:.7});
    const mesh=new THREE.InstancedMesh(boxGeometry,material,r.cells.length),matrix=new THREE.Matrix4();
    r.cells.forEach((c,i)=>{const s=r.voxel_size;matrix.makeScale(s*.96,s*.96,s*.96);matrix.setPosition(...c.map(n=>(n+.5)*s));mesh.setMatrixAt(i,matrix);});
    mesh.instanceMatrix.needsUpdate=true;mesh.computeBoundingSphere();group.add(mesh);
    group.position.fromArray(r.pose.translation);group.quaternion.fromArray(r.pose.rotation_xyzw);group.visible=$('robot-volume-layer').checked;
    scene.add(group);v={group,signature,at:performance.now()};robotVolumes.set(r.peer_id,v);
   }
  }
  const count=entries.reduce((n,r)=>n+r.cells.length,0),missing=entries.reduce((n,r)=>n+r.missing_links.length,0);
  const status=$('robot-volume-status');status.dataset.robots=String(entries.length);status.dataset.cells=String(count);
  status.dataset.poses=JSON.stringify(entries.map(r=>({peer:r.peer_id,pose:r.pose})));
  status.textContent=entries.length?`${entries.length} robot${entries.length===1?'':'s'} · ${count.toLocaleString()} red cells · 2 Hz${missing?' · '+missing+' links missing':''}${entries.some(r=>r.approximate_links.length)?' · gripper travel envelopes':''}`:
   'Robot volume · '+(data.unavailable?.[0]?.detail||data.status||'waiting for runner');
 }catch(e){$('robot-volume-status').textContent='Robot volume · '+e.message;}
 finally{volumeBusy=false;}
}
setInterval(()=>{for(const [key,v] of robotVolumes)if(performance.now()-v.at>3000)removeVolume(key);refreshRobotVolumes();},500);

function uiButton(text,action,className='panel-row'){
 const button=document.createElement('button');button.type='button';button.className=className;button.textContent=text;button.onclick=action;return button;
}
function renderMapRows(){
 mapsList.replaceChildren();
 for(const name of ['Local','Network']){
  const section=document.createElement('section');section.setAttribute('aria-label',name+' scenegraphs');const heading=document.createElement('div');heading.className='section-label';heading.textContent=name;section.append(heading);
  const options=Array.from($('map-select').options).filter(o=>(o.parentElement.label||'Local')===name);
  section.append(...options.map(option=>{
  const button=uiButton(option.textContent,()=>{$('map-select').value=option.value;$('map-select').dispatchEvent(new Event('change'));renderMapRows();});
  button.dataset.map=option.value;button.setAttribute('aria-pressed',String(option.value===$('map-select').value));return button;
 }));
  if(!options.length){const empty=document.createElement('p');empty.className='panel-hint';empty.textContent='No scenegraphs available';section.append(empty);}
  mapsList.append(section);
 }
}
new MutationObserver(renderMapRows).observe($('map-select'),{childList:true,subtree:true,characterData:true});
$('map-select').addEventListener('change',()=>{renderMapRows();noteView='list';renderNotes();});renderMapRows();
function renderFleetRows(){
 const selfId=fleetNetwork?.connection?.peer_id;
 const peers=[{peer_id:selfId||'self',name:'Home Server',self:true},...Array.from(new Map((fleetNetwork?.peers||[]).filter(p=>p.peer_id&&p.peer_id!==selfId).map(p=>[p.peer_id,p])).values())];
 const sensors=Array.from($('sensor-select').options).filter(o=>o.value);
 const signature=JSON.stringify([peers,fleetNetwork?.enabled,fleetNetwork?.status,sensors.map(o=>[o.value,o.textContent,o.dataset.peer]),$('sensor-select').value]);
 if(signature===fleetSignature)return;fleetSignature=signature;
 const opened=new Set(Array.from(fleetList.querySelectorAll('details[open]'),d=>d.dataset.peer));fleetList.replaceChildren();
 for(const peer of peers){const options=sensors.filter(o=>o.dataset.peer===peer.peer_id);const group=document.createElement('details');group.dataset.peer=peer.peer_id;group.open=opened.has(peer.peer_id);const heading=document.createElement('summary');heading.textContent=peer.self?'Home Server':peer.peer_id;group.append(heading);
  const detail=document.createElement('p');detail.className='panel-hint';detail.textContent=peer.self?(fleetNetwork?.enabled?'This peer · '+fleetNetwork.status:fleetNetwork?'This peer · P2P disabled':'This peer · Network status unavailable'):'Connected peer';group.append(detail);
  if(peer.self&&selfId){const identity=document.createElement('p');identity.className='panel-hint';identity.textContent=selfId;group.append(identity);}
  for(const option of options){const button=uiButton(option.textContent+($('sensor-select').value===option.value?' · PiP ●':''),()=>{$('sensor-select').value=$('sensor-select').value===option.value?'':option.value;$('sensor-select').dispatchEvent(new Event('change'));});button.setAttribute('aria-pressed',String($('sensor-select').value===option.value));group.append(button);}fleetList.append(group);
 }
}
new MutationObserver(renderFleetRows).observe($('sensor-select'),{childList:true});$('sensor-select').addEventListener('change',renderFleetRows);renderFleetRows();
let noteView='list',sidebarNoteId=null,noteSignature='';
function noteMessage(text){const p=document.createElement('p');p.className='panel-hint';p.textContent=text;notesBody.append(p);return p;}
function noteTitle(note){return note?.details?.title||note?.text||'';}
function notePhoto(note){
 const button=uiButton('',()=>{
  let dialog=$('photo-dialog');
  if(!dialog){dialog=document.createElement('dialog');dialog.id='photo-dialog';document.body.append(dialog);dialog.addEventListener('click',e=>{if(e.target===dialog)dialog.close();});}
  dialog.replaceChildren();const title=document.createElement('h3');title.textContent=noteTitle(note);dialog.append(title);
  const image=document.createElement('img');image.src=note.details.photo;image.alt='Photo for '+noteTitle(note);dialog.append(image,uiButton('Close photo',()=>dialog.close()));dialog.showModal();
 },'note-photo-button');button.setAttribute('aria-label','Open photo for '+noteTitle(note));
 const image=document.createElement('img');image.className='note-photo';image.src=note.details.thumbnail||note.details.photo;image.alt='Photo for '+noteTitle(note);button.append(image);return button;
}
function noteFields(container,note){
 const details=note?.details||{};
 function field(caption,tag,value,max){const label=document.createElement('label');label.textContent=caption;const input=document.createElement(tag);input.value=value||'';input.maxLength=max;label.append(input);container.append(label);return input;}
 const description=field('Description','textarea',details.description,10000);
 const photoLabel=document.createElement('label');photoLabel.textContent='Upload photo';const upload=document.createElement('input');upload.type='file';upload.accept='image/*';photoLabel.append(upload);container.append(photoLabel);
 const preview=document.createElement('img');preview.className='note-photo';preview.alt='Attached note photo';container.append(preview);
 const status=document.createElement('p');status.className='panel-hint';status.setAttribute('role','status');container.append(status);
 let photo=details.photo||null,pending=Promise.resolve(),photoError=null,generation=0;
 const remove=uiButton('Remove photo',()=>{generation++;photo=null;photoError=null;upload.value='';status.textContent='';render();});container.append(remove);
 function render(){preview.hidden=!photo;remove.hidden=!photo;if(photo)preview.src=photo;else preview.removeAttribute('src');}render();
 upload.onchange=()=>{const file=upload.files[0];if(!file)return;const version=++generation;photoError=null;status.textContent='Preparing photo…';pending=(async()=>{let bitmap;try{
  if(file.size>20*1024*1024)throw Error('Choose an image smaller than 20 MB.');
  bitmap=await createImageBitmap(file);const scale=Math.min(1,1280/Math.max(bitmap.width,bitmap.height));const canvas=document.createElement('canvas');canvas.width=Math.max(1,Math.round(bitmap.width*scale));canvas.height=Math.max(1,Math.round(bitmap.height*scale));const ctx=canvas.getContext('2d');ctx.fillStyle='#fff';ctx.fillRect(0,0,canvas.width,canvas.height);ctx.drawImage(bitmap,0,0,canvas.width,canvas.height);const encoded=canvas.toDataURL('image/jpeg',.8);if(encoded.length>1400000)throw Error('Photo is too large. Choose a smaller image.');if(version!==generation)return;photo=encoded;status.textContent='Photo attached';render();
 }catch(e){if(version===generation){photoError=e;status.textContent=e.message;}}finally{bitmap?.close();}})();};
 return {read:async title=>{await pending;if(photoError)throw photoError;return {title,description:description.value,photo};}};
}
function renderNotes(){
 if(noteView==='edit')return;
 notesBody.replaceChildren();const notes=current?.notes||[];const selected=notes.find(n=>n.id===sidebarNoteId);
 if(noteView==='detail'&&selected){
  notesBody.append(uiButton('‹ All notes',()=>{noteView='list';renderNotes();}));
  const title=document.createElement('h3');title.textContent=noteTitle(selected);notesBody.append(title);noteMessage('Pose · '+selected.position.map(v=>v.toFixed(2)).join(', ')+' m');
  if(selected.details?.description)noteMessage(selected.details.description);
  if(selected.details?.photo)notesBody.append(notePhoto(selected));
  notesBody.append(uiButton('Focus in scene',()=>focusNote(selected)));
  if(!selected.anchor_robot&&!selected.phone_pose&&!isRunner()&&activeMap!=='domain'){
   notesBody.append(uiButton('Edit',()=>editSidebarNote(selected)),uiButton('Delete',async()=>{const error=noteMessage('Deleting…');try{await api('/api/notes/'+encodeURIComponent(selected.id),undefined,'DELETE');noteView='list';lastScene=null;await refresh();renderNotes();}catch(e){error.textContent=e.message;}}));
  }
  return;
 }
 notesBody.append(uiButton('+ New note',()=>editSidebarNote(null),'primary'));
 if(!notes.length)noteMessage('No notes in this scenegraph.');
 for(const note of notes){const row=uiButton(noteTitle(note),()=>{sidebarNoteId=note.id;noteView='detail';focusNote(note);renderNotes();});if(note.details?.thumbnail){const image=document.createElement('img');image.src=note.details.thumbnail;image.alt='';image.className='note-list-thumbnail';row.prepend(image);}notesBody.append(row);}
}
async function editSidebarNote(note){
 noteView='edit';notesBody.replaceChildren();const form=document.createElement('form');form.className='note-form';
 const textLabel=document.createElement('label');textLabel.textContent='Title';const input=document.createElement('input');input.name='text';input.required=true;input.maxLength=120;input.value=noteTitle(note);textLabel.append(input);form.append(textLabel);
 const fields=noteFields(form,note);
 let portal,height;
 if(!note){
  const hint=document.createElement('p');hint.className='panel-hint';hint.textContent='Anchor to a Home portal below, or right-click a surface in the scene to place a note there.';form.append(hint);
  const portalLabel=document.createElement('label');portalLabel.textContent='Anchor portal';portal=document.createElement('select');portal.name='portal';portal.required=true;portalLabel.append(portal);form.append(portalLabel);
  const heightLabel=document.createElement('label');heightLabel.textContent='Height above portal (m)';height=document.createElement('input');height.name='height';height.type='number';height.step='0.01';height.min='-100';height.max='100';height.value='1';height.required=true;heightLabel.append(height);form.append(heightLabel);
 }
 const actions=document.createElement('div');actions.className='note-actions';const save=document.createElement('button');save.type='submit';save.textContent='Save';actions.append(uiButton('Cancel',()=>{noteView=note?'detail':'list';renderNotes();}),save);form.append(actions);const error=document.createElement('p');error.className='panel-hint';error.setAttribute('role','status');form.append(error);notesBody.append(form);input.focus();
 if(!note){save.disabled=true;try{const canonical=await api('/api/scene');for(const p of canonical.portals||[])portal.add(new Option(p.anchor_id,p.anchor_id));save.disabled=!portal.options.length;}catch(e){error.textContent=e.message;}}
 form.onsubmit=async event=>{event.preventDefault();save.disabled=true;try{
  const details=await fields.read(input.value);
  const result=note?await api('/api/notes/'+encodeURIComponent(note.id),{text:input.value,details},'PATCH'):await api('/api/notes',{text:input.value,details,portal:portal.value,height_m:Number(height.value)});
  sidebarNoteId=result.id;noteView='detail';
  if(isRunner()||activeMap==='domain'){$('map-select').value='demo';await $('map-select').onchange();}
  lastScene=null;await refresh();renderNotes();
 }catch(e){error.textContent=e.message;save.disabled=false;}};
}
setInterval(()=>{const signature=JSON.stringify(current?.notes||[]);if(signature!==noteSignature){noteSignature=signature;renderNotes();}},500);renderNotes();
function applyTheme(light){document.documentElement.dataset.theme=light?'light':'dark';scene.background.set(light?'#e8e8f0':'#0d0d0f');scene.fog?.color.copy(scene.background);ground.material.color.set(light?'#e0e3e8':'#101014');themeButton.textContent=light?'☾':'☼';themeButton.title=light?'Switch to dark theme':'Switch to light theme';themeButton.setAttribute('aria-label',themeButton.title);}
themeButton.onclick=()=>{const light=document.documentElement.dataset.theme!=='light';applyTheme(light);try{localStorage.setItem('spatial-theme',light?'light':'dark');}catch{}};
try{applyTheme(localStorage.getItem('spatial-theme')==='light');}catch{}
const opacityLabel=document.createElement('label');opacityLabel.className='layer';opacityLabel.textContent='Grid opacity';const opacityInput=document.createElement('input');opacityInput.type='range';opacityInput.min='0';opacityInput.max='1';opacityInput.step='.01';opacityInput.value=String(grid.material.opacity);opacityInput.oninput=()=>{grid.material.opacity=Number(opacityInput.value);};opacityLabel.append(opacityInput);panelBodies.settings.append(opacityLabel);
