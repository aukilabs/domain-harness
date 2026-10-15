import * as THREE from 'three';
import { zoomTier, clusterNotes } from './note-lod.js';

// Scene labels: note bubbles and volume names as HTML over the 3D view, so text is crisp CSS pixels, themes
// apply and every change of form animates. Styles live in style.css under "Scene labels".
// - One zoom tier for the whole scene (see note-lod.js): text, then pins and tiles, then note clusters.
// - Labels never take pointer events; the canvas keeps every click, drag and wheel, and asks hitTest().
// - Stacking is fixed by kind and state, never by depth: volume chips, then pins and clusters, then bubbles,
//   then the hovered label, then the selected one.
// - Declutter: only distance (the zoom tier) turns a note into a pin, so notes always show their tier's form. A
//   volume chip that would cover a note, or a higher-priority chip, steps out of sight. While the view moves labels only step down (5 times a
//   second); they come back once it settles, with 4 px of hysteresis, so nothing flickers while orbiting.
// - Positions are written every frame straight from the anchors (no easing, so labels never swim); only the
//   form inside a label transitions. Clusters merge and split by sliding their pins (JS-driven, 260 ms).
// Items: {id, kind:'note'|'volume', anchor:Object3D, text, detail?, peer?, note?, volumeId?, hidden?}. The anchor's
// world position is the label's point and its ancestors' visibility is the label's visibility.
const NOTE_GLYPH='M4 2h10v14H4ZM6 6h6M6 9h6M6 12h4',CUBE='M9 1.5 15.5 5v8L9 16.5 2.5 13V5ZM2.5 5 9 8.5l6.5-3.5M9 8.5v8';
const glyph=(d,size)=>`<svg width="${size}" height="${size}" viewBox="0 0 18 18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="${d}"/></svg>`;
const reduceMotion=()=>matchMedia('(prefers-reduced-motion: reduce)').matches;
const smoothstep=(a,b,x)=>{const t=Math.min(1,Math.max(0,(x-a)/(b-a)));return t*t*(3-2*t);};
const easeOut=t=>1-Math.pow(1-Math.min(1,Math.max(0,t)),3);
const MERGE_MS=260,CLUSTER_DELAY_MS=180;

