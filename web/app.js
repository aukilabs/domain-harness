import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { createArOverlay } from './ar.js';
import { createPhoneTrails, cameraShape } from './phone-trails.js';
import { createCompactPoseTrail } from './compact-pose-trail.js';
import { createSemanticVolumes, legacyVolumesForView } from './semantic-volumes.js';
import { createMap2D } from './map-2d.js';
import { createCrayon } from './crayon.js';
import { createGhostCamera, createCameraPan, isEditing } from './ghost-camera.js';
import { createNoteBubble } from './note-bubble.js';
import { createPipWindow } from './pip-window.js';
import { createVideoPresentation } from './video-presentation.js';
import { readCameraResponse } from './camera-frame.js';
import { moveMapCamera, applyChatActions, chatResultPresentation } from './view-camera.js';
import { createAttachedCamera } from './attached-camera.js';
import { createRobotModel } from './robot-model.js';
import { applyRobotHealth } from './robot-health.js';
const $=id=>document.getElementById(id);
const chatForm=$('scene-chat-form'),chatInput=$('scene-chat-input'),chatStatus=$('scene-chat-status');
chatForm.querySelector('button').disabled=true;
const chatHistory=$('scene-chat-history'),chatConversations=new Map();let chatPending=false;
let chatCompact=false;try{chatCompact=localStorage.getItem('home.chat.compact')==='true';}catch{}
function syncChatCompact(){document.querySelector('.scene-chat').classList.toggle('compact',chatCompact);const toggle=$('scene-chat-toggle');toggle.textContent=chatCompact?'⌃':'⌄';toggle.title=chatCompact?'Show conversation':'Hide conversation';toggle.setAttribute('aria-label',toggle.title);toggle.setAttribute('aria-expanded',String(!chatCompact));}
$('scene-chat-toggle').onclick=()=>{chatCompact=!chatCompact;try{localStorage.setItem('home.chat.compact',String(chatCompact));}catch{}syncChatCompact();};syncChatCompact();
function renderChat(){const messages=chatConversations.get(activeMap)||[];chatHistory.replaceChildren();chatHistory.hidden=!messages.length;for(const m of messages){const row=document.createElement('div');row.className='scene-chat-message '+m.role;const who=document.createElement('strong');who.textContent=m.role==='user'?'You':'Qwen';const text=document.createElement('div');text.textContent=m.display_content||m.content;row.append(who,text);chatHistory.append(row);}chatHistory.scrollTop=chatHistory.scrollHeight;}
chatInput.addEventListener('input',()=>{chatForm.querySelector('button').disabled=chatPending||!chatInput.value.trim();});
chatForm.onsubmit=async e=>{
 e.preventDefault();if(chatPending||!chatInput.value.trim())return;
 const map=activeMap,message=chatInput.value.trim(),history=(chatConversations.get(map)||[]).slice(-18);
 history.push({role:'user',content:message});
 if(history.reduce((n,m)=>n+m.content.length,0)>32000){chatStatus.textContent='Conversation is too long. Start a shorter conversation.';return;}
 chatConversations.set(map,history);renderChat();chatPending=true;chatInput.value='';chatForm.querySelector('button').disabled=true;chatStatus.textContent='Qwen is replying locally…';
 try{
  const result=await api('/api/chat',{messages:history.map(({role,content})=>({role,content})),scene:map});
  const browser=result.ui_actions===undefined?null:await applyChatCamera(result.ui_actions,map);
  const presentation=chatResultPresentation(result,browser);
  history.push({role:'assistant',content:presentation.content,display_content:presentation.display_content});
  chatStatus.textContent=presentation.status;
  if(result.changes?.length){
   // A failed refresh cannot undo saved receipts or offer the whole edit again.
   lastScene=null;try{await refresh();}catch(error){chatStatus.textContent+=' · View refresh failed: '+error.message;}
  }
 }catch(error){history.pop();if(activeMap===map&&!chatInput.value)chatInput.value=message;chatStatus.textContent=error.message;}
 finally{chatPending=false;renderChat();chatForm.querySelector('button').disabled=!chatInput.value.trim();}
};
$('map-select').addEventListener('change',()=>{renderChat();chatStatus.textContent=chatPending?'A local reply is pending for the previous scene…':'Local Qwen · inspect scene and edit volume descriptions';});
for(const event of ['pointerdown','pointerup','click','dblclick','contextmenu','wheel','keydown','keyup']){
 document.querySelector('.scene-chat').addEventListener(event,e=>e.stopPropagation());
}
// Organize existing live controls into the mockup's rail and exclusive panels.
const sidebar=document.querySelector('aside'),shell=document.querySelector('.shell');
const rail=document.createElement('nav');rail.className='icon-rail';rail.setAttribute('aria-label','Workspace panels');shell.prepend(rail);
const panelNames={profile:'Profile',maps:'Maps',notes:'Notes',volumes:'Volumes',tags:'Tags',fleet:'Devices',settings:'Settings'};
const iconPaths={profile:'M9 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6M3 16c0-7 12-7 12 0',maps:'M2 4l5-2 4 2 5-2v12l-5 2-4-2-5 2ZM7 2v12M11 4v12',notes:'M4 2h10v14H4ZM6 6h6M6 9h6M6 12h4',tags:'M2 6V2h4M12 2h4v4M16 12v4h-4M6 16H2v-4M6 6h6v6H6Z',fleet:'M4 6h10v9H4ZM6 2h6v4M7 9h.1M11 9h.1M7 12h4M1 10h3M14 10h3',settings:'M9 1v3M9 14v3M1 9h3M14 9h3M3 3l2 2M13 13l2 2M3 15l2-2M13 5l2-2M12 9a3 3 0 1 0-6 0 3 3 0 0 0 6 0'};
const panelBodies={};let activePanel='profile';
iconPaths.volumes='M2 5l7-3 7 3v8l-7 3-7-3ZM2 5l7 3 7-3M9 8v8';
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
const spatialBrowser=document.createElement('section');spatialBrowser.id='spatial-browser';spatialBrowser.setAttribute('aria-label','Anchors and landmarks');
spatialBrowser.innerHTML='<h3>Anchors & landmarks</h3><label class="spatial-search-label" for="spatial-search">Find by name or type</label><input id="spatial-search" type="search" placeholder="ID, type, or corner…"><p id="spatial-status" class="panel-hint" role="status"></p><details open><summary>Anchors <span id="anchor-count">0</span></summary><div id="anchor-list" class="spatial-list" aria-label="Anchors"></div></details><details open><summary>Landmarks <span id="landmark-count">0</span></summary><div id="landmark-list" class="spatial-list" aria-label="Landmarks"></div></details>';
mapsList.after(spatialBrowser);
$('spatial-search').oninput=()=>renderSpatialLists();
const fleetList=document.createElement('div');fleetList.id='fleet-sensors';panelBodies.fleet.append(fleetList);$('sensor-select').hidden=true;
const {createPoseInspector}=await import('/pose-inspector.js');
const inspectPose=createPoseInspector(panelBodies.fleet);
const disconnectRobots=document.createElement('button');disconnectRobots.id='disconnect-robots';disconnectRobots.className='panel-row';disconnectRobots.textContent='Disconnect simulated robots';disconnectRobots.hidden=true;panelBodies.fleet.append(disconnectRobots);
disconnectRobots.onclick=async()=>{disconnectRobots.disabled=true;try{await api('/api/simulation/disconnect',{});}catch(e){disconnectRobots.disabled=false;availability.hidden=false;availability.textContent=e.message;}};
const notesBody=panelBodies.notes;
const tagsBody=panelBodies.tags;
let fleetNetwork=null,fleetSignature='';
const componentControlResults=new Map();
let componentControlPending=false;
const scene=new THREE.Scene();scene.background=new THREE.Color('#0d0d0f');scene.fog=new THREE.Fog('#0d0d0f',10,26);
const camera=new THREE.PerspectiveCamera(42,1,.01,100);
const renderer=new THREE.WebGLRenderer({antialias:true,preserveDrawingBuffer:true});renderer.setPixelRatio(Math.min(devicePixelRatio,2));renderer.shadowMap.enabled=true;renderer.shadowMap.type=THREE.PCFSoftShadowMap;renderer.outputColorSpace=THREE.SRGBColorSpace;renderer.toneMapping=THREE.ACESFilmicToneMapping;renderer.toneMappingExposure=1.1;
$('canvas').append(renderer.domElement);renderer.domElement.setAttribute('aria-label','3D voxel map');
const controls=new OrbitControls(camera,renderer.domElement);controls.enableDamping=true;controls.minDistance=1.2;controls.maxDistance=60;controls.maxPolarAngle=Math.PI*.49;
let crayon=null;
const cameraPan=createCameraPan({camera,controls});
renderer.domElement.addEventListener('pointerdown',()=>cameraPan.cancel(),true);
renderer.domElement.addEventListener('wheel',()=>cameraPan.cancel(),{capture:true,passive:true});
for(const id of ['map-select','fit','top','demo-view'])$(id).addEventListener(id==='map-select'?'change':'click',()=>cameraPan.cancel(),true);
const ghostCamera=createGhostCamera({camera,controls,canvas:renderer.domElement,enabled:()=>!map2D.isActive()&&!crayon?.isActive(),onMove:()=>{cameraPan.cancel();if(attachedCamera.isActive())attachedCamera.detach();}});
let activeMap='demo',current=null,busy=false,refreshing=false,lastScene=null,selectable=[],selectedNoteId=null,selectedPathId=null,deletingPathId=null,deletingNoteId=null,selectedSpatialNode=null,spatialSignature='';
let homeOffline=false,catalogInitialized=false;
function isLocalScene(map=activeMap){return map.startsWith('local:');}
function localSceneUrl(map=activeMap){return '/api/local-scenes/'+encodeURIComponent(map.slice(6));}
function noteUrl(id,map=activeMap){return '/api/notes/'+encodeURIComponent(id)+(isLocalScene(map)?'?scene='+encodeURIComponent(map.slice(6)):'');}
const availability=document.createElement('div');availability.id='scene-availability';availability.setAttribute('role','status');availability.style.cssText='position:absolute;top:48px;left:12px;right:12px;z-index:90;padding:10px;background:#432b20;color:#fff;font:12px system-ui;pointer-events:none';availability.hidden=true;$('canvas').append(availability);
function markOffline(){homeOffline=true;availability.hidden=false;availability.textContent='Home offline · showing a cached browser view, not a live or saved map';renderMapRows();}
function isRunner(map=activeMap){return map==='runner'||map.startsWith('runner:');}
function isSimulation(map=activeMap){return map==='sim-voxels'||map==='sim-occupancy';}
function runnerUrl(endpoint,map=activeMap){return endpoint+(map.startsWith('runner:')?'?peer='+encodeURIComponent(map.slice(7)):'');}
const runnerNames=new Map(),peerNames=new Map();
let primaryRunnerPeer=null;
function runnerDisplay(){return activeMap.startsWith('peer-scene:')?'Robot':isSimulation()?'Simulated robot':runnerNames.get(activeMap)||'Galbot';}
async function runnerCatalog(){
 try {
  const data=await api('/api/scene-sources');
  const local=document.createElement('optgroup');local.label='Local';
  homeOffline=false;
  for(const map of data.local||[]){if(map.available===false)continue;local.append(new Option(map.name+(map.status==='retained'?' · retained':''),map.id));}
  const network=document.createElement('optgroup');network.label='Network';
  for(const phone of data.phones||[]){
   // Prefer the phone-owned Catalog Product; the push cache is only a fallback.
   const product=(data.scenes||[]).find(m=>m.peer_id===phone.peer_id&&m.name==='phone-local-voxels');
   if(product){if(activeMap===phone.id){activeMap=product.id;lastScene=null;}continue;}
   network.append(new Option(phone.name,phone.id));
  }
  const visibleRunners=(data.runners||[]).filter(r=>r.available&&['connected','stale'].includes(r.status));
  for(const map of data.scenes||[]){
   // Configured runners subscribe to canonical-scene; keep other products on the same peer.
   const runner=map.name==='canonical-scene'&&visibleRunners.find(r=>r.peer_id===map.peer_id);
   const primary=!runner&&map.name==='canonical-scene'&&data.available&&['connected','stale'].includes(data.status)&&data.peer_id===map.peer_id;
   if(runner||primary){
    if(activeMap===map.id){activeMap=runner&&!runner.primary?'runner:'+runner.peer_id:'runner';lastScene=null;}
    continue;
   }
   const label=map.name==='phone-local-voxels'?'Phone · local map':map.name;
   const option=new Option(map.peer_alias||label+' · '+map.peer_id.slice(-8),map.id);
   option.title=label+' · '+map.peer_id;
   network.append(option);
  }
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
  for(const peer of fleetNetwork?.peers||[])network.append(new Option((peer.name||peer.peer_id)+' · pose trail','peer-pose:'+peer.peer_id));
  if(!catalogInitialized&&data.preferred&&Array.from(select.options).some(o=>o.value===data.preferred)){select.value=data.preferred;select.dispatchEvent(new Event('change'));}
  else if(Array.from(select.options).some(o=>o.value===activeMap))select.value=activeMap;
  else if(select.options.length){select.value=select.options[0].value;select.dispatchEvent(new Event('change'));}
  catalogInitialized=true;
  renderMapRows();
 }catch{markOffline();}
 setTimeout(runnerCatalog,5000);
}

