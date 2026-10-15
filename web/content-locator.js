import * as THREE from 'three';

// Content locator: when the scene's content is off screen, or has faded into the distance, a glass pill hugs the
// edge of the free screen area pointing at it, with the distance; clicking it flies back. Styles live in style.css
// under "Content locator". It shows after a short wait (350 ms, so brief misses never flash it) and hides at once.
// options: fog() → THREE.Fog, blocked() → true during flights, the 2D plan or an attached camera, safeArea() →
// {left, top, right, bottom} in client px (the canvas minus panels and bars), fly(centre, radius).
export function createContentLocator({camera,renderer,fog,blocked,safeArea,fly,rects=()=>null}){
 const button=document.createElement('button');button.type='button';button.className='content-locator';button.hidden=true;button.title='Back to content';
 button.innerHTML='<svg class="locator-arrow" width="16" height="16" viewBox="0 0 18 18" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M7 4.5 11.5 9 7 13.5"/></svg><span class="locator-distance"></span>';
 renderer.domElement.parentElement.append(button);
 const arrow=button.querySelector('.locator-arrow'),label=button.querySelector('.locator-distance');
 const centre=new THREE.Vector3(),corner=new THREE.Vector3(),nearest=new THREE.Vector3(),view=new THREE.Vector3();
 let bounds=null,radius=0,shown=false,wantSince=0,area=null,areaAt=0,lastText='';
 button.onclick=()=>{if(bounds)fly(centre.clone(),radius);};
 const smooth=(a,b,x)=>{const t=Math.min(1,Math.max(0,(x-a)/(b-a)));return t*t*(3-2*t);};
 function setBounds(box){bounds=box&&!box.isEmpty()?box.clone():null;if(bounds){bounds.getCenter(centre);radius=bounds.getSize(corner).length()/2;}else hide();}
 function show(){if(shown)return;shown=true;button.hidden=false;button.classList.remove('leaving');void button.offsetWidth;button.classList.add('shown');}
 function hide(){if(!shown)return;shown=false;wantSince=0;button.classList.remove('shown');button.classList.add('leaving');setTimeout(()=>{if(!shown){button.hidden=true;button.classList.remove('leaving');}},160);}
 function tick(now=performance.now()){
  if(!bounds||blocked()){hide();return;}
  if(!area||now-areaAt>250){area=safeArea();areaAt=now;}
  // Is any of the content's box on screen inside the free area, and how faded is its nearest point?
  let left=Infinity,top=Infinity,right=-Infinity,bottom=-Infinity,inFront=false;const shared=rects(),rect=shared?.canvas||renderer.domElement.getBoundingClientRect();
  for(const x of [bounds.min.x,bounds.max.x])for(const y of [bounds.min.y,bounds.max.y])for(const z of [bounds.min.z,bounds.max.z]){
   corner.set(x,y,z).project(camera);if(corner.z<-1||corner.z>1)continue;inFront=true;
   const sx=rect.left+(corner.x+1)/2*rect.width,sy=rect.top+(1-corner.y)/2*rect.height;left=Math.min(left,sx);right=Math.max(right,sx);top=Math.min(top,sy);bottom=Math.max(bottom,sy);}
  const onScreen=inFront&&right>area.left&&left<area.right&&bottom>area.top&&top<area.bottom;
  const f=fog(),depth=camera.position.distanceTo(bounds.clampPoint(camera.position,nearest)),fade=f?1-smooth(f.near,f.far,depth):1;
  const want=shown?!(onScreen&&fade>.45):(!onScreen||fade<.25);
  if(!want){hide();return;}
  if(!shown){wantSince||=now;if(now-wantSince<350)return;show();}
  // Point at the content's centre: on the edge of the free area along its direction from the area's centre, or,
  // when that centre is on screen but faded, right on it as a beacon pointing down.
  view.copy(centre).applyMatrix4(camera.matrixWorldInverse);
  const cx=(area.left+area.right)/2,cy=(area.top+area.bottom)/2,projected=corner.copy(centre).project(camera);
  const px=rect.left+(projected.x+1)/2*rect.width,py=rect.top+(1-projected.y)/2*rect.height,inset=28;
  let x,y,angle;
  if(projected.z>-1&&projected.z<1&&px>area.left+inset&&px<area.right-inset&&py>area.top+inset&&py<area.bottom-inset){x=px;y=py-26;angle=90;}
  else{let dx=view.x,dy=-view.y;const length=Math.hypot(dx,dy);if(length<1e-6){dx=0;dy=1;}else{dx/=length;dy/=length;}
   const tx=dx>0?(area.right-inset-cx)/dx:dx<0?(area.left+inset-cx)/dx:Infinity,ty=dy>0?(area.bottom-inset-cy)/dy:dy<0?(area.top+inset-cy)/dy:Infinity,t=Math.max(0,Math.min(tx,ty));
   x=cx+dx*t;y=cy+dy*t;angle=Math.atan2(dy,dx)*180/Math.PI;}
  const host=shared?.host||button.parentElement.getBoundingClientRect();
  button.style.transform=`translate(${x-host.left}px,${y-host.top}px) translate(-50%,-50%)`;arrow.style.transform=`rotate(${angle}deg)`;
  const metres=camera.position.distanceTo(centre),rounded=metres<100?Math.round(metres/5)*5:Math.round(metres/10)*10,text=rounded+' m';
  if(text!==lastText){lastText=text;label.textContent=text;button.setAttribute('aria-label',`Content off-screen, ${rounded} metres away. Return to content`);}
 }
 return {setBounds,tick,get centre(){return bounds?centre.clone():null;},get radius(){return radius;}};
}