export function createSceneLabels({camera,controls,renderer,parent=renderer.domElement.parentElement,hidden=()=>false,fog=()=>null}){
 const layer=document.createElement('div');layer.className='scene-labels';layer.setAttribute('aria-hidden','true');parent.append(layer);
 const sources=new Map(),labels=new Map(),clusters=new Map(),point=new THREE.Vector3(),at=new THREE.Vector3();
 let tier=0,selected=null,hovered=null,mask=null,moving=false,dirty=true,declutterAt=0,settled=false,clusterCache=null,focus=null,lastView='',restSince=0,host=null,canvasRect=null;

 function shown(anchor){if(!anchor.parent)return false;for(let o=anchor;o;o=o.parent)if(!o.visible)return false;return true;}
 function create(item){
  const el=document.createElement('div');el.className='scene-label';el.dataset.kind=item.kind;el.dataset.form='hidden';
  el.innerHTML=item.kind==='note'
   ?`<span class="label-form label-bubble"><span class="label-ring"></span><span class="label-body"><span class="label-text"></span></span></span><span class="label-form label-pin">${glyph(NOTE_GLYPH,12)}</span>`
   :`<span class="label-form label-chip"><span class="label-tile">${glyph(CUBE,11)}</span><span class="label-text"></span><span class="label-detail"></span></span><span class="label-form label-tile-only"><span class="label-tile">${glyph(CUBE,11)}</span></span>`;
  layer.append(el);
  return {el,item,form:'hidden',text:null,size:{},x:NaN,y:NaN,visible:false,merge:null,rect:null};
 }
 function describe(label){
  const {item,el}=label,text=String(item.text||(item.kind==='note'?'Note':'Volume')),detail=item.detail?String(item.detail):'';
  if(label.text===text+'\n'+detail)return;label.text=text+'\n'+detail;label.size={};
  el.querySelector('.label-text').textContent=text;const extra=el.querySelector('.label-detail');if(extra){extra.textContent=detail;extra.hidden=!detail;}
  el.dataset.peer=item.peer?'true':'false';
 }
 // sync(source, items): each caller (notes, legacy volumes, semantic volumes) owns its own list.
 function sync(source,items){
  sources.set(source,items);const ids=new Set();
  for(const list of sources.values())for(const item of list){ids.add(item.id);let label=labels.get(item.id);if(!label){label=create(item);labels.set(item.id,label);}label.item=item;describe(label);}
  for(const [id,label] of labels)if(!ids.has(id)){label.el.remove();labels.delete(id);}
  dirty=true;if(source==='notes')clusterCache=null;
 }

 // The zoom focus (the last wheel position, set by the app's wheel zoom) staggers changes outward from where the
 // user is looking.
 const stagger=label=>{if(!focus||!label.visible||!host)return 0;return Math.min(120,.12*Math.hypot(label.x+host.left-focus[0],label.y+host.top-focus[1]));};
 function setForm(label,form){
  if(label.form===form)return;
  label.el.style.setProperty('--delay',Math.round(stagger(label))+'ms');label.form=form;label.el.dataset.form=form;
 }
 const stepDown={chip:'hidden',tile:'hidden'};
 function shownForm(label){return label.demoted&&stepDown[label.base]?stepDown[label.base]:label.base;}
 function clusterMembers(cluster){return cluster.ids.map(id=>labels.get(id)?.item).filter(Boolean);}
 function clusterCentre(cluster){const members=clusterMembers(cluster);if(!members.length)return false;cluster.centre.set(0,0,0);for(const m of members)cluster.centre.add(m.anchor.getWorldPosition(point));cluster.centre.divideScalar(members.length);return members.every(m=>shown(m.anchor));}
 // Where a pin is in its slide (0 at its own spot, 1 at the cluster), so a reversed slide carries on from there.
 function slideAt(label,now){const m=label.merge;if(!m)return label.form==='merged'?1:0;const t=reduceMotion()?1:Math.min(1,(now-m.start)/MERGE_MS);return m.dir>0?easeOut(t):1-easeOut(t);}
 function clusterKey(members){return members.map(m=>m.id).join('|');}
 function decide(now){
  const notes=[...labels.values()].filter(l=>l.item.kind==='note'&&!l.item.hidden);
  // Membership in world space, cached per note set; the selected note never joins a cluster.
  let groups=[];
  if(tier>=2){
   clusterCache??=clusterNotes(notes.filter(l=>l.item.id!==selected).map(l=>{l.item.anchor.getWorldPosition(point);return {id:l.item.id,x:point.x,z:point.z};}));
   groups=clusterCache.filter(g=>g.length>1);
  }
  const memberOf=new Map();for(const g of groups){const key=clusterKey(g);for(const m of g)memberOf.set(m.id,key);}
  for(const label of labels.values()){
   const {item}=label;let form;
   if(item.hidden)form='hidden';
   else if(item.kind==='note')form=item.id===selected?'bubble':tier===0?'bubble':memberOf.has(item.id)?'merged':'pin';
   else form=tier===0?'chip':tier===1?'tile':'hidden';
   const was=label.form;
   label.base=form;
   if(form==='merged'&&was!=='merged'){const k=slideAt(label,now);label.merge={start:now-(1-Math.cbrt(1-k))*MERGE_MS,dir:1,key:memberOf.get(item.id)};setForm(label,'pin');label.el.dataset.merging='';label.form='merged';continue;}
   if(was==='merged'&&form!=='merged'){const key=label.merge?.key||label.mergedInto,k=slideAt(label,now);label.merge={start:now-(1-Math.cbrt(k))*MERGE_MS,dir:-1,key,centre:clusters.get(key)?.centre.clone()};label.form=form;label.el.dataset.form=form==='hidden'?'hidden':'pin';label.el.dataset.merging='';continue;}
   setForm(label,shownForm(label));
  }
  // Cluster pins: created when their members start merging, popping in once the pins arrive.
  const keep=new Set();
  for(const g of groups){
   const key=clusterKey(g);keep.add(key);let cluster=clusters.get(key);
   if(!cluster){
    const el=document.createElement('div');el.className='scene-label';el.dataset.kind='cluster';el.dataset.form='hidden';
    el.innerHTML='<span class="label-form label-cluster"><span class="label-count"></span></span>';el.querySelector('.label-count').textContent=g.length>99?'99+':String(g.length);layer.append(el);
    // Members are kept by id and resolved each frame: a scene rebuild replaces every note's anchor.
    cluster={el,key,centre:new THREE.Vector3(),ids:g.map(m=>m.id),x:NaN,y:NaN,visible:false,rect:null,form:'hidden'};clusters.set(key,cluster);clusterCentre(cluster);
    clearTimeout(cluster.timer);cluster.timer=setTimeout(()=>{if(clusters.get(key)===cluster){cluster.form='cluster';el.dataset.form='cluster';}},reduceMotion()?0:CLUSTER_DELAY_MS);
   }
   for(const m of g)labels.get(m.id).mergedInto=key;
  }
  for(const [key,cluster] of clusters)if(!keep.has(key)){clusters.delete(key);clearTimeout(cluster.timer);cluster.el.dataset.form='hidden';cluster.form='hidden';setTimeout(()=>cluster.el.remove(),reduceMotion()?0:220);for(const label of labels.values())if(label.merge?.key===key&&label.merge.dir<0)label.merge.centre=cluster.centre.clone();}
  let pins=0;for(const label of labels.values())if(label.form==='pin')pins++;
  renderer.domElement.dataset.notePinCount=String(pins);renderer.domElement.dataset.noteClusterCount=String(groups.length);
 }

 // Screen position of a world point relative to the layer, or null behind the camera or past the far plane.
 function place(world){point.copy(world).project(camera);if(point.z>1||point.z<-1)return null;return [canvasRect.left-host.left+(point.x+1)/2*canvasRect.width,canvasRect.top-host.top+(1-point.y)/2*canvasRect.height];}
 function write(target,xy,still){
  if(!xy){if(target.visible){target.el.style.display='none';target.visible=false;}return;}
  if(!target.visible){target.el.style.display='';target.visible=true;}
  let [x,y]=xy;if(still){const dpr=devicePixelRatio||1;x=Math.round(x*dpr)/dpr;y=Math.round(y*dpr)/dpr;}
  if(!(Math.abs(x-target.x)<=.1&&Math.abs(y-target.y)<=.1)){target.x=x;target.y=y;target.el.style.transform=`translate3d(${x}px,${y}px,0)`;}
 }
 const formNode={bubble:'.label-bubble',pin:'.label-pin',chip:'.label-chip',tile:'.label-tile-only'};
 function measure(label,form){if(!label.size[form]?.w){const node=label.el.querySelector(formNode[form]);label.size[form]={w:node.offsetWidth,h:node.offsetHeight};}return label.size[form];}
 // Client-space hit rects for the forms a click can land on (targets, not the animated opacity).
 function hitRect(label,form=label.form){
  const x=label.x+host.left,y=label.y+host.top;
  if(form==='bubble'){const s=measure(label,'bubble');return {left:x-s.w/2,right:x+s.w/2,top:y-s.h-6,bottom:y};}
  if(form==='pin')return {left:x-16,right:x+16,top:y-27,bottom:y+5};
  if(form==='chip'||form==='tile'){const s=measure(label,form);return {left:x-s.w/2,right:x+s.w/2,top:y-s.h,bottom:y};}
  return null;
 }

 function tick(now=performance.now()){
  layer.hidden=hidden();if(layer.hidden)return;
  canvasRect=renderer.domElement.getBoundingClientRect();host=layer.getBoundingClientRect();
  const distance=camera.position.distanceTo(controls.target),mpp=2*distance*Math.tan(THREE.MathUtils.degToRad(camera.fov/2))/Math.max(1,canvasRect.height);
  const nextTier=zoomTier(mpp,tier);if(nextTier!==tier){tier=nextTier;dirty=true;}
  // Positions snap to device pixels once the view has been still for 100 ms; while it moves they stay sub-pixel.
  const view=camera.matrixWorld.elements.join(',')+camera.projectionMatrix.elements.join(',')+canvasRect.width+'x'+canvasRect.height;
  if(view!==lastView){lastView=view;restSince=now;}const still=now-restSince>100;moving=!still;
  if(dirty){dirty=false;decide(now);}
  // Labels fade with the scene's distance fog; a faint label isn't clickable, and the selected one stays readable.
  const f=fog(),fade=(world,floor=0)=>{const alpha=f?1-smoothstep(f.near,f.far,camera.position.distanceTo(world)):1;return Math.max(floor,alpha);};
  const tint=(target,alpha)=>{const value=alpha>.99?'':alpha.toFixed(2);if(target.alpha!==value){target.alpha=value;target.el.style.opacity=value;}};
  for(const cluster of clusters.values()){const live=clusterCentre(cluster),alpha=fade(cluster.centre);cluster.faint=alpha<.5;write(cluster,alpha>.01&&live?place(cluster.centre):null,still);tint(cluster,alpha);}
  for(const label of labels.values()){
   const {item,el}=label;item.anchor.getWorldPosition(at);const alpha=fade(at,item.id===selected?.5:0);label.faint=alpha<.5;
   write(label,alpha>.01&&shown(item.anchor)?place(at):null,still);tint(label,alpha);
   if(label.merge){
    // Slide the pin into its cluster (dir 1) or back out of it (dir -1).
    const merge=label.merge,cluster=clusters.get(merge.key),centre=cluster?.centre||merge.centre,t=reduceMotion()?1:(now-merge.start)/MERGE_MS,k=merge.dir>0?easeOut(t):1-easeOut(t),pin=el.querySelector('.label-pin');
    const at=centre&&label.visible?place(centre):null;
    if(at){pin.style.transform=`translate(calc(-50% + ${(at[0]-label.x)*k}px),${(at[1]-label.y)*k}px)`;pin.style.opacity=String(merge.dir>0?1-k*.85:1-k);}
    if(t>=1){label.merge=null;pin.style.transform='';pin.style.opacity='';delete el.dataset.merging;el.dataset.form=label.form==='merged'?'hidden':label.form;}
   }
   // Labels under an open overlay (the right-click ring) fade out instead of flickering through its glass.
   const covered=!!mask&&label.visible&&underMask(hitRect(label));if(covered!==!!label.masked){label.masked=covered;el.classList.toggle('masked',covered);}
   label.rect=label.visible&&!label.merge&&!label.faint&&!covered?hitRect(label):null;
  }
  for(const cluster of clusters.values())cluster.rect=cluster.visible&&!cluster.faint&&cluster.form==='cluster'?{left:cluster.x+host.left-16,right:cluster.x+host.left+16,top:cluster.y+host.top-29,bottom:cluster.y+host.top+3}:null;
  // Declutter at 5 Hz while moving (step down only), and once when the view settles (steps back up allowed).
  if(!still){settled=false;if(now-declutterAt>200){declutterAt=now;declutter(false);}}
  else if(!settled){settled=true;declutterAt=now;declutter(true);}
 }
 // mask: {x, y, radius} in client px, or null. A label is under it when its rect comes within the circle.
 function underMask(r){if(!r)return false;const dx=Math.max(r.left-mask.x,0,mask.x-r.right),dy=Math.max(r.top-mask.y,0,mask.y-r.bottom);return dx*dx+dy*dy<mask.radius*mask.radius;}
 function setMask(next){mask=next;if(!mask)for(const label of labels.values())if(label.masked){label.masked=false;label.el.classList.remove('masked');}}
 function declutter(mayReturn){
  const rank=label=>label.item.id===selected?1000:label.item.id===hovered?900:label.item.kind==='note'?(label.item.peer?90:100):50;
  const near=label=>{label.item.anchor.getWorldPosition(at);return at.distanceTo(controls.target);};
  const texts=[...labels.values()].filter(l=>l.visible&&!l.faint&&stepDown[l.base]).sort((a,b)=>rank(b)-rank(a)||near(a)-near(b)||(a.item.id<b.item.id?-1:1));
  // Notes (bubbles and pins) and clusters always stay; chips may not cover them.
  const taken=[...labels.values()].filter(l=>l.visible&&!l.faint&&(l.base==='pin'||l.base==='bubble')).map(l=>hitRect(l,l.base));
  for(const cluster of clusters.values())if(cluster.rect)taken.push(cluster.rect);
  const overlaps=(r,pad)=>taken.some(q=>r.left-pad<q.right&&r.right+pad>q.left&&r.top-pad<q.bottom&&r.bottom+pad>q.top);
  let changed=false;
  for(const label of texts){
   const full=hitRect(label,label.base),blocked=label.item.id!==selected&&overlaps(full,label.demoted?4:-4);
   const demoted=blocked||(label.demoted&&!mayReturn);
   if(demoted!==label.demoted){label.demoted=demoted;setForm(label,shownForm(label));changed=true;}
   if(!label.demoted)taken.push(full);
  }
  if(changed){let pins=0;for(const label of labels.values())if(label.form==='pin')pins++;renderer.domElement.dataset.notePinCount=String(pins);}
 }

 // Top-most first, in the fixed stacking order.
 function hitTest(x,y){
  const inside=r=>r&&x>=r.left&&x<=r.right&&y>=r.top&&y<=r.bottom,all=[...labels.values()];
  const result=label=>label.item.kind==='note'?{kind:'note',id:label.item.id,note:label.item.note}:{kind:'volume',id:label.item.id,volumeId:label.item.volumeId,anchor:label.item.anchor};
  for(const id of [selected,hovered]){const label=labels.get(id);if(label&&inside(label.rect))return result(label);}
  for(const label of all)if(label.form==='bubble'&&inside(label.rect))return result(label);
  for(const cluster of clusters.values())if(inside(cluster.rect))return {kind:'note-cluster',notes:clusterMembers(cluster).map(m=>m.note),centre:cluster.centre.clone()};
  for(const label of all)if(label.form==='pin'&&inside(label.rect))return result(label);
  for(const label of all)if((label.form==='chip'||label.form==='tile')&&inside(label.rect))return result(label);
  return null;
 }
 function setSelected(id){if(id===selected)return;labels.get(selected)?.el.classList.remove('selected');selected=id;labels.get(id)?.el.classList.add('selected');dirty=true;clusterCache=null;}
 function setHover(id){if(id===hovered)return;labels.get(hovered)?.el.classList.remove('hovered');hovered=id;labels.get(id)?.el.classList.add('hovered');}
 return {sync,tick,hitTest,setSelected,setHover,setMask,setFocus(x,y){focus=[x,y];},get tier(){return tier;},get moving(){return moving;}};
}
