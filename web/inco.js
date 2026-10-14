// Inco, the Auki mascot, as the scene assistant's avatar. Ported from GettingSpatial's
// src/components/Inco (React) to a plain module: same model, clips and behaviour.
// - Spawns with a one-shot clip, then idles; `think(true)` swaps to the pondering loop and
//   eases a sampled pose on top (hands to head, eyes squinting).
// - Head and neck follow the cursor; the cursor owns those bones, so clips never yank them.
// - Blinks on a random timer; click plays the face-wipe gesture and crossfades to the next skin.
// One instance means one extra WebGL context. `onStill` receives a PNG of the first frame for
// static avatars elsewhere (message rows, the sheet header).
import * as THREE from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';

const CLIP={idle:'SimpleFloaterRig.001|Scene',gesture:'SimpleFloaterRig.042|Scene',spawn:'SimpleFloaterRig.050|Scene',
 despawn:'SimpleFloaterRig.049|Scene',thinking:'SimpleFloaterRig.080|Scene',blink:'SimpleFloaterRig|Scene.002'};
const SKINS=['05','07','08','11','12','04'].map(n=>`/models/Tex_Inco_Skin_${n}.webp`);
const SKIN_FADE_SECONDS=.7,BLINK_MIN_MS=2500,BLINK_EXTRA_MS=4500;
// The rig has no hands-on-head or squint clip, so the thinking pose is sampled from the wipe gesture.
const HANDS_ON_HEAD_TIME=.24,EYES_CLOSED_TIME=.6,SQUINT_AMOUNT=.65;
const THINK_ARM_BONES=['armL001','armL002','armR001','armR002'];
const THINK_EYE_BONES=['eyelid_upperL','eyelid_lowerL','eyelid_upperR','eyelid_lowerR','BSDriver_eyes_01'];
const clamp=(v,lo,hi)=>Math.min(hi,Math.max(lo,v));