function domainCenter(){return new THREE.Box3().setFromPoints((current?.portals||[]).map(p=>new THREE.Vector3(...p.pose_in_map.translation))).getCenter(new THREE.Vector3());}
function fit(){if(current?.simulation){const d=Math.max(12,11/Math.max(camera.aspect,.2));camera.position.set(0,d,d*.4);controls.target.set(0,0,0);}else if(current?.portals.length){const c=domainCenter();const box=new THREE.Box3().setFromPoints(current.portals.map(p=>new THREE.Vector3(...p.pose_in_map.translation)));const size=box.getSize(new THREE.Vector3());const distance=Math.max(size.x/Math.max(camera.aspect,.2),size.z)*1.65+3;camera.position.copy(c).add(new THREE.Vector3(0,distance,distance*.25));controls.target.copy(c);}else{camera.position.set(3.6,3.5,4.6);controls.target.set(0,.5,0);}controls.update();}
fit();
const map2D=createMap2D($('canvas'),active=>{controls.enabled=!active;$('top').textContent=active?'3D':'2D';$('top').setAttribute('aria-pressed',String(active));$('top').title=active?'Switch to 3D mode':'Switch to 2D mode';$('top').setAttribute('aria-label',$('top').title);$('canvas').dataset.viewMode=active?'2d':'3d';});
const attachedCamera=createAttachedCamera({host:$('canvas'),camera,controls,map2D,context:()=>({scene:activeMap,frame:current?.frame}),request:async(action,signal)=>{
 const q=new URLSearchParams({follow:'true',home_frame:action.frame,sensor:action.sensor_id,expected_manifest:action.pose_product.manifest_hash});
 const response=await fetch('/api/sensors?'+q,{signal});const value=await response.json();if(!response.ok)throw Error(value.error||'Sensor unavailable');return value;
}});
for(const id of ['map-select','fit','top','demo-view'])$(id).addEventListener(id==='map-select'?'change':'click',()=>{if(attachedCamera.isActive())attachedCamera.detach();},true);
$('demo-view').onclick=()=>{camera.position.set(-.5,4.5,5.5);controls.target.set(-3.7,.5,2.1);controls.update();};$('fit').onclick=()=>map2D.isActive()?map2D.fit():fit();$('top').onclick=()=>map2D.toggle();
scene.add(new THREE.HemisphereLight(0xffffff,0x1a2438,1.7));
const sun=new THREE.DirectionalLight(0xffffff,2.4);sun.position.set(-3,7,4);sun.castShadow=true;sun.shadow.mapSize.set(2048,2048);Object.assign(sun.shadow.camera,{left:-5,right:5,top:5,bottom:-5,near:.1,far:20});sun.shadow.bias=-.0005;scene.add(sun);
const ground=new THREE.Mesh(new THREE.PlaneGeometry(200,200),new THREE.MeshStandardMaterial({color:'#101014',roughness:1}));ground.rotation.x=-Math.PI/2;ground.position.y=-.15;ground.receiveShadow=true;scene.add(ground);
const grid=new THREE.GridHelper(6,30,0x4f8ef7,0x24242b);grid.position.y=-.14;grid.material.transparent=true;grid.material.opacity=.28;scene.add(grid);
const groups=Object.fromEntries(['voxels','portals','objects','rays','notes'].map(k=>[k,new THREE.Group()]));Object.values(groups).forEach(g=>scene.add(g));
crayon=createCrayon({button:$('crayon'),canvas:renderer.domElement,camera,controls,scene,voxels:groups.voxels,
 state:()=>({map:activeMap,frame:current?.frame,paths:current?.paths,editable:isLocalScene()&&!!current&&!map2D.isActive()}),
 save:async(context,stroke,extend=false)=>{const result=await api('/api/paths'+(extend?'/'+encodeURIComponent(stroke.id):'')+'?scene='+encodeURIComponent(context.map),{frame:context.frame,stroke},extend?'PATCH':'POST');if(activeMap===context.map&&current?.frame===context.frame)current.paths=result;lastScene=null;return result;},
 remove:async(context,id)=>{const result=await api('/api/paths/'+encodeURIComponent(id)+'?scene='+encodeURIComponent(context.map),undefined,'DELETE');if(activeMap===context.map&&current?.frame===context.frame)current.paths=result;lastScene=null;return result;}});
for(const input of document.querySelectorAll('[data-layer]'))input.onchange=()=>{groups[input.dataset.layer].visible=input.checked;};

const boxGeometry=new THREE.BoxGeometry(1,1,1), cubeMaterial=new THREE.MeshStandardMaterial({roughness:.82,metalness:0});
function clear(group){for(const child of [...group.children]){group.remove(child);if(child.isInstancedMesh)child.dispose();child.traverse(o=>{if(o.geometry&&o.geometry!==boxGeometry)o.geometry.dispose();if(o.material&&o.material!==cubeMaterial){for(const m of Array.isArray(o.material)?o.material:[o.material]){m.map?.dispose();m.dispose();}}});}}
function label(text,color='#36594a'){
 const c=document.createElement('canvas');c.width=384;c.height=96;const ctx=c.getContext('2d');ctx.fillStyle='#fcfdf6';ctx.beginPath();ctx.roundRect(4,8,376,76,18);ctx.fill();ctx.font='500 30px system-ui';ctx.textAlign='center';ctx.fillStyle=color;ctx.fillText(text,192,56,356);const tex=new THREE.CanvasTexture(c);tex.colorSpace=THREE.SRGBColorSpace;const sprite=new THREE.Sprite(new THREE.SpriteMaterial({map:tex,depthTest:false}));sprite.scale.set(.65,.1625,1);return sprite;
}
const phoneTrails=createPhoneTrails(scene,text=>createNoteBubble(text,{peer:true}),peer=>cameraShape(fleetNetwork?.peers?.find(p=>p.peer_id===peer)?.catalog));
const compactTrail=createCompactPoseTrail(scene);
$('map-select').addEventListener('change',()=>compactTrail.reset());
async function refreshCompactTrail(){
 refreshPeer();
 grid.position.set(0,0,0);
 const selected=activeMap,peer=fleetNetwork?.peers?.find(p=>'peer-pose:'+p.peer_id===selected);
 const products=(peer?.catalog?.products||[]).filter(p=>p.metadata?.schema==='spatial-harness.compact-pose-contract/v1'&&(p.metadata.value?.lifetime??'sampled')==='sampled');
 $('scene-title').textContent=(peer?.name||peer?.peer_id||'Peer')+' · pose trail';
 $('frame-info').textContent='Native peer space · no Home alignment';
 $('portal-info').textContent='Last 300 received poses since opening · older ghosts are fainter';
 if(!products.length){compactTrail.reset();availability.hidden=false;availability.textContent='No compact pose Product advertised by this peer';return;}
 if(products.length!==1){compactTrail.reset();availability.hidden=false;availability.textContent='Multiple sampled pose Products; inspect the desired Product in Devices';return;}
 // Each Product has its own declared frame/session; do not mix unrelated outputs.
 const product=products[0];
 const query=new URLSearchParams({debug_pose:'true',sensor:peer.peer_id+'/'+product.manifest.product_id,expected_manifest:product.manifest_hash});
 try{
  const data=await api('/api/sensors?'+query);if(activeMap!==selected)return;
  const result=compactTrail.update(data);
  $('frame-info').textContent=result.frame+' · native coordinates (registry-oriented display)';
  $('observation-count').textContent=String(result.count);
  availability.hidden=false;availability.textContent=result.status+' · '+data.stream+' · '+product.manifest.product_id+(products.length>1?' · first pose Product':'');
 }catch(error){if(activeMap!==selected)return;compactTrail.reset();availability.hidden=false;availability.textContent='Pose trail unavailable · '+error.message;}
}
const semanticVolumes=createSemanticVolumes({scene,camera,renderer,controls,panel:panelBodies.volumes,api,label,clear,openPanel,priorityPickObjects:()=>[...selectable.filter(o=>o.userData.kind==='note'),...crayon.objects()]});
$('map-select').addEventListener('change',()=>semanticVolumes.reset());
let phonePresenceBusy=false;
const homePoseStatus=document.createElement('p');homePoseStatus.className='panel-hint';panelBodies.maps.append(homePoseStatus);
$('map-select').addEventListener('change',()=>{homePoseStatus.textContent='';phoneTrails.reset(null);});
async function refreshPhonePresence(){
 if(phonePresenceBusy||!isLocalScene()||!current)return;
 const map=activeMap,frame=current.frame;phonePresenceBusy=true;
 try{const poses=[],errors=[];
  for(const peer of fleetNetwork?.peers||[]){
   if(activeMap!==map||current?.frame!==frame)return;
   const products=(peer.catalog?.products||[]).filter(p=>p.metadata?.schema==='spatial-harness.compact-pose-contract/v1'&&(p.metadata.value?.lifetime??'sampled')==='sampled');
   if(!products.length)continue;
   const name=fleetNetwork?.aliases?.[peer.peer_id]||peer.name||peer.peer_id;
   if(products.length!==1){errors.push(name+': ambiguous pose Products');continue;}
   const product=products[0],q=new URLSearchParams({debug_pose:'true',sensor:peer.peer_id+'/'+product.manifest.product_id,expected_manifest:product.manifest_hash,home_frame:frame});
   try{const value=await api('/api/sensors?'+q),home=value.home_pose;
    if(home?.frame!==frame||home.tracking?.status!=='valid'||value.stream!=='live')continue;
    const p=home.tracking.pose,[x,y,z,w]=p.rotation_xyzw;
    poses.push({receivedAt:performance.now(),peer_id:peer.peer_id,name,clock:{session_id:value.resolved.contract.session_id+':'+home.alignment_timestamp_ns},capture_timestamp_ns:value.timestamp_ns,received_age_ms:value.age_ms,
     pose:{to_frame_id:frame,translation:p.translation,rotation_wxyz:[w,x,y,z]}});
   }catch(e){errors.push(name+': '+e.message);}
  }
  if(activeMap===map&&current?.frame===frame){const fresh=poses.map(p=>({...p,received_age_ms:p.received_age_ms+performance.now()-p.receivedAt})).filter(p=>p.received_age_ms<2000);phoneTrails.ingest(frame,fresh);homePoseStatus.textContent=errors.length?errors.join(' · '):fresh.length+' phones aligned to this Home scene';$('canvas').dataset.phonePoseStatus=errors.join('\n');}
 }catch{if(activeMap===map)phoneTrails.ingest(frame,[]);}finally{phonePresenceBusy=false;}
}
setInterval(refreshPhonePresence,200);
function build(data){
 map2D.update(data);
 semanticVolumes.update(activeMap,data);
 const first=current===null||current?.frame!==data.frame;data.receivedAt=performance.now();current=data;selectable=[];Object.values(groups).forEach(clear);
 crayon.update();
 const voxels=new THREE.InstancedMesh(boxGeometry,cubeMaterial,data.voxels.length);voxels.castShadow=true;voxels.receiveShadow=true;
 const matrix=new THREE.Matrix4(),color=new THREE.Color();
 data.voxels.forEach((v,i)=>{const scale=data.voxel_size;matrix.compose(new THREE.Vector3(...v.center),new THREE.Quaternion(...(data.voxel_rotation_xyzw||[0,0,0,1])),new THREE.Vector3(scale,scale,scale));voxels.setMatrixAt(i,matrix);const furnishing=data.synthetic_layout?.boxes.findLast(b=>v.center.every((n,j)=>n>=b.min[j]-.051&&n<=b.max[j]+.051));if(v.linear_rgb){color.setRGB(...v.linear_rgb,THREE.LinearSRGBColorSpace);}else color.set(furnishing?.color||(v.center[1]<0?'#c6cbbb':'#a7b4ac'));voxels.setColorAt(i,color);});
 voxels.userData.kind='voxels';groups.voxels.add(voxels);selectable.push(voxels);
 if(data.occupancy_tiles){
  const tiles=new THREE.InstancedMesh(boxGeometry,new THREE.MeshBasicMaterial(),data.occupancy_tiles.length);
  data.occupancy_tiles.forEach((tile,i)=>{
   matrix.compose(new THREE.Vector3(...tile.center),new THREE.Quaternion(),new THREE.Vector3(data.voxel_size*.96,.025,data.voxel_size*.96));
   tiles.setMatrixAt(i,matrix);color.set(tile.value===100?'#ee795e':tile.value===0?'#82b9a0':'#343c49');tiles.setColorAt(i,color);
  });
  tiles.userData.kind='occupancy';groups.voxels.add(tiles);selectable.push(tiles);
 }
 $('canvas').dataset.occupancyCount=String(data.occupancy_tiles?.length||0);
 for(const p of data.portals){
  const imported=p.anchor_id.startsWith('home:');const blue=data.local_map&&!imported;const code=imported?p.anchor_id.slice(5):p.anchor_id;
  const g=new THREE.Group();g.position.fromArray(p.pose_in_map.translation);const [w,x,y,z]=p.pose_in_map.rotation_wxyz;g.quaternion.set(x,y,z,w);
  const plate=new THREE.Mesh(new THREE.PlaneGeometry(p.side_length_m,p.side_length_m),new THREE.MeshBasicMaterial({color:blue?'#2477ec':'#e53935',wireframe:imported&&data.portals.some(o=>o.anchor_id===code),side:THREE.DoubleSide,depthTest:false,depthWrite:false}));plate.renderOrder=61;plate.userData={kind:'portal',portal:p};g.add(plate);selectable.push(plate);
  groups.portals.add(g);
 }
 if(first)phoneTrails.reset(data.frame);
 for(const volume of legacyVolumesForView(data)){
  const [w,h,d]=volume.dimensions;
  const group=new THREE.Group();group.position.fromArray(volume.position);group.rotation.y=volume.yaw;
  const geometry=new THREE.BoxGeometry(w,h,d);
  const body=new THREE.Mesh(geometry,new THREE.MeshBasicMaterial({color:0x22cddd,transparent:true,opacity:.25,depthWrite:false}));
  body.position.y=h/2;group.add(body);
  const edges=new THREE.LineSegments(new THREE.EdgesGeometry(geometry),new THREE.LineBasicMaterial({color:0x22cddd}));
  edges.position.y=h/2;group.add(edges);
  const tag=label(volume.name+' · '+volume.semantic_type,'#087b88');tag.position.set(0,h+.12,0);group.add(tag);
  groups.objects.add(group);
 }
 $('canvas').dataset.volumeCount=String(data.semantic_volumes?.frame===data.frame?data.semantic_volumes.items.length:(data.volumes||[]).length);
 renderer.domElement.dataset.legacyVolumeCount=String(legacyVolumesForView(data).length);
 for(const note of data.notes||[]){
  if(note.phone_pose)continue;
  const peer=Boolean(note.robot_pose||note.anchor_robot||note.id?.startsWith('robot-name:')||note.id?.startsWith('phone-name:'));
  const tag=createNoteBubble(noteTitle(note),{peer});tag.position.fromArray(note.position);
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
 $('empty').hidden=data.portals.length>0||data.voxels.length>0;if(activeMap==='home-live')$('empty').textContent='Waiting for Home to observe UCOF1J40N7O';else $('empty').textContent='No geometry yet';$('voxel-count').textContent=data.voxels.length.toLocaleString();$('observation-count').textContent=data.integrated_observations??'—';$('object-count').textContent=data.objects.length;
 for(const [peer,id] of [['iphone','phone-state'],['bracketbot','bot-state']]){const n=data.sources[peer];$(id).textContent=n?'Contributed':'Ready';$(id).classList.toggle('active',Boolean(n));}
 $('load').innerHTML=data.integrated_observations?'Reload test contributions <span>↗</span>':'Load two test peers <span>↗</span>';
 $('project').disabled=busy||!data.voxels.length;$('replay').disabled=busy||!data.sources.iphone;
 if(selectedSpatialNode){const {kind,id}=selectedSpatialNode;const node=data.landmarks?.[kind==='anchor'?'anchors':'landmarks']?.[id];if(node)showSpatialSelection(kind,node);else{selectedSpatialNode=null;$('selection').hidden=true;}}
 else if(selectedPathId){const path=data.paths?.items.find(p=>p.id===selectedPathId);if(path){if($('selection').dataset.pathId!==path.id)showPathSelection(path);}else{selectedPathId=null;crayon.select(null);$('selection').hidden=true;}}else if(selectedNoteId){const note=(data.notes||[]).find(n=>n.id===selectedNoteId);if(note){if($('selection').dataset.noteId!==note.id)showNoteSelection(note);}else{selectedNoteId=null;$('selection').hidden=true;}}else $('selection').hidden=true;if(!selectedNoteId&&!selectedPathId&&!selectedSpatialNode&&data.objects.length){const o=data.objects[0];showSelection(o.label,`[${o.position.map(v=>v.toFixed(2)).join(', ')}] m`,'Projected onto occupied voxel surface');}
 updateMapUI();if(data.occupancy_tiles){$('empty').hidden=data.occupancy_tiles.length>0;$('voxel-count').textContent=data.occupancy_tiles.length.toLocaleString();}if(first)fit();
}
function showPathSelection(path){
 showSelection(path.title,'Crayon path · '+path.segments.reduce((n,s)=>n+s.length,0)+' points','Shared scene path');
 selectedPathId=path.id;crayon.select(path.id);$('selection').dataset.pathId=path.id;
 if(isLocalScene())$('selection').append(pathDeleteButton(path));
}
function focusPath(path){
 const box=new THREE.Box3().setFromPoints(path.segments.flat().map(p=>new THREE.Vector3(...p))),center=box.getCenter(new THREE.Vector3());
 const distance=Math.max(2,box.getSize(new THREE.Vector3()).length());
 if(attachedCamera.isActive())attachedCamera.detach();
 camera.position.copy(center).add(new THREE.Vector3(distance*.4,distance,distance));controls.target.copy(center);controls.update();showPathSelection(path);
}
function pathDeleteButton(path){
 const map=activeMap,frame=current?.frame;
 const button=uiButton('Delete path',()=>deletePath(path,map,frame,button));
 button.dataset.deletePath=path.id;button.disabled=!!deletingPathId;return button;
}
async function deletePath(path,map=activeMap,frame=current?.frame,button=null){
  if(deletingPathId||activeMap!==map||current?.frame!==frame)return false;
  deletingPathId=path.id;if(button)button.disabled=true;
  const index=(current.paths?.items||[]).findIndex(p=>p.id===path.id);
  const status=document.createElement('div');status.setAttribute('role','status');if(button)button.after(status);else $('selection').append(status);status.textContent='Deleting…';
  try{
   const result=await api('/api/paths/'+encodeURIComponent(path.id)+'?scene='+encodeURIComponent(map),undefined,'DELETE');
   if(activeMap!==map||current?.frame!==frame)return false;
   current.paths=result;crayon.update();
   const stillSelected=selectedPathId===path.id;
   if(stillSelected){
    const next=result.items[Math.min(Math.max(index,0),result.items.length-1)];
    if(next){showPathSelection(next);}
    else{selectedPathId=null;crayon.select(null);$('selection').hidden=true;if(noteView==='path')noteView='list';}
   }
   renderNotes();lastScene=null;hideAr();return stillSelected;
  }catch(e){status.textContent=e.message;return false;}finally{
   deletingPathId=null;document.querySelectorAll('[data-delete-path]').forEach(b=>b.disabled=false);
  }
}
let backspaceQueue=0,backspaceDeleting=false;
window.addEventListener('keydown',async e=>{
 if(e.key!=='Backspace'||e.defaultPrevented||e.isComposing||e.ctrlKey||e.metaKey||e.altKey||isEditing(e.target)||isEditing(document.activeElement)||document.querySelector('dialog[open]')||!selectedPathId||!isLocalScene()||crayon.isActive())return;
 e.preventDefault();if(e.repeat)return;
 if(deletingPathId&&!backspaceDeleting)return;
 backspaceQueue++;if(backspaceDeleting)return;
 backspaceDeleting=true;const map=activeMap,frame=current?.frame;
 try{while(backspaceQueue>0){
  backspaceQueue--;if(map!==activeMap||frame!==current?.frame)break;
  const path=current?.paths?.items.find(p=>p.id===selectedPathId);
  if(!path||!await deletePath(path,map,frame))break;
 }}finally{backspaceQueue=0;backspaceDeleting=false;}
});
function closeUp(point){
 if(attachedCamera.isActive())attachedCamera.detach();
 const target=new THREE.Vector3(...point),direction=camera.position.clone().sub(target);
 if(direction.lengthSq()<1e-8)direction.set(0,.3,1);
 const position=target.clone().addScaledVector(direction.normalize(),2);
 cameraPan.start(position,target);
}
function focusNote(note){closeUp(note.position);showNoteSelection(note);}
function showNoteSelection(note){
 showSelection(noteTitle(note),`[${note.position.map(v=>v.toFixed(2)).join(', ')}] m`,note.robot_pose?'Live robot position':note.phone_pose?'Live phone position':note.anchor_surface?'Placed in Home':`${note.height_m} m above ${note.anchor_robot?note.text+' base':note.anchor_portal} · Home up axis`);
 selectedNoteId=note.id;$('selection').dataset.noteId=note.id;
 if(note.details?.description){const description=document.createElement('p');description.textContent=note.details.description;$('selection').append(description);}
 if(note.details?.photo)$('selection').append(notePhoto(note));
 if(note.anchor_robot||note.phone_pose||note.robot_pose||isRunner()||activeMap==='domain')return;
 const button=document.createElement('button');button.className='delete-note';button.textContent='Delete annotation';button.disabled=deletingNoteId===note.id;
 const status=document.createElement('div');status.setAttribute('role','status');
 button.onclick=async()=>{
  if(deletingNoteId)return;deletingNoteId=note.id;button.disabled=true;status.textContent='Deleting…';
  try{await api(noteUrl(note.id),undefined,'DELETE');
   if(selectedNoteId===note.id){selectedNoteId=null;$('selection').hidden=true;}
   hideAr();lastScene=null;await refresh();
  }catch(e){status.textContent=e.message;button.disabled=false;}
  finally{deletingNoteId=null;}
 };
 $('selection').append(button,status);
}
function focusPortal(p){const center=new THREE.Vector3(...p.pose_in_map.translation);camera.position.copy(center).add(new THREE.Vector3(.35,.65,.65));controls.target.copy(center);controls.update();showSelection(p.anchor_id,`[${p.pose_in_map.translation.map(v=>v.toFixed(4)).join(', ')}] m`,`${Math.round(p.side_length_m*100)} cm · ${p.anchor_id.startsWith('home:')?'Home reference aligned via measured portal':current?.local_map?'measured camera observation':'stored height, not mounting height'}`);}
function spatialPosition(kind,node){
 if(kind==='anchor')return node.pose?.translation;
 if(!node.parent_anchor)return node.position;
 const pose=current?.landmarks?.anchors?.[node.parent_anchor]?.pose;
 if(!pose||pose.from_frame_id!==node.frame)return null;
 const [w,x,y,z]=pose.rotation_wxyz;
 return new THREE.Vector3(...node.position).applyQuaternion(new THREE.Quaternion(x,y,z,w)).add(new THREE.Vector3(...pose.translation)).toArray();
}
function spatialCoordinates(position){return `[${position.map(v=>v.toFixed(4)).join(', ')}] m`;}
function spatialComponent(component){return component?`${component.peer_id} / ${component.component_id} (${component.manifest_hash})`:'Not recorded';}
function showSpatialSelection(kind,node,focus=false){
 const position=spatialPosition(kind,node),registry=current.landmarks;
 if(focus&&position){const center=new THREE.Vector3(...position);controls.target.copy(center);camera.position.copy(center).add(new THREE.Vector3(.5,.65,1));controls.update();}
 showSelection(`${kind==='anchor'?'Anchor':'Landmark'} · ${node.id}`,position?spatialCoordinates(position):'Map position unavailable',kind==='anchor'?'Full pose':'Position only');
 selectedSpatialNode={kind,id:node.id};const selection=$('selection');selection.dataset.spatialKind=kind;selection.dataset.spatialId=node.id;
 const fields=document.createElement('dl');fields.className='spatial-fields';
 function field(name,value){const term=document.createElement('dt'),description=document.createElement('dd');term.textContent=name;description.textContent=value;fields.append(term,description);}
 field('Type',node.recognition.landmark_type);field('Identity',node.recognition.identity);
 field('Map frame',registry.frame);
 if(kind==='anchor'){field('Local frame',node.pose.from_frame_id);field('Rotation (w, x, y, z)',node.pose.rotation_wxyz.map(v=>v.toFixed(4)).join(', '));const portal=current.portals?.find(p=>p.anchor_id===node.id);if(portal)field('Portal size',`${(portal.side_length_m*1000).toLocaleString()} mm encoded square`);}
 else{field('Stored position',spatialCoordinates(node.position));field('Position frame',node.frame);}
 const provenance=node.provenance||{};
 field('Detected by',spatialComponent(provenance.detected_by));field('Mapped by',spatialComponent(provenance.mapped_by));
 if(provenance.source_observation){const source=provenance.source_observation;field('Source observation',`${source.output.output_id} · sequence ${source.sequence} · ${source.timestamp_ns} ns`);}
 selection.append(fields);updateSpatialSelection();
 if(focus&&window.matchMedia('(max-width:720px)').matches)openPanel(null);
}
function updateSpatialSelection(){
 for(const button of spatialBrowser.querySelectorAll('[data-spatial-kind]'))button.setAttribute('aria-pressed',String(button.dataset.spatialKind===selectedSpatialNode?.kind&&button.dataset.spatialId===selectedSpatialNode?.id));
}
function renderSpatialLists(){
 const registry=current?.landmarks,query=$('spatial-search').value.trim().toLowerCase();
 const signature=JSON.stringify([activeMap,current?.frame,registry,query]);
 if(signature===spatialSignature){updateSpatialSelection();return;}spatialSignature=signature;
 $('spatial-status').textContent=!current?'Waiting for the selected scenegraph…':!registry?'This scenegraph does not publish anchors and landmarks.':'Full-pose anchors · position-only landmarks';
 for(const [kind,plural] of [['anchor','anchors'],['landmark','landmarks']]){
  const nodes=Object.values(registry?.[plural]||{}).sort((a,b)=>a.id.localeCompare(b.id));
  const matching=nodes.filter(node=>[node.id,node.recognition.landmark_type,node.recognition.identity,node.parent_anchor].filter(Boolean).join(' ').toLowerCase().includes(query));
  $(kind+'-count').textContent=query?`${matching.length} / ${nodes.length}`:String(nodes.length);
  const list=$(kind+'-list'),scroll=list.scrollTop;list.replaceChildren();
  for(const node of matching){
   const button=uiButton('',()=>showSpatialSelection(kind,node,true),'panel-row spatial-row');button.dataset.spatialKind=kind;button.dataset.spatialId=node.id;
   button.setAttribute('aria-label',`${kind==='anchor'?'Anchor':'Landmark'} ${node.id}`);
   const name=document.createElement('strong'),detail=document.createElement('small');name.textContent=node.id;detail.textContent=node.recognition.landmark_type;
   button.append(name,detail);list.append(button);
  }
  if(!matching.length){const empty=document.createElement('p');empty.className='panel-hint';empty.textContent=!registry?'Not available':query?'No matches':`No ${plural} in this scene`;list.append(empty);}
  list.scrollTop=scroll;
 }
 updateSpatialSelection();
}
function updateMapUI(){
 const local=Boolean(current?.local_map)||((isRunner()||activeMap==='home-live')&&!current);
 $('frame-info').textContent=activeMap==='home-live'?'Live origin: UCOF1J40N7O · shared Home frame':local?(current?.display_alignment?`Display frame: ${current.frame} · robot-native frame unchanged`:'Local origin: robot boot pose · fixed frame'): activeMap==='domain'?'Stored domain frame · OpenGL · Y up · meters':'Home origin: UCOF1J40N7O · OpenGL · Y up · meters';
 $('portal-info').textContent=activeMap==='home-live'?'Blue: measured portals. Each robot calibrates through its own observed canonical portal.':local?'Blue: camera observations. Red: Home references aligned through a shared portal. The robot boot origin stays fixed.':'Stored Y = 0 for every portal. Actual mounting heights are unverified.';
 $('portal-swatch').style.background=local?'#2477ec':'#e53935';
 const domain=activeMap==='domain';$('domain-info').hidden=false;$('demo-controls').hidden=domain||!current?.demo;
 $('scene-title').textContent=activeMap==='home-live'?'Home live map':isRunner()?runnerDisplay()+' scenegraph':domain?'13 stored domain portals':(current?.demo?'Domain + synthetic scene':current?.environment_name||'Canonical portals');
 $('map-authority').textContent=isRunner()?runnerDisplay()+' mapper':'Home Server';
 $('scene-badge').textContent=activeMap==='home-live'?'HOME LIVE MAP':isRunner()?'GALBOT SCENEGRAPH':domain?'STORED DOMAIN POSES':(current?.demo?'DOMAIN + SYNTHETIC SCENE':'CANONICAL PORTALS');
 $('scene-note').textContent=activeMap==='home-live'?'Mapped on Home from both robots’ raw observations, each calibrated through known portal poses. UCOF1J40N7O defines the fixed origin.':local?(current?.display_alignment?`Display aligned via ${current.display_alignment.portal}. Robot data remains in its native frame.`:current?.frame_transform?.validity==='stale'?'Robot-native map. Portal transform is stale; waiting for a fresh tracked observation.':'Robot-native map. Waiting for a shared portal transform from its Catalog.'):domain?'Supplied poses and sizes. Grid is a coordinate reference, not a measured floor.':(current?.demo?'Illustrative L-shaped workspace fitted to the marker footprint. Furniture and walls are synthetic; stored portal poses are unchanged.':'Canonical portal record with supplied poses and sizes. Live geometry is in Home live map.');
 document.querySelector('.flow').hidden=domain||!current?.demo;
 $('demo-view').hidden=!current?.demo;
 document.querySelector('#inspect-snapshot').href=document.querySelector('.export').href=activeMap==='home-live'?'/api/home-live-snapshot':isRunner()?runnerUrl('/api/runner-snapshot'):domain?'/api/domain-snapshot':'/api/snapshots';
 ground.visible=Boolean(current?.demo);scene.fog=null;grid.scale.setScalar(3);grid.position.x=local?0:-5;grid.position.z=local?0:3;
 const list=$('portal-list');list.replaceChildren();{for(const p of (current?.portals||[]).filter(p=>p.anchor_id.length===11||p.anchor_id.startsWith('home:'))){const button=document.createElement('button');button.className='portal-row';button.textContent=p.anchor_id;const size=document.createElement('span');size.textContent=`${Math.round(p.side_length_m*100)} cm`;button.append(size);button.onclick=()=>focusPortal(p);list.append(button);}}
 list.hidden=Boolean(current?.landmarks);renderSpatialLists();
 if(isLocalScene()) {
  $('scene-title').textContent=current?.environment_name||'Local scenegraph';
  $('scene-badge').textContent='SAVED LOCAL SCENEGRAPH';
  $('frame-info').textContent=(current?.frame||'')+' · OpenGL · meters';
  $('portal-info').textContent=current?.frame_description||'';
  $('scene-note').textContent='Independent Home scenegraph. Pair your phone with this scene and scan its Portal to share depth and annotations. Grid assumes a flat floor at Y = 0.';
  $('mapping-status').textContent=current?.mobile_enabled?'Phone mapping · '+(current.integrated_observations||0)+' scans':'Saved reference · phone mapping unavailable';
  $('mapping-detail').textContent=(current?.portals.length||0)+' Portal(s) · observations and notes remain separate from other scenes';
  document.querySelector('#inspect-snapshot').href=document.querySelector('.export').href=localSceneUrl()+'/snapshot';
  grid.position.x=0;grid.position.z=0;
 }
 if(activeMap.startsWith('peer-scene:') && current) {
  $('scene-title').textContent=current.product_id||'Peer scenegraph';
  $('map-authority').textContent=current.authority_peer_id;
  $('scene-badge').textContent=current.local_map?'ROBOT-LOCAL MAP':'PEER SCENEGRAPH';
  $('frame-info').textContent=current.frame+' · '+current.convention;
  $('portal-info').textContent=current.local_map?'Independent robot frame · not merged into Home':'Publisher-owned scenegraph';
  $('scene-note').textContent=current.local_mapping?.detail||'Discovered SDK scenegraph product · read-only';
  $('mapping-status').textContent=(current.integrated_observations||0)+' scans integrated';
  $('mapping-detail').textContent='Latest retained map from the publishing peer';
  if(current.product_id==='phone-local-voxels'){
   $('scene-title').textContent='Phone · local voxel map';
   $('scene-badge').textContent='PHONE-LOCAL MAP';
   $('portal-info').textContent='Independent phone AR session frame · not merged into Home';
   $('scene-note').textContent='Phone-owned 10 cm voxels · read directly from its SDK Catalog Product';
   $('mapping-status').textContent='Phone-local depth mapping';
  }
  document.querySelector('#inspect-snapshot').href=document.querySelector('.export').href=peerSceneUrl(activeMap);
  if(current.layer_type==='occupancy_2d'){
   $('scene-badge').textContent='OCCUPANCY 2D';
   $('scene-note').textContent='Robot-owned 10 cm occupancy · first meter above the flat floor · 2 m cells · unaligned';
   $('mapping-status').textContent='Occupancy · '+current.occupancy_status;
   const counts=current.occupancy_counts;
   $('mapping-detail').textContent=counts.occupied+' occupied · '+counts.free+' free · '+counts.unknown+' unknown';
   $('portal-info').textContent='Coral: occupied · green: free · slate: unknown. Not merged into Home.';
  }
 }
 if(activeMap.startsWith('phone:')) {
  $('scene-title').textContent='Phone · local voxel map';
  $('map-authority').textContent='Phone peer · AR depth';
  $('scene-badge').textContent='UNALIGNED PHONE MAP';
  $('frame-info').textContent='Phone AR session · OpenGL · Y up · meters';
  $('portal-info').textContent='No Portal calibration. Not merged into Home.';
  $('scene-note').textContent='Real depth hits accumulated into 10 cm voxels on the phone. Tracking loss starts a new map. Unknown space is not free space.';
  $('mapping-status').textContent='Phone-local depth mapping';
  $('mapping-detail').textContent='Unaligned · read-only · removed after 10 seconds without updates';
  document.querySelector('#inspect-snapshot').href=document.querySelector('.export').href='/api/phone-local-scene?peer='+encodeURIComponent(activeMap.slice(6));
 }
 if(isSimulation()){
  const occupancy=activeMap==='sim-occupancy';
  $('frame-info').textContent='Synthetic room · flat floor Y = 0 · meters';
  $('portal-info').textContent=occupancy?'Coral: occupied (100) · green: free (0) · slate: unknown (-1). Blank areas are also unknown.':'10 cm voxels · synthetic walls, floor and central obstacle';
  $('scene-title').textContent=occupancy?'Simulated robot · occupancy 2D':'Simulated robot · voxels';
  $('map-authority').textContent=occupancy?'Home occupancy mapper':'Simulated robot peer';
  $('scene-badge').textContent='SIMULATION · '+(occupancy?'OCCUPANCY 2D':'VOXELS');
  const robotCount=Object.keys(current?.local_mapping?.runners||{}).length;
  $('scene-note').textContent=occupancy?'Accumulated occupancy from robot observations. Outside view retains its last evidence; obstacles clear only when observed free. No time decay; mapper: 1 Hz.':`${robotCount||1} robot(s) producing live voxel observations with a 1.5 m footprint. A low block shuttles every 12 seconds.`;
  $('mapping-status').textContent='Simulation · '+(current?.simulation_status||'starting')+(occupancy?' · '+(current?.occupancy_status||'waiting'):'');
  $('mapping-detail').textContent=current?.simulation_detail||'Waiting for the synthetic peer';
  document.querySelector('#inspect-snapshot').href=document.querySelector('.export').href='/api/simulation/'+(occupancy?'occupancy':'voxels')+'?snapshot=true';
  grid.position.x=0;grid.position.z=0;
 }
}
$('map-select').onchange=async()=>{activeMap=$('map-select').value;selectedPathId=null;crayon.select(null);selectedNoteId=null;selectedSpatialNode=null;lastScene=null;current=null;crayon.update();hideRobots();selectable=[];Object.values(groups).forEach(clear);$('selection').hidden=true;updateMapUI();$('scene-title').textContent='Loading selected scene…';$('voxel-count').textContent='—';$('observation-count').textContent='—';$('object-count').textContent='—';try{await refresh();}catch{$('scene-title').textContent=isRunner()?'Waiting for Galbot scenegraph':'Scene unavailable';}};
function showSelection(title,body,note){selectedPathId=null;crayon?.select(null);delete $('selection').dataset.pathId;selectedNoteId=null;selectedSpatialNode=null;updateSpatialSelection();const el=$('selection');delete el.dataset.noteId;delete el.dataset.spatialId;delete el.dataset.spatialKind;el.replaceChildren();const strong=document.createElement('strong');strong.textContent=title;const line=document.createElement('div');line.textContent=body;const small=document.createElement('small');small.textContent=note;el.append(strong,line,small);el.hidden=false;}
let pointerStart;
renderer.domElement.addEventListener('pointerdown',e=>{pointerStart=[e.clientX,e.clientY];});
renderer.domElement.addEventListener('pointerup',e=>{
 if(e.semanticVolume)return;
 if(e.button!==0)return;
 if(!current||!pointerStart||Math.hypot(e.clientX-pointerStart[0],e.clientY-pointerStart[1])>4)return;
 const r=renderer.domElement.getBoundingClientRect(), ray=new THREE.Raycaster();ray.setFromCamera(new THREE.Vector2((e.clientX-r.left)/r.width*2-1,-(e.clientY-r.top)/r.height*2+1),camera);
 const hits=ray.intersectObjects([...selectable,...crayon.objects()].filter(o=>{for(let p=o;p;p=p.parent){if(!p.visible)return false;}return true;}));if(!hits.length){selectedPathId=null;crayon.select(null);selectedNoteId=null;selectedSpatialNode=null;updateSpatialSelection();$('selection').hidden=true;return;}const h=hits.find(h=>h.object.userData.kind==='note')||hits.find(h=>h.object.userData.kind==='path')||hits.find(h=>h.object.userData.kind==='portal')||hits[0],d=h.object.userData;
 if(d.kind==='voxels'){const v=current.voxels[h.instanceId];showSelection('Occupied voxel',`[${v.center.map(n=>n.toFixed(2)).join(', ')}] m`,`Evidence ${v.evidence} · 10 cm cell`);}
 if(d.kind==='occupancy'){const t=current.occupancy_tiles[h.instanceId];showSelection('Occupancy · '+(t.value===100?'occupied':t.value===0?'free':'unknown'),`[${t.center.map(n=>n.toFixed(2)).join(', ')}] m`,`Value ${t.value} · 10 cm sample · retrieved in 2 m cells`);}
 if(d.kind==='path'){showPathSelection(d.path);noteView='path';openPanel('notes');renderNotes();return;}
 if(d.kind==='note')showNoteSelection(d.note);
 if(d.kind==='object'){const o=d.object;showSelection(o.label,`[${o.position.map(n=>n.toFixed(2)).join(', ')}] m`,`Source: ${o.source_peer}`);}
 if(d.kind==='portal'&&(d.portal.anchor_id.length===11||d.portal.anchor_id.startsWith('home:'))){showSelection(d.portal.anchor_id,'Portal','Double-click to approach');return;}
 if(d.kind==='portal')showSelection(`Portal ${d.portal.payload.split(':').at(-1)}`,`${d.portal.side_length_m.toFixed(2)} m encoded size`,'Manually supplied · no backend lookup');
});
renderer.domElement.addEventListener('dblclick',e=>{
 if(e.button!==0||!current||crayon.isActive()||map2D.isActive())return;
 const r=renderer.domElement.getBoundingClientRect(),ray=new THREE.Raycaster();
 ray.setFromCamera(new THREE.Vector2((e.clientX-r.left)/r.width*2-1,1-(e.clientY-r.top)/r.height*2),camera);
 const hits=ray.intersectObjects(selectable.filter(o=>{for(let p=o;p;p=p.parent)if(!p.visible)return false;return true;}));
 const note=hits.find(h=>h.object.userData.kind==='note');
 if(note){focusNote(note.object.userData.note);return;}
 const volume=semanticVolumes.pick(ray);if(volume){closeUp(volume.point.toArray());return;}
 const hit=hits[0];if(hit)closeUp(hit.point.toArray());
});
async function api(path,body,method){if(path==='/api/chat'&&body){camera.updateMatrixWorld();const right=map2D.isActive()?[1,0,0]:new THREE.Vector3().setFromMatrixColumn(camera.matrixWorld,0).toArray();body={...body,viewer:{frame:current?.frame||'',mode:map2D.isActive()?'2d':'3d',screen_right:right,camera_position:camera.position.toArray(),camera_target:controls.target.toArray()}};}const r=await fetch(path,body?{method:method||'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}:method?{method}:undefined);const text=await r.text();let result;try{result=JSON.parse(text);}catch{throw new Error(text);}if(!r.ok){const error=new Error(result.error||text);error.status=r.status;throw error;}return result;}
function peerSceneUrl(map){return '/api/sensors?scene=true&sensor='+encodeURIComponent(map.slice(11));}
async function refresh(){if(activeMap.startsWith('peer-pose:')){if(refreshing)return;refreshing=true;try{await refreshCompactTrail();}finally{refreshing=false;}return;}if(refreshing)return;refreshing=true;refreshPeer();if(!isSimulation())refreshMapping();try{const selected=activeMap;const data=await api(isLocalScene(selected)?localSceneUrl(selected):selected.startsWith('peer-scene:')?peerSceneUrl(selected):selected.startsWith('phone:')?'/api/phone-local-scene?peer='+encodeURIComponent(selected.slice(6)):isSimulation(selected)?'/api/simulation/'+selected.slice(4):selected==='home-live'?'/api/home-live-scene':isRunner(selected)?runnerUrl('/api/runner-scene',selected):selected==='domain'?'/api/domain-scene':'/api/scene');if(selected!==activeMap)return;const content=JSON.stringify({...data,remote_status:undefined});if(content!==lastScene){lastScene=content;build(data);}else {data.receivedAt=performance.now();current=data;}homeOffline=false;renderMapRows();availability.hidden=!(selected==='sim-occupancy'&&data.simulation_status!=='running');if(!availability.hidden)availability.textContent='Retained on Home · robot publishers offline · no new observations';$('connection').textContent='Home Server online';}catch(e){if(!e.status){markOffline();}else{availability.hidden=false;availability.textContent='Scene unavailable · '+e.message;if(activeMap==='sim-voxels'||activeMap.startsWith('peer-scene:')){Object.values(groups).forEach(clear);hideRobots();current=null;lastScene=null;}}$('connection').textContent=e.status?'Home online · scene unavailable':'Home offline';throw e;}finally{refreshing=false;}}
async function action(fn){if(busy)return;busy=true;document.querySelectorAll('#load,#project,#replay').forEach(b=>b.disabled=true);$('notice').classList.remove('error');$('notice').textContent='Integrating…';try{await fn();await refresh();}catch(e){$('notice').textContent=e.message;$('notice').classList.add('error');}finally{busy=false;$('load').disabled=false;$('project').disabled=!current?.voxels.length;$('replay').disabled=!current?.sources.iphone;}}
$('load').onclick=()=>action(async()=>{const f=await api('/api/fixtures');for(const c of f.contributions)await api('/api/contributions',c);$('notice').textContent='Two scans aligned through Portal A. Ready to project a detection.';});
$('project').onclick=()=>action(async()=>{const f=await api('/api/fixtures');const result=await api('/api/detections',f.detection);$('notice').textContent=result.applied?'Book placed at the first occupied voxel hit.':'This detection is already placed. No duplicate object created.';});
$('replay').onclick=()=>action(async()=>{const f=await api('/api/fixtures');const r=await api('/api/contributions',f.contributions[0]);$('notice').textContent=r.applied?'Contribution accepted.':'Replay ignored. Voxel evidence and object count are unchanged.';});
new ResizeObserver(()=>{const {width,height}=$('canvas').getBoundingClientRect();renderer.setSize(width,height);camera.aspect=width/height;camera.updateProjectionMatrix();}).observe($('canvas'));
renderer.setAnimationLoop(now=>{if(!cameraPan.tick(now)&&!attachedCamera.isActive())controls.update();ghostCamera.tick(now);semanticVolumes.tick();if(!current)phoneTrails.reset(null);const phones=phoneTrails.tick();phoneTrails.group.visible=groups.notes.visible;$('canvas').dataset.phoneGhostCount=String(phones.ghosts);$('canvas').dataset.phoneLiveCount=String(phones.live);renderer.render(scene,camera);});
refresh().catch(e=>{$('notice').textContent=e.message;$('notice').classList.add('error');});setInterval(()=>{if(!busy)refresh().catch(()=>{});},500);

function updatePairingDownload(){const link=$('pairing-download');link.href=isLocalScene()?localSceneUrl()+'/connection':'/api/home-connection';link.download=isLocalScene()?activeMap.slice(6)+'-connection.json':'home-connection.json';link.textContent=isLocalScene()?'Download phone connection for selected scene ↗':'Download Xmart connection ↗';}
async function refreshPeer(){try{const peer=await api('/api/peer');$('p2p-status').textContent=peer.enabled?'P2P · '+peer.status:'P2P disabled';$('p2p-detail').textContent=peer.connection?peer.connection.peer_id:peer.detail||'Configure a peer to join the network.';$('pairing-download').hidden=peer.status!=='Ready';updatePairingDownload();fleetNetwork=peer;renderFleetRows();}catch{$('p2p-status').textContent='P2P unavailable';$('pairing-download').hidden=true;fleetNetwork=null;renderFleetRows();}}

// Dynamic telemetry is independent of scene updates and synthetic peer scans.
const robots=new Map(),poseUpdates=new Map(),robotVolumes=new Map(),bodyVolumePeers=new Set();
function usesBodyVolume(key){
 const peer=key==='selected'?(current?.authority_peer_id||(activeMap.startsWith('runner:')?activeMap.slice(7):primaryRunnerPeer)):key;
 return bodyVolumePeers.has(peer)||(key==='selected'&&(current?.source_frame||current?.frame||'').startsWith('bracketbot-local:'));
}
const poseColors=['#e49b32','#985cdb','#219b96'];
let runner=null,runnerBusy=false,lastRunnerReply=0;
function hideRobots(){for(const glyph of robots.values())glyph.group.visible=false;}
// Robot models drawn instead of the pose box, by the robot's native map family.
function robotModel(frame){return frame?.startsWith('bracketbot-local:')?'bracketbot':null;}
// Swap the box glyph for the robot's URDF model. The model's root is in ROS body
// axes like the glyph; the box stays as the fallback while loading, on failure,
// and for robots without a model.
function setGlyphModel(glyph,model){
 if(glyph.model===model)return;
 glyph.model=model;
 if(glyph.robot){glyph.group.remove(glyph.robot.root);glyph.robot=null;}
 glyph.body.visible=true;
 if(model!=='bracketbot')return;
 createRobotModel().then(robot=>{
  if(glyph.model!=='bracketbot'||glyph.robot)return;
  glyph.robot=robot;glyph.group.add(robot.root);glyph.body.visible=false;applyJoints(glyph);applyRobotHealth(glyph,glyph.health);
 }).catch(e=>console.warn('Robot model unavailable; keeping pose glyph',e));
}
// URDF joint positions published next to the pose; joints left out keep their last value.
function applyJoints(glyph){
 if(!glyph.robot||!glyph.joints)return;
 for(const [name,value] of Object.entries(glyph.joints))glyph.robot.setJoint(name,value);
}
// A Home robot presence entry (robot.base pose in the scene) as a glyph pose in
// ROS body axes. Bracketbot declares robot.base Z-up/X-right/Y-forward, so its
// ROS body (URDF root, X forward) is robot.base turned +90 degrees about Z.
const BRACKETBOT_BASE_FROM_ROS=new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(0,0,1),Math.PI/2);
function presenceGlyphPose(robot){
 const p=robot?.pose;if(!p?.translation||!p?.rotation_wxyz)return null;
 const [w,x,y,z]=p.rotation_wxyz,q=new THREE.Quaternion(x,y,z,w);
 if(robotModel(robot.native_pose?.to_frame_id)==='bracketbot')q.multiply(BRACKETBOT_BASE_FROM_ROS);
 return {translation:p.translation,rotation_xyzw:[q.x,q.y,q.z,q.w]};
}
function glyphFor(key,name){
 let glyph=robots.get(key);
 if(!glyph){
  const group=new THREE.Group();scene.add(group);
  const color=poseColors[robots.size%poseColors.length];
  const material=new THREE.MeshStandardMaterial({color,transparent:true,opacity:.82,roughness:.6,depthTest:false});
  // Robot-local ROS body axes: +X forward, +Y left, +Z up; illustrative pose glyph.
  const body=new THREE.Mesh(new THREE.BoxGeometry(.42,.32,.18),material);body.position.z=.11;body.renderOrder=5;group.add(body);
  const arrow=new THREE.ArrowHelper(new THREE.Vector3(1,0,0),new THREE.Vector3(0,0,.25),.8,color,.2,.12);arrow.traverse(o=>{if(o.material){o.material.depthTest=false;o.renderOrder=6;}});group.add(arrow);
  glyph={group,material,arrow,color,body};robots.set(key,glyph);
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
  glyph.tag.visible=activeMap!=='home-live'&&!isLocalScene();  // local scenes name robots with a note bubble
  glyph.state=state;glyph.group.visible=Boolean(state.pose)&&$('runner-layer').checked&&!usesBodyVolume(entry.key);
  if(state.pose){glyph.group.position.fromArray(state.pose.translation);glyph.group.quaternion.fromArray(state.pose.rotation_xyzw);}
  setGlyphModel(glyph,state.model||null);
  if(state.joints&&state.joints!==glyph.joints){glyph.joints=state.joints;applyJoints(glyph);}
  glyph.health=state.health||null;applyRobotHealth(glyph,glyph.health);
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
 $('focus-runner').textContent='Focus '+runnerDisplay()+' ↗';$('focus-runner').disabled=!state.pose;
 $('focus-runner').onclick=()=>focusPose(runnerDisplay(),state);
 const glyph=robots.get('selected');
 $('runner-card').dataset.position=state.pose?JSON.stringify(glyph.group.position.toArray()):'';
 $('runner-card').dataset.rotation=state.pose?JSON.stringify(glyph.group.quaternion.toArray()):'';
}
// Scene responses retain the physical body pose and provide a separate SDK-
// converted pose for this +X-forward glyph. Legacy ROS telemetry/simulations
// already supply glyph-compatible poses.
function robotGlyphPose(sample){return sample?.glyph_pose??sample?.pose??null;}
function showHomeRunners(local,fresh){
 const now=performance.now();
 const entries=Object.entries(local.runners).map(([key,value])=>{
  const sample=value.robot_pose,stamp=sample?JSON.stringify([sample.clock,sample.capture_timestamp_ns]):null;
  let update=poseUpdates.get(key);if(!update||update.stamp!==stamp){update={stamp,at:now};poseUpdates.set(key,update);}
  const pose=robotGlyphPose(sample);
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
$('runner-layer').onchange=()=>{refreshRunner();};
async function refreshRunner(){
 if(isLocalScene()){
  // Robots aligned to this scene, from Home's authenticated robot presence.
  const present=(current?.notes||[]).map(n=>n.robot_pose).filter(r=>r?.pose&&r.peer_id);
  if(!present.length){hideRobots();$('runner-status').textContent='No robot aligned to this scene';return;}
  renderRunners(present.map(r=>({key:'presence:'+r.peer_id,name:r.name||'Robot '+r.peer_id.slice(-6),
   state:{status:(r.received_age_ms??0)<3000?'live':'stale',pose:presenceGlyphPose(r),model:robotModel(r.native_pose?.to_frame_id),joints:r.joints?.positions,health:r.health}})));
  $('runner-status').textContent=present.length+' robot(s) aligned to this scene';
  return;
 }
 if(activeMap.startsWith('peer-scene:')){
  const pose=robotGlyphPose(current?.local_mapping?.robot_pose);
  showRunner({status:pose&&performance.now()-(current?.receivedAt||0)<5000?'live':'stale',pose:pose||null,model:robotModel(current?.frame),joints:current?.local_mapping?.robot_joints?.positions,health:current?.local_mapping?.robot_health,detail:'Pose in the publishing robot’s map frame'});return;
 }
 if(isSimulation()){
  const local=current?.local_mapping;
  const fresh=current?.simulation_status==='running'&&performance.now()-(current?.receivedAt||0)<5000;
  if(local?.runners){
   renderRunners(Object.entries(local.runners).map(([key,r])=>({key,name:r.name,state:{status:fresh?'live':'stale',pose:robotGlyphPose(r.robot_pose)}})));
   $('runner-status').textContent=Object.keys(local.runners).length+' simulated robot(s) · '+(fresh?'live':'stale');
   $('runner-card').dataset.status=fresh?'live':'stale';return;
  }
  showRunner({status:fresh?'live':'stale',pose:robotGlyphPose(local?.robot_pose),detail:'Synthetic patrol pose · not robot telemetry'});return;
 }
 if((isRunner()||activeMap==='home-live')) {
  const local=current?.local_mapping, remote=current?.remote_status;
  const fresh=remote?.status==='connected' && remote.age_ms<10000 && performance.now()-(current?.receivedAt||0)<5000;
  if(activeMap==='home-live'&&local?.runners){showHomeRunners(local,fresh);return;}
  showRunner({status:fresh&&local?.robot_pose?'live':'stale',pose:robotGlyphPose(local?.robot_pose),model:robotModel(current?.frame),joints:local?.robot_joints?.positions,health:local?.robot_health,detail:local?(activeMap==='home-live'?'Pose in Home’s observed portal frame':current?.display_alignment?'Display pose uses the Catalog transform; robot pose remains native':'Pose in the robot boot frame'):'Waiting for independent robot pose'});return;
 }
 if(runnerBusy)return;runnerBusy=true;try{const state=await api('/api/runner-pose');lastRunnerReply=performance.now();if(!(isLocalScene()||isRunner()||isSimulation()||activeMap==='home-live'||activeMap.startsWith('peer-scene:')))showRunner(state);}catch{if(!(isLocalScene()||isRunner()||isSimulation()||activeMap==='home-live'||activeMap.startsWith('peer-scene:')))showRunner({...runner,status:'unavailable',pose:null,detail:'Home telemetry connection lost'});}finally{runnerBusy=false;}}
runnerCatalog();refreshRunner();setInterval(refreshRunner,250);
setInterval(()=>{if(!(isLocalScene()||isRunner()||isSimulation()||activeMap==='home-live'||activeMap.startsWith('peer-scene:'))&&lastRunnerReply&&performance.now()-lastRunnerReply>3000&&runner?.status==='live')showRunner({...runner,status:'stale',detail:'Home telemetry has stopped updating'});},500);

async function refreshMapping(){if(isLocalScene()||activeMap.startsWith('phone:')||activeMap.startsWith('peer-scene:'))return;if(activeMap==='home-live'){try{const state=await api('/api/home-live-status');$('mapping-status').textContent='Home map · '+state.status;$('mapping-detail').textContent=state.detail||'';}catch{$('mapping-status').textContent='Home map · unavailable';}return;}if(activeMap==='demo'&&!current?.demo){$('mapping-status').textContent='Canonical reference';$('mapping-detail').textContent='13 supplied portal poses and physical sizes';return;}if(isRunner()){try{const sources=await api('/api/scene-sources');const remote=activeMap.startsWith('runner:')?(sources.runners||[]).find(r=>r.peer_id===activeMap.slice(7))||{status:'unavailable'}:sources;$('mapping-status').textContent='Galbot map · '+remote.status;$('mapping-detail').textContent=remote.detail||'';}catch{$('mapping-status').textContent='Galbot map · unavailable';}return;}try{const mapping=await api('/api/mapping');$('mapping-status').textContent='LiDAR · '+mapping.status+(mapping.rgbd?' / RGB-D · '+mapping.rgbd.status:'');$('mapping-detail').textContent=(mapping.detail||'')+(mapping.rgbd?' · '+(mapping.rgbd.detail||`${mapping.rgbd.colored_voxels} colored voxels`):'');$('mapping-card').dataset.status=mapping.status;}catch{$('mapping-status').textContent='LiDAR · unavailable';}}

// Prefer live topography; canonical scenes can also use the reference floor.
const labelDialog=$('label-dialog'),labelInput=$('label-dialog-text'),labelStatus=$('label-dialog-status'),labelSave=$('label-dialog-save');
let placement=null,placementGeneration=0,displayedCamera=null,labelFields=null;
function placementError(message){labelStatus.textContent=message;labelSave.disabled=true;}
async function beginLabel(request,error){
 if(request?.source!=='camera'&&activeMap.startsWith('peer-scene:')) {availability.hidden=false;availability.textContent='Peer maps are read-only.';return;}
 if(request?.source!=='camera'&&activeMap.startsWith('phone:')) {availability.hidden=false;availability.textContent='Phone-local maps are unaligned and read-only.';return;}
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
 if(crayon.isActive())return;
 if(!current){beginLabel(null,'Wait for Home’s map to load.');return;}
 const rect=renderer.domElement.getBoundingClientRect(),ray=new THREE.Raycaster();
 ray.setFromCamera(new THREE.Vector2((e.clientX-rect.left)/rect.width*2-1,1-(e.clientY-rect.top)/rect.height*2),camera);
 beginLabel({source:'scene',frame:current.frame,origin:ray.ray.origin.toArray(),direction:ray.ray.direction.toArray()});
});
$('sensor-stage').addEventListener('contextmenu',e=>{e.preventDefault();e.stopPropagation();
 const f=displayedCamera;
 if(!f?.token||Date.now()-f.receivedAt>10000){beginLabel(null,f?.error||'Waiting for a fresh capture-matched camera pose. Try again when calibrated video arrives.');return;}
 beginLabel({source:'camera',camera_token:f.token,pixel:videoPresentation.pixel(e.clientX,e.clientY)});
});

// Camera previews are independent of the selected map and never control the robot.
const sensorSelect=document.querySelector('#sensor-select'),pip=document.querySelector('#sensor-pip'),sensorCanvas=document.querySelector('#sensor-image');
const pipWindow=createPipWindow(pip);
const videoPresentation=createVideoPresentation($('sensor-stage'),pip.querySelector('.sensor-pip-heading'),()=>{pipWindow.fit();arOverlay?.setRotation(Number($('sensor-stage').dataset.rotation)||0);});
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
async function applyChatCamera(actions,map){
 return applyChatActions(actions,{scene:map,context:()=>({scene:activeMap,frame:current?.frame}),apply:async(action,guard)=>{
  if(['attach_camera','detach_camera','move_camera'].includes(action.type))cameraPan.cancel();
  if(action.type==='attach_camera')return attachedCamera.attach(action);
  if(action.type==='detach_camera'){
   if(action.scene!==activeMap||action.frame!==current?.frame)throw Error('Selected scene changed; ask again');
   return attachedCamera.detach();
  }
  if(action.type==='move_camera'){const result=moveMapCamera(action,{scene:activeMap,frame:current?.frame,camera,controls,map2D});attachedCamera.detach();return result;}
  const catalog=await api('/api/sensors');
  guard();
  const sensor=catalog.sensors.find(s=>s.id===action.sensor_id&&s.peer_id===action.peer_id);
  if(!sensor)throw Error('Camera is no longer available');
  let option=Array.from(sensorSelect.options).find(o=>o.value===sensor.id);
  if(!option){option=new Option(sensor.label,sensor.id);option.dataset.peer=sensor.peer_id;sensorSelect.append(option);}
  arAvailable.set(sensor.id,sensor.ar_available===true);
  if(sensorSelect.value!==sensor.id||pip.hidden){sensorSelect.value=sensor.id;sensorSelect.dispatchEvent(new Event('change'));}
  return 'Camera PiP opened · waiting for live frames.';
 }});
}
arToggle.addEventListener('change',restartSensor);
setInterval(()=>{if(!labelDialog.open&&arToggle.checked&&arLastSuccess&&Date.now()-arLastSuccess>3000){hideAr();arStatus.hidden=false;arStatus.textContent='AR paused · camera frames are stale';}},500);
async function sensorCatalog(){
 try{
  const response=await fetch('/api/sensors');const data=await response.json();if(!response.ok)throw Error(data.error);
  const selected=sensorSelect.value;const options=[new Option('None','')];
  arAvailable.clear();
  for(const s of data.sensors){arAvailable.set(s.id,s.ar_available===true);const option=new Option(s.label+(s.ar_available?' · AR':''),s.id);option.dataset.peer=s.peer_id;options.push(option);}
  const stillAvailable=data.sensors.some(s=>s.id===selected);
  sensorSelect.replaceChildren(...options);sensorSelect.value=stillAvailable?selected:'';updateArAvailability();
  if(selected&&!stillAvailable)sensorSelect.dispatchEvent(new Event('change'));
  document.querySelector('#sensor-catalog-status').textContent=data.sensors.length?`${data.sensors.length} published camera feeds`:'No camera feeds available · retrying';
 }catch(e){document.querySelector('#sensor-catalog-status').textContent='Camera peers unavailable · retrying';}
 setTimeout(sensorCatalog,10000);
}
async function watchSensor(generation){
 if(generation!==sensorGeneration||!sensorSelect.value)return;
 const status=document.querySelector('#sensor-feed-status');
 try{
  const response=await fetch('/api/sensors?sensor='+encodeURIComponent(sensorSelect.value)+(arToggle.checked?'&ar=true':'')+(arAvailable.get(sensorSelect.value)?'&pick=true':''));const f=await readCameraResponse(response);
  if(generation!==sensorGeneration)return;
  if(f.sequence!==lastSensorSequence&&!labelDialog.open){
   const bytes=f.bytes;
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
   videoPresentation.update(f);
   if(arToggle.checked){arOverlay??=createArOverlay($('sensor-ar-overlay'));arOverlay.show(f,Number($('sensor-stage').dataset.rotation)||0);arLastSuccess=Date.now();arStatus.textContent='AR · Home notes + drawings · capture-matched camera';}
   lastSensorSequence=f.sequence;lastSensorChange=Date.now();
  }
  status.textContent=Date.now()-lastSensorChange>3000?'Stale · no new frames':'Receiving · frame '+f.sequence;
 }catch(e){if(generation===sensorGeneration){displayedCamera=null;status.textContent=(lastSensorSequence!==null?'Stale image · ':'Waiting · ')+e.message;if(!labelDialog.open)hideAr();if(arToggle.checked){arStatus.hidden=false;arStatus.textContent='AR paused · '+e.message;}}}
 if(generation===sensorGeneration)setTimeout(()=>watchSensor(generation),100);
}
sensorSelect.addEventListener('change',()=>{
 videoPresentation.select(sensorSelect.value);
 pip.hidden=!sensorSelect.value;arToggle.checked=false;updateArAvailability();
 sensorCanvas.getContext('2d').clearRect(0,0,sensorCanvas.width,sensorCanvas.height);
 document.querySelector('#sensor-title').textContent=sensorSelect.selectedOptions[0].textContent;
 document.querySelector('#sensor-feed-status').textContent='Waiting for a frame…';restartSensor();
});
document.querySelector('#sensor-close').addEventListener('click',()=>{sensorSelect.value='';sensorSelect.dispatchEvent(new Event('change'));});
sensorCatalog();

// Transient body occupancy: keep cube axes in robot.base, transform the entire
// group with the SDK-composed capture-time pose. Never add cells to topography.
let volumeBusy=false,volumeScope=null;
function volumeContext(){return JSON.stringify([activeMap,current?.frame,current?.source_frame,current?.local_mapping?.map_correction,current?.display_alignment]);}
function staleVolume(v){
 v.stale=true;v.material.color.set('#84908f');v.material.emissive.set('#000000');v.material.transparent=true;v.material.opacity=.35;
 if(!v.tag){v.tag=label('Last known position · tracking lost','#84908f');v.tag.position.set(0,0,1);v.group.add(v.tag);}
 v.group.visible=$('robot-volume-layer').checked;
}
function volumeStatus(){
 const values=[...robotVolumes.values()],stale=values.filter(v=>v.stale).length;
 const status=$('robot-volume-status');status.dataset.robots=String(values.length);status.dataset.stale=String(stale);status.dataset.cells=String(values.reduce((n,v)=>n+v.cells,0));
 if(stale)status.textContent=`${stale} robot${stale===1?'':'s'} · last known position · tracking lost · not live`;
}
function removeVolume(key){const v=robotVolumes.get(key);if(v){scene.remove(v.group);clear(v.group);robotVolumes.delete(key);}}
function resetVolumes(){for(const key of [...robotVolumes.keys()])removeVolume(key);volumeScope=null;}
$('map-select').addEventListener('change',resetVolumes);
$('robot-volume-layer').onchange=()=>{for(const v of robotVolumes.values())v.group.visible=$('robot-volume-layer').checked;};
async function refreshRobotVolumes(){
 if(volumeBusy)return;
 const scope=volumeContext();if(volumeScope!==scope){resetVolumes();volumeScope=scope;}
 if(!(activeMap==='home-live'||isRunner())){resetVolumes();$('robot-volume-status').textContent='Robot volume · choose a live map';return;}
 const selected=activeMap,frame=current?.frame;
 const peer=selected==='runner'?primaryRunnerPeer:selected.startsWith('runner:')?selected.slice(7):null;
 if(isRunner()&&!peer)return;
 volumeBusy=true;
 try{
  const data=await api('/api/robot-occupancy'+(peer?'?peer='+encodeURIComponent(peer):''));
  if(selected!==activeMap||frame!==current?.frame||scope!==volumeContext())return;
  const entries=(data.robots||[]).filter(r=>r.frame===frame);
  const keys=new Set(entries.map(r=>r.peer_id));for(const [key,v] of robotVolumes)if(!keys.has(key))staleVolume(v);
  for(const r of entries){
   bodyVolumePeers.add(r.peer_id);
   const signature=JSON.stringify([r.model_sha256,r.timestamp_ns,r.sequence,r.frame,r.pose]);let v=robotVolumes.get(r.peer_id);
   if(!v||v.signature!==signature){
    removeVolume(r.peer_id);
    const group=new THREE.Group(),material=new THREE.MeshStandardMaterial({color:'#ff2020',emissive:'#a00000',emissiveIntensity:.3,roughness:.7});
    const mesh=new THREE.InstancedMesh(boxGeometry,material,r.cells.length),matrix=new THREE.Matrix4();
    r.cells.forEach((c,i)=>{const s=r.voxel_size;matrix.makeScale(s*.96,s*.96,s*.96);matrix.setPosition(...c.map(n=>(n+.5)*s));mesh.setMatrixAt(i,matrix);});
    mesh.instanceMatrix.needsUpdate=true;mesh.computeBoundingSphere();group.add(mesh);
    group.position.fromArray(r.pose.translation);group.quaternion.fromArray(r.pose.rotation_xyzw);group.visible=$('robot-volume-layer').checked;
    scene.add(group);v={group,material,signature,cells:r.cells.length,at:performance.now(),stale:false};robotVolumes.set(r.peer_id,v);
   }
  }
  const count=entries.reduce((n,r)=>n+r.cells.length,0),missing=entries.reduce((n,r)=>n+r.missing_links.length,0);
  const status=$('robot-volume-status');status.dataset.robots=String(entries.length);status.dataset.cells=String(count);
  status.dataset.poses=JSON.stringify(entries.map(r=>({peer:r.peer_id,pose:r.pose})));
  status.textContent=entries.length?`${entries.length} robot${entries.length===1?'':'s'} · ${count.toLocaleString()} red cells · 2 Hz${missing?' · '+missing+' links missing':''}${entries.some(r=>r.approximate_links.length)?' · approximate envelopes':''}`:
   'Robot volume · '+(data.unavailable?.[0]?.detail||data.status||'waiting for runner');
  volumeStatus();
  refreshRunner();
 }catch(e){if(selected!==activeMap||scope!==volumeContext())return;for(const v of robotVolumes.values())staleVolume(v);$('robot-volume-status').textContent='Robot volume · '+e.message;volumeStatus();}
 finally{volumeBusy=false;}
}
setInterval(()=>{for(const v of robotVolumes.values())if(performance.now()-v.at>3000)staleVolume(v);volumeStatus();refreshRobotVolumes();},500);

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
  button.title=option.title;button.disabled=homeOffline||option.disabled;if(homeOffline)button.textContent+=' · offline';button.dataset.map=option.value;button.setAttribute('aria-pressed',String(option.value===$('map-select').value));return button;
 }));
  if(!options.length){const empty=document.createElement('p');empty.className='panel-hint';empty.textContent='No scenegraphs available';section.append(empty);}
  mapsList.append(section);
 }
}
new MutationObserver(renderMapRows).observe($('map-select'),{childList:true,subtree:true,characterData:true});
$('map-select').addEventListener('change',()=>{renderMapRows();noteView='list';renderNotes();});renderMapRows();
function catalogType(entry,kind,catalog){
 const producer=entry.manifest.producer;
 const component=kind==='component'?entry:(catalog.components||[]).find(c=>c.manifest.component_id===producer?.component_id&&(!producer.component_manifest_hash||c.manifest_hash===producer.component_manifest_hash));
 const outputs=Object.entries(component?.current_outputs||{}).filter(([slot])=>kind==='component'||slot===producer?.slot).map(([,o])=>o.manifest.payload);
 const contracts=(component?.manifest.observables||[]).filter(o=>kind==='component'||o.name===producer?.slot);
 const types=[...outputs,...contracts].filter(Boolean);
 const signature=types.map(t=>[t.kind,t.modality,t.datatype,t.schema].filter(Boolean).join(' ')).join(' ')+' '+(entry.metadata?.schema||'');
 if(types.some(t=>t.kind==='camera')||/video[-_.]?frame|camera[-_.]?frame/i.test(signature))return 'Cameras';
 if(/scenegraph|canonical-scene|voxel|occupancy/i.test(signature))return 'Maps';
 if(/odometry|pose[-_.]?update|frame[-_]?transform|\bpose\b/i.test(signature))return 'Poses & odometry';
 if(/detection|landmark-observations/i.test(signature))return 'Detections';
 if(/lidar|depth|sensor|mapping.observation|mapping-input/i.test(signature))return 'Sensors & observations';
 return 'Other';
}
function renderFleetRows(){
 if(componentControlPending||(fleetList.contains(document.activeElement)&&document.activeElement.matches('input,textarea,select')))return;
 disconnectRobots.hidden=!fleetNetwork?.simulation;disconnectRobots.disabled=homeOffline||!(fleetNetwork?.peers||[]).some(p=>p.status==='connected');
 const selfId=fleetNetwork?.connection?.peer_id;
 const peers=[{peer_id:selfId||'self',name:'Home Server',self:true,catalog:fleetNetwork?.catalog},...Array.from(new Map((fleetNetwork?.peers||[]).filter(p=>p.peer_id&&p.peer_id!==selfId).map(p=>[p.peer_id,p])).values())];
 const sensors=Array.from($('sensor-select').options).filter(o=>o.value);
 const signature=JSON.stringify([peers,fleetNetwork?.enabled,fleetNetwork?.status,sensors.map(o=>[o.value,o.textContent,o.dataset.peer]),$('sensor-select').value]);
 if(signature===fleetSignature)return;fleetSignature=signature;
 const opened=new Set(Array.from(fleetList.querySelectorAll('details[open]'),d=>d.dataset.peer));
 const openedCatalog=new Set(Array.from(fleetList.querySelectorAll('details[open][data-product],details[open][data-component]'),d=>JSON.stringify([d.closest('[data-peer]').dataset.peer,d.dataset.product!==undefined?'product':'component',d.dataset.product??d.dataset.component])));fleetList.replaceChildren();
 for(const peer of peers){const options=sensors.filter(o=>o.dataset.peer===peer.peer_id);const group=document.createElement('details');group.dataset.peer=peer.peer_id;group.open=opened.has(peer.peer_id);const heading=document.createElement('summary');heading.textContent=peer.self?'Home Server':peer.name||peer.peer_id;group.append(heading);
  const detail=document.createElement('p');detail.className='panel-hint';detail.textContent=peer.self?(fleetNetwork?.enabled?'This peer · '+fleetNetwork.status:fleetNetwork?'This peer · P2P disabled':'This peer · Network status unavailable'):peer.status||'Connected peer';group.append(detail);
  if(!peer.self){const identity=document.createElement('p');identity.className='panel-hint';identity.textContent=peer.peer_id;group.append(identity);}
  if(!peer.self){const form=document.createElement('form');const input=document.createElement('input');input.type='text';input.maxLength=64;input.placeholder='Peer alias';input.value=fleetNetwork?.aliases?.[peer.peer_id]||'';input.setAttribute('aria-label','Alias for '+(peer.name||peer.peer_id));const save=document.createElement('button');save.type='submit';save.textContent='Save alias';form.append(input,save);form.onsubmit=async e=>{e.preventDefault();save.disabled=true;try{await api('/api/peer-alias',{peer_id:peer.peer_id,alias:input.value});input.blur();fleetSignature='';lastScene=null;await refreshPeer();await runnerCatalog();await refresh();}catch(error){save.disabled=false;alert(error.message);}};group.append(form);}
  if(peer.received_observations!==undefined){const count=document.createElement('p');count.className='panel-hint';count.dataset.observations=String(peer.received_observations);count.textContent=peer.received_observations+' observations received over P2P';group.append(count);}
  if(!peer.self){const catalogStatus=document.createElement('p');catalogStatus.className='panel-hint';catalogStatus.dataset.catalogStatus=peer.catalog_status||'unknown';catalogStatus.textContent=peer.catalog_status==='ready'?((peer.catalog?.components?.length||0)+' components · '+(peer.catalog?.products?.length||0)+' products'):peer.catalog_status==='unavailable'?(peer.catalog_detail||'Component catalog unavailable'):peer.catalog?'':'Loading component catalog…';if(catalogStatus.textContent)group.append(catalogStatus);}
  if(peer.catalog_cached){const hint=document.createElement('p');hint.className='panel-hint';hint.textContent='Catalog refresh delayed · using last known entries; stream freshness checked separately';group.append(hint);}
  if(peer.registries){const hint=document.createElement('p');hint.className='panel-hint';hint.textContent='Registry: '+peer.registries.status+' · '+Object.keys(peer.registries.definitions||{}).length+' resolved definitions';hint.title=JSON.stringify(peer.registries.errors||peer.registries.detail||{},null,2);group.append(hint);}
  const feeds=document.createElement('section');feeds.setAttribute('aria-label','Camera feeds');const feedsHeading=document.createElement('h3');feedsHeading.textContent=`Camera feeds (${options.length})`;feeds.append(feedsHeading);
  for(const option of options.sort((a,b)=>a.textContent.localeCompare(b.textContent))){const button=uiButton(option.textContent+($('sensor-select').value===option.value?' · PiP ●':''),()=>{$('sensor-select').value=$('sensor-select').value===option.value?'':option.value;$('sensor-select').dispatchEvent(new Event('change'));});button.setAttribute('aria-pressed',String($('sensor-select').value===option.value));feeds.append(button);}
  if(!options.length){const empty=document.createElement('p');empty.className='panel-hint';empty.textContent='No camera feeds available';feeds.append(empty);}group.append(feeds);
  const catalog=peer.catalog||{},entries=[...(catalog.components||[]).map(entry=>({kind:'component',entry})),...(catalog.products||[]).map(entry=>({kind:'product',entry}))];
  const poseLogs=document.createElement('section'),poseHeading=document.createElement('h3');poseLogs.setAttribute('aria-label','Pose logs');poseHeading.textContent='Pose logs';poseLogs.append(poseHeading);
  const poses=(catalog.products||[]).filter(p=>p.metadata?.schema==='spatial-harness.compact-pose-contract/v1');
  for(const product of poses)poseLogs.append(uiButton('Inspect '+product.manifest.product_id,()=>inspectPose(peer.peer_id,product,(peer.name||peer.peer_id)+' / '+product.manifest.product_id)));
  if(!poses.length){const hint=document.createElement('p');hint.className='panel-hint';hint.textContent='No compact pose Product advertised';poseLogs.append(hint);}group.append(poseLogs);
  for(const type of ['Cameras','Maps','Poses & odometry','Detections','Sensors & observations','Other']){
   const matching=entries.filter(item=>catalogType(item.entry,item.kind,catalog)===type).sort((a,b)=>a.entry.manifest[a.kind+'_id'].localeCompare(b.entry.manifest[b.kind+'_id'])||a.kind.localeCompare(b.kind));
   if(!matching.length)continue;
   const section=document.createElement('section');section.dataset.catalogType=type;section.setAttribute('aria-label',type);const heading=document.createElement('h3');heading.textContent=`${type} (${matching.length})`;section.append(heading);
   for(const {kind,entry} of matching){
   const id=entry.manifest[kind+'_id'],details=document.createElement('details');details.dataset[kind]=id;details.open=openedCatalog.has(JSON.stringify([peer.peer_id,kind,id]));
   const title=document.createElement('summary');title.textContent=(kind==='component'?'Component':'Product')+' · '+id;
   if(kind==='component'&&!peer.self&&entry.manifest.operables?.some(o=>o.name==='set_enabled'&&o.instruction==='spatial-harness.set-enabled/v1'&&o.result==='spatial-harness.component-enabled-state/v1')){
    const stateProduct=(catalog.products||[]).find(p=>p.manifest.producer?.component_id===id&&p.manifest.producer?.component_manifest_hash===entry.manifest_hash&&p.metadata?.schema==='spatial-harness.component-enabled-state/v1');
    let enabled=stateProduct?.metadata?.value?.enabled;
    const key=JSON.stringify([peer.peer_id,id,entry.manifest_hash,stateProduct?.manifest_hash]),ack=componentControlResults.get(key);
    if(ack){if(enabled===ack.enabled||stateProduct?.metadata?.source_sequence>ack.sequence)componentControlResults.delete(key);else enabled=ack.enabled;}
    const control=document.createElement('div'),status=document.createElement('span');status.setAttribute('role','status');
    const button=uiButton(typeof enabled==='boolean'?(enabled?'Turn off':'Turn on'):'State unavailable',async()=>{
     componentControlPending=true;button.disabled=true;status.textContent='Applying…';
     try{const confirmed=await api('/api/component-enabled',{peer_id:peer.peer_id,component_id:id,manifest_hash:entry.manifest_hash,enabled:!enabled});
      enabled=confirmed.enabled;componentControlResults.set(key,{enabled,sequence:stateProduct?.metadata?.source_sequence});button.textContent=enabled?'Turn off':'Turn on';status.textContent=enabled?'Enabled':'Disabled';
     }catch(e){status.textContent=e.message;button.textContent='Refresh Devices to confirm';return;}
     finally{componentControlPending=false;}
     button.disabled=false;
    });
    button.disabled=typeof enabled!=='boolean'||peer.catalog_status!=='ready'||!!peer.catalog_cached;
    status.textContent=typeof enabled==='boolean'?(enabled?'Enabled':'Disabled'):'Waiting for state';control.append(button,status);details.append(control);
   }
   if(kind==='product'&&entry.metadata?.schema==='spatial-harness.compact-pose-contract/v1')details.append(uiButton('Inspect live pose',()=>inspectPose(peer.peer_id,entry,(peer.name||peer.peer_id)+' / '+id)));
   const pre=document.createElement('pre');pre.style.cssText='white-space:pre-wrap;overflow-wrap:anywhere;font-size:10px';pre.textContent=JSON.stringify(entry.manifest,null,2);details.append(title,pre);section.append(details);
   }group.append(section);
  }
  if(peer.self&&selfId){const identity=document.createElement('p');identity.className='panel-hint';identity.textContent=selfId;group.append(identity);}
  fleetList.append(group);
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
 const paths=current?.paths?.items||[],path=paths.find(p=>p.id===selectedPathId);
 if(noteView==='path'&&path){
  notesBody.append(uiButton('‹ All notes',()=>{noteView='list';renderNotes();}));
  const title=document.createElement('h3');title.textContent=path.title;notesBody.append(title);
  noteMessage('Crayon path · '+path.segments.length+' segment(s)');
  notesBody.append(uiButton('Focus in scene',()=>focusPath(path)));
  if(isLocalScene())notesBody.append(pathDeleteButton(path));
  return;
 }
 if(noteView==='detail'&&selected){
  notesBody.append(uiButton('‹ All notes',()=>{noteView='list';renderNotes();}));
  const title=document.createElement('h3');title.textContent=noteTitle(selected);notesBody.append(title);noteMessage('Pose · '+selected.position.map(v=>v.toFixed(2)).join(', ')+' m');
  if(selected.details?.description)noteMessage(selected.details.description);
  if(selected.details?.photo)notesBody.append(notePhoto(selected));
  notesBody.append(uiButton('Focus in scene',()=>focusNote(selected)));
  if(!selected.anchor_robot&&!selected.phone_pose&&!selected.robot_pose&&!isRunner()&&activeMap!=='domain'){
   notesBody.append(uiButton('Edit',()=>editSidebarNote(selected)),uiButton('Delete',async()=>{const error=noteMessage('Deleting…');try{await api(noteUrl(selected.id),undefined,'DELETE');noteView='list';lastScene=null;await refresh();renderNotes();}catch(e){error.textContent=e.message;}}));
  }
  return;
 }
 notesBody.append(uiButton('+ New note',()=>editSidebarNote(null),'primary'));
 if(!notes.length)noteMessage('No notes in this scenegraph.');
 if(paths.length){
  const heading=document.createElement('h3');heading.textContent='Crayon paths';notesBody.append(heading);
  paths.forEach((path,i)=>{const row=uiButton(`${path.title} · ${i+1}`,()=>{noteView='path';focusPath(path);renderNotes();});row.dataset.pathId=path.id;notesBody.append(row);});
 }
 for(const note of notes){const row=uiButton(noteTitle(note),()=>{sidebarNoteId=note.id;noteView='detail';showNoteSelection(note);renderNotes();});if(note.details?.thumbnail){const image=document.createElement('img');image.src=note.details.thumbnail;image.alt='';image.className='note-list-thumbnail';row.prepend(image);}notesBody.append(row);}
}
async function editSidebarNote(note){
 const editingMap=activeMap;
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
 if(!note){save.disabled=true;try{const canonical=await api(isLocalScene(editingMap)?localSceneUrl(editingMap):'/api/scene');for(const p of canonical.portals||[])portal.add(new Option(p.anchor_id,p.anchor_id));save.disabled=!portal.options.length;if(save.disabled)error.textContent='No portals in this scene. Right-click a surface to place a note.';}catch(e){error.textContent=e.message;}}
 form.onsubmit=async event=>{event.preventDefault();save.disabled=true;try{
  const details=await fields.read(input.value);
  const result=note?await api(noteUrl(note.id,editingMap),{text:input.value,details},'PATCH'):await api('/api/notes'+(isLocalScene(editingMap)?'?scene='+encodeURIComponent(editingMap.slice(6)):''),{text:input.value,details,portal:portal.value,height_m:Number(height.value)});
  sidebarNoteId=result.id;noteView='detail';
  if(isRunner()||activeMap==='domain'){$('map-select').value='demo';await $('map-select').onchange();}
  lastScene=null;await refresh();renderNotes();
 }catch(e){error.textContent=e.message;save.disabled=false;}};
}
setInterval(()=>{const signature=JSON.stringify([activeMap,current?.notes||[],current?.paths]);if(signature!==noteSignature){noteSignature=signature;renderNotes();}},500);renderNotes();
function applyTheme(light){document.documentElement.dataset.theme=light?'light':'dark';scene.background.set(light?'#e8e8f0':'#0d0d0f');scene.fog?.color.copy(scene.background);ground.material.color.set(light?'#e0e3e8':'#101014');themeButton.textContent=light?'☾':'☼';themeButton.title=light?'Switch to dark theme':'Switch to light theme';themeButton.setAttribute('aria-label',themeButton.title);}
themeButton.onclick=()=>{const light=document.documentElement.dataset.theme!=='light';applyTheme(light);try{localStorage.setItem('spatial-theme',light?'light':'dark');}catch{}};
try{applyTheme(localStorage.getItem('spatial-theme')==='light');}catch{}
const opacityLabel=document.createElement('label');opacityLabel.className='layer';opacityLabel.textContent='Grid opacity';const opacityInput=document.createElement('input');opacityInput.type='range';opacityInput.min='0';opacityInput.max='1';opacityInput.step='.01';opacityInput.value=String(grid.material.opacity);opacityInput.oninput=()=>{grid.material.opacity=Number(opacityInput.value);};opacityLabel.append(opacityInput);panelBodies.settings.append(opacityLabel);

let createdFiducialKey=null;
function fiducialLink(text,href){const link=document.createElement('a');link.textContent=text;link.href=href;return link;}
async function renderFiducials(){
 tagsBody.replaceChildren();
 const intro=document.createElement('p');intro.className='panel-hint';intro.textContent='Generate a standard visual tag and register the size of its outer black square. That measured size is the metric contract a pose estimator needs.';tagsBody.append(intro);
 let catalog;
 try{catalog=await api('/api/fiducials');}catch(e){const status=document.createElement('p');status.className='panel-hint';status.setAttribute('role','status');status.textContent=e.message;tagsBody.append(status);return;}
 const form=document.createElement('form');form.className='note-form fiducial-form';
 const familyLabel=document.createElement('label');familyLabel.textContent='Tag family';const family=document.createElement('select');family.id='fiducial-family';familyLabel.htmlFor=family.id;for(const item of catalog.families)family.add(new Option(item.label,item.id));familyLabel.append(family);form.append(familyLabel);
 const idLabel=document.createElement('label');idLabel.textContent='Marker ID (optional)';const id=document.createElement('input');id.id='fiducial-id';id.type='number';id.min='0';id.step='1';id.placeholder='Next available';idLabel.htmlFor=id.id;idLabel.append(id);form.append(idLabel);
 const sizeLabel=document.createElement('label');sizeLabel.textContent='Printed marker size (mm)';const size=document.createElement('input');size.id='fiducial-size';size.type='number';size.min='10';size.max='2000';size.step='.1';size.value='150';size.required=true;sizeLabel.htmlFor=size.id;sizeLabel.append(size);form.append(sizeLabel);
 const basis=document.createElement('p');basis.className='panel-hint fiducial-basis';basis.textContent='Measure edge-to-edge across the black square. Do not include the white quiet zone.';form.append(basis);
 const create=document.createElement('button');create.type='submit';create.className='primary';create.textContent='Generate tag';form.append(create);
 const message=document.createElement('p');message.className='panel-hint';message.setAttribute('role','status');form.append(message);
 form.onsubmit=async event=>{event.preventDefault();create.disabled=true;message.textContent='Generating…';try{const marker=await api('/api/fiducials',{family:family.value,id:id.value===''?null:Number(id.value),side_length_m:Number(size.value)/1000});createdFiducialKey=marker.key;await renderFiducials();}catch(e){message.textContent=e.message;create.disabled=false;}};
 tagsBody.append(form);
 const heading=document.createElement('div');heading.className='section-label';heading.textContent='REGISTERED TAGS';tagsBody.append(heading);
 if(!catalog.fiducials.length){const empty=document.createElement('p');empty.className='panel-hint';empty.textContent='No generated tags yet.';tagsBody.append(empty);return;}
 for(const marker of catalog.fiducials){const card=document.createElement('article');card.className='fiducial-card';if(marker.key===createdFiducialKey)card.dataset.created='true';
  const image=document.createElement('img');image.className='fiducial-preview';image.src=marker.download_url;image.alt=`${marker.family_label} marker ${marker.id}`;
  const details=document.createElement('div');const name=document.createElement('strong');name.textContent=`${marker.family_label} · ${marker.id}`;const measurement=document.createElement('small');measurement.textContent=`${marker.side_length_mm.toLocaleString(undefined,{maximumFractionDigits:1})} mm black square`;details.append(name,measurement);
  const actions=document.createElement('div');actions.className='fiducial-actions';const download=fiducialLink('Download SVG',marker.download_url);download.download=`${marker.key}.svg`;const print=fiducialLink('Print at size',marker.print_url);print.target='_blank';print.rel='noopener';actions.append(download,print);details.append(actions);
  const edit=uiButton('Set measured size',()=>{
   edit.hidden=true;const form=document.createElement('form');form.className='note-form fiducial-measurement';
   const label=document.createElement('label');label.textContent='Measured black square (mm)';const input=document.createElement('input');input.type='number';input.min='10';input.max='2000';input.step='.1';input.required=true;input.value=String(marker.side_length_mm);label.append(input);form.append(label);
   const buttons=document.createElement('div');buttons.className='note-actions';const save=document.createElement('button');save.type='submit';save.textContent='Save size';buttons.append(uiButton('Cancel',()=>{form.remove();edit.hidden=false;}),save);form.append(buttons);
   const status=document.createElement('p');status.className='panel-hint';status.setAttribute('role','status');form.append(status);details.append(form);input.focus();
   form.onsubmit=async event=>{event.preventDefault();save.disabled=true;try{await api(`/api/fiducials/${marker.family}/${marker.id}`,{side_length_m:Number(input.value)/1000},'PATCH');await renderFiducials();}catch(e){status.textContent=e.message;save.disabled=false;}};
  });details.append(edit);card.append(image,details);tagsBody.append(card);
 }
 createdFiducialKey=null;
}
renderFiducials();