export function createInco(host,{framing='bust',onStill}={}){
 let thinking=false,api=null,disposed=false,raf=0,blinkTimer=0;
 let renderer;
 try{renderer=new THREE.WebGLRenderer({alpha:true,antialias:true});}catch{return {think(){},click(){},dispose(){}};}
 renderer.setPixelRatio(Math.min(devicePixelRatio,2));renderer.setSize(host.clientWidth||1,host.clientHeight||1);
 renderer.toneMapping=THREE.ACESFilmicToneMapping;host.append(renderer.domElement);
 const scene=new THREE.Scene(),pmrem=new THREE.PMREMGenerator(renderer);
 // RoomEnvironment does most of the lighting; if it can't load, stronger lights stand in.
 const hemi=new THREE.HemisphereLight(0xffffff,0xbbbbcc,1.1),key=new THREE.DirectionalLight(0xffffff,.35);key.position.set(2,3,4);scene.add(hemi,key);
 import('three/addons/environments/RoomEnvironment.js').then(({RoomEnvironment})=>{if(!disposed)scene.environment=pmrem.fromScene(new RoomEnvironment(),.04).texture;})
  .catch(()=>{hemi.intensity=2.2;key.intensity=1.2;});
 const camera=new THREE.PerspectiveCamera(35,(host.clientWidth||1)/(host.clientHeight||1),.01,100);
 const reducedMotion=matchMedia('(prefers-reduced-motion: reduce)').matches;

 let mixer=null,headBone=null,neckBone=null,headRest=null,neckRest=null;
 const look={yaw:0,pitch:0},lookTarget={yaw:0,pitch:0};
 // The cursor has no say during spawn/despawn, then eases in so the head drifts rather than snaps.
 let lookSuppressed=true,lookWeight=0;

 // Skin cycling: the shader mixes the current map with uSkinNext; each variant loads on first use.
 const skinCache=SKINS.map(()=>null);
 const loadSkin=(index,onLoad)=>{const cached=skinCache[index];if(cached){onLoad?.(cached);return cached;}
  const t=new THREE.TextureLoader().load(SKINS[index],()=>onLoad?.(t));t.flipY=false;t.colorSpace=THREE.SRGBColorSpace;skinCache[index]=t;return t;};
 let skinIndex=0,fading=false,skinMaterial=null;
 const uSkinNext={value:loadSkin(0)},uSkinMix={value:0};
 const nextSkin=()=>{if(fading||!skinMaterial)return;const index=(skinIndex+1)%SKINS.length;skinIndex=index;
  loadSkin(index,t=>{if(skinIndex!==index||!skinMaterial)return;if(reducedMotion){skinMaterial.map=t;renderer.render(scene,camera);return;}uSkinNext.value=t;uSkinMix.value=0;fading=true;});};

 const thinkPose=[];let thinkWeight=0,thinkPoseOn=()=>false,gesturePlaying=()=>false;

 new GLTFLoader().load('/models/inco.glb',gltf=>{
  if(disposed)return;
  const model=gltf.scene;scene.add(model);
  model.traverse(obj=>{if(!obj.isMesh||Array.isArray(obj.material)||!obj.material.map)return;const mat=obj.material;skinMaterial=mat;mat.map=loadSkin(skinIndex);
   mat.onBeforeCompile=shader=>{shader.uniforms.uSkinNext=uSkinNext;shader.uniforms.uSkinMix=uSkinMix;
    shader.fragmentShader=shader.fragmentShader.replace('#include <map_pars_fragment>','#include <map_pars_fragment>\nuniform sampler2D uSkinNext;\nuniform float uSkinMix;')
     .replace('#include <map_fragment>',['#ifdef USE_MAP','\tvec4 incoSkinA = texture2D( map, vMapUv );','\tvec4 incoSkinB = texture2D( uSkinNext, vMapUv );','\tdiffuseColor *= mix( incoSkinA, incoSkinB, uSkinMix );','#endif'].join('\n'));};
   mat.needsUpdate=true;});

  const box=new THREE.Box3().setFromObject(model),size=box.getSize(new THREE.Vector3()),center=box.getCenter(new THREE.Vector3());model.position.sub(center);
  headBone=model.getObjectByName('head')??null;neckBone=model.getObjectByName('neck')??null;
  headRest=headBone?.rotation.clone()??null;neckRest=neckBone?.rotation.clone()??null;
  const fitDist=Math.max(size.x,size.y,size.z)/(2*Math.tan(camera.fov*Math.PI/360));
  if(framing==='bust'&&headBone){const head=headBone.getWorldPosition(new THREE.Vector3());camera.position.set(head.x,head.y,head.z+fitDist*.62);camera.lookAt(head);}
  else{camera.position.set(0,size.y*.04,fitDist*1.42);camera.lookAt(0,0,0);}

  const isEyelid=name=>/^eyelid_/.test(name),isHead=name=>/^(head|neck)\./.test(name);
  const byName=name=>gltf.animations.find(c=>c.name===name)??null;
  // Body clips: blinks own the eyelids and the cursor owns head/neck (spawn/despawn keep their head motion).
  const bodyClip=(name,keepHead=false)=>{const source=byName(name);if(!source)return null;const c=source.clone();c.tracks=c.tracks.filter(t=>!isEyelid(t.name)&&(keepHead||!isHead(t.name)));return c;};
  // The spawn clip is authored as an exit; shift its root so the last frame lands at the origin.
  const rebaseRootToEnd=c=>{const track=c?.tracks.find(t=>t.name==='root.position');if(!c||!track)return c;const v=track.values,n=v.length,ex=v[n-3],ey=v[n-2],ez=v[n-1];
   for(let i=0;i<n;i+=3){v[i]-=ex;v[i+1]-=ey;v[i+2]-=ez;}return c;};

  if(gltf.animations.length&&!reducedMotion){
   mixer=new THREE.AnimationMixer(model);
   const action=(clip,once)=>{if(!clip)return null;const a=mixer.clipAction(clip);if(once){a.setLoop(THREE.LoopOnce,1);a.clampWhenFinished=true;}return a;};
   const idle=action(bodyClip(CLIP.idle),false),think=action(bodyClip(CLIP.thinking),false);
   const spawn=action(rebaseRootToEnd(bodyClip(CLIP.spawn,true)),true),despawn=action(bodyClip(CLIP.despawn,true),true);
   const gestureSource=byName(CLIP.gesture);let gestureClip=null;
   if(gestureSource){gestureClip=gestureSource.clone();gestureClip.tracks=gestureClip.tracks.filter(t=>!isHead(t.name));}
   const gesture=action(gestureClip,true);
   const blinkSource=byName(CLIP.blink);let blinkClip=null;
   if(blinkSource){blinkClip=blinkSource.clone();blinkClip.tracks=blinkClip.tracks.filter(t=>isEyelid(t.name));}
   const blink=action(blinkClip,true);if(blink)blink.clampWhenFinished=false;

   let current=null;
   const modeAction=()=>thinking?(think??idle):idle;
   const playLoop=a=>{if(!a||current===a)return;a.reset().play();if(current)current.crossFadeTo(a,.3,false);current=a;lookSuppressed=false;};
   const playOnce=a=>{if(!a)return false;a.reset().play();if(current&&current!==a)current.crossFadeTo(a,.15,false);current=a;lookSuppressed=a===spawn||a===despawn;return true;};
   mixer.addEventListener('finished',e=>{if(e.action===current&&e.action!==despawn)playLoop(modeAction());});

   if(gestureSource){
    const sample=(boneName,time,amount,fromRest=false)=>{const bone=model.getObjectByName(boneName);if(!bone)return;const target={bone,amount};
     if(fromRest)target.rest={pos:bone.position.clone(),quat:bone.quaternion.clone(),scale:bone.scale.clone()};
     for(const path of ['position','quaternion','scale']){const track=gestureSource.tracks.find(t=>t.name===`${boneName}.${path}`);if(!track)continue;
      const v=track.createInterpolant().evaluate(time);
      if(path==='quaternion')target.quat=new THREE.Quaternion().fromArray(v);else if(path==='position')target.pos=new THREE.Vector3().fromArray(v);else target.scale=new THREE.Vector3().fromArray(v);}
     if(target.quat||target.pos||target.scale)thinkPose.push(target);};
    for(const b of THINK_ARM_BONES)sample(b,HANDS_ON_HEAD_TIME,1);
    for(const b of THINK_EYE_BONES)sample(b,EYES_CLOSED_TIME,SQUINT_AMOUNT,true);
    thinkPoseOn=()=>thinking&&!lookSuppressed&&current!==gesture;
    gesturePlaying=()=>current===gesture;
   }
   api={think:()=>{if(current===idle||current===think||current===null)playLoop(modeAction());},click:()=>{playOnce(gesture);nextSkin();}};
   const scheduleBlink=()=>{blinkTimer=setTimeout(()=>{if(blink&&current!==gesture&&!thinking)blink.reset().play();scheduleBlink();},BLINK_MIN_MS+Math.random()*BLINK_EXTRA_MS);};
   scheduleBlink();
   if(!playOnce(spawn))playLoop(modeAction());
  }else api={think(){},click:nextSkin};

  // First paint, then a still for the static avatars (read straight after rendering, before the buffer clears).
  renderer.render(scene,camera);
  if(onStill){const still=()=>{renderer.render(scene,camera);try{onStill(renderer.domElement.toDataURL('image/png'));}catch{}};
   // Wait for the skin so the still isn't grey, and for the spawn to settle into idle.
   const ready=()=>uSkinNext.value.image?setTimeout(still,reducedMotion?0:1400):setTimeout(ready,100);ready();}
  host.classList.add('ready');
 });

 const onPointerMove=e=>{const r=host.getBoundingClientRect();
  lookTarget.yaw=clamp((e.clientX-(r.left+r.width/2))/(innerWidth/2),-1,1)*.55;lookTarget.pitch=clamp((e.clientY-(r.top+r.height/2))/(innerHeight/2),-1,1)*.35;};
 if(!reducedMotion)addEventListener('pointermove',onPointerMove);

 const clock=new THREE.Clock();
 const tick=()=>{
  raf=requestAnimationFrame(tick);const dt=clock.getDelta();mixer?.update(dt);
  // Thinking pose layers over whatever loop is playing, weighted so it eases in and out.
  thinkWeight+=((thinkPoseOn()?1:0)-thinkWeight)*(1-Math.exp(-dt*4));
  if(thinkWeight>.001)for(const p of thinkPose){const w=thinkWeight*p.amount;
   if(p.rest){if(gesturePlaying())continue;
    if(p.quat)p.bone.quaternion.slerpQuaternions(p.rest.quat,p.quat,w);if(p.pos)p.bone.position.lerpVectors(p.rest.pos,p.pos,w);if(p.scale)p.bone.scale.lerpVectors(p.rest.scale,p.scale,w);}
   else{if(p.quat)p.bone.quaternion.slerp(p.quat,w);if(p.pos)p.bone.position.lerp(p.pos,w);if(p.scale)p.bone.scale.lerp(p.scale,w);}}
  if(!reducedMotion){const k=1-Math.exp(-dt*6);look.yaw+=(lookTarget.yaw-look.yaw)*k;look.pitch+=(lookTarget.pitch-look.pitch)*k;
   lookWeight+=((lookSuppressed?0:1)-lookWeight)*(1-Math.exp(-dt*2.5));const yaw=look.yaw*lookWeight,pitch=look.pitch*lookWeight;
   if(headBone&&headRest){headBone.rotation.y=headRest.y+yaw*.7;headBone.rotation.x=headRest.x+pitch*.7;}
   if(neckBone&&neckRest){neckBone.rotation.y=neckRest.y+yaw*.3;neckBone.rotation.x=neckRest.x+pitch*.25;}}
  if(fading&&skinMaterial&&uSkinNext.value.image){uSkinMix.value=Math.min(1,uSkinMix.value+dt/SKIN_FADE_SECONDS);if(uSkinMix.value>=1){skinMaterial.map=uSkinNext.value;uSkinMix.value=0;fading=false;}}
  renderer.render(scene,camera);
 };
 if(!reducedMotion)tick();
 const ro=new ResizeObserver(()=>{const w=host.clientWidth||1,h=host.clientHeight||1;camera.aspect=w/h;camera.updateProjectionMatrix();renderer.setSize(w,h);});ro.observe(host);

 return {
  think(on){thinking=on;api?.think();},
  click(){api?.click();},
  dispose(){disposed=true;cancelAnimationFrame(raf);clearTimeout(blinkTimer);ro.disconnect();removeEventListener('pointermove',onPointerMove);skinCache.forEach(t=>t?.dispose());pmrem.dispose();renderer.dispose();renderer.domElement.remove();},
 };
}
