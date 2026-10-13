// Layout only: moving/resizing a preview never reopens its camera subscription.
export function createPipWindow(pip) {
 const parent=pip.parentElement,heading=pip.querySelector('.sensor-pip-heading');
 const title=pip.querySelector('#sensor-title'),stage=pip.querySelector('#sensor-stage'),image=pip.querySelector('#sensor-image');
 const grip=document.createElement('button');
 grip.type='button';grip.className='pip-resize';grip.title='Resize camera preview';grip.setAttribute('aria-label',grip.title);pip.append(grip);
 title.tabIndex=0;title.setAttribute('role','button');title.setAttribute('aria-label','Move camera preview');
 heading.title='Drag to move camera preview';
 let layout=null,gesture=null,queued=false;
 const margin=8,clamp=(n,a,b)=>Math.max(a,Math.min(n,b));
 function metrics(){
  const p=parent.getBoundingClientRect(),r=pip.getBoundingClientRect();
  const ratio=image.width>0&&image.height>0?image.width/image.height:16/9;
  return {width:parent.clientWidth,height:parent.clientHeight,left:r.left-p.left-parent.clientLeft,top:r.top-p.top-parent.clientTop,
   boxWidth:r.width,boxHeight:r.height,ratio,chrome:r.height-stage.getBoundingClientRect().height};
 }
 function place(left,top,width,resize=false){
  if(pip.hidden)return;
  const m=metrics(),availableWidth=Math.max(1,m.width-2*margin),availableHeight=Math.max(1,m.height-2*margin);
  // Preserve the camera aspect ratio, keeping AR and image pixels co-registered.
  const maxWidth=Math.max(1,Math.min(availableWidth,(availableHeight-m.chrome)*m.ratio+2));
  let limit=maxWidth;
  if(resize)limit=Math.max(1,Math.min(limit,m.width-margin-left,(m.height-margin-top-m.chrome)*m.ratio+2));
  width=clamp(width,Math.min(220,limit),limit);
  pip.style.width=width+'px';
  const height=pip.getBoundingClientRect().height;
  layout={left:clamp(left,margin,Math.max(margin,m.width-width-margin)),top:clamp(top,margin,Math.max(margin,m.height-height-margin)),width};
  Object.assign(pip.style,{left:layout.left+'px',top:layout.top+'px',right:'auto',bottom:'auto'});
 }
 function fit(){
  if(pip.hidden)return;
  const m=metrics();place(layout?.left??m.left,layout?.top??m.top,layout?.width??m.boxWidth);
 }
 function schedule(){if(!queued){queued=true;requestAnimationFrame(()=>{queued=false;fit();});}}
 function start(e,kind){
  if(e.button!==0||!e.isPrimary||gesture)return;
  if(kind==='move'&&e.target.closest('button,input,label'))return;
  e.preventDefault();e.stopPropagation();fit();
  gesture={kind,id:e.pointerId,x:e.clientX,y:e.clientY,...layout,ratio:metrics().ratio,target:e.currentTarget};
  e.currentTarget.setPointerCapture(e.pointerId);pip.classList.add('pip-interacting');
 }
 function move(e){
  if(!gesture||gesture.id!==e.pointerId)return;
  e.preventDefault();e.stopPropagation();
  const g=gesture,dx=e.clientX-g.x,dy=e.clientY-g.y;
  if(g.kind==='move')place(g.left+dx,g.top+dy,g.width);
  else place(g.left,g.top,g.width+(Math.abs(dx)>=Math.abs(dy*g.ratio)?dx:dy*g.ratio),true);
 }
 function stop(e){
  if(!gesture||gesture.id!==e.pointerId)return;
  const g=gesture;gesture=null;pip.classList.remove('pip-interacting');
  if(g.target.hasPointerCapture(e.pointerId))g.target.releasePointerCapture(e.pointerId);
 }
 for(const [element,kind] of [[heading,'move'],[grip,'resize']]){
  element.addEventListener('pointerdown',e=>start(e,kind));
  element.addEventListener('pointermove',move);
  for(const event of ['pointerup','pointercancel','lostpointercapture'])element.addEventListener(event,stop);
  element.addEventListener('keydown',e=>{
   if(kind==='move'&&e.target!==title)return;
   if(!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(e.key))return;
   e.preventDefault();e.stopPropagation();fit();
   const step=e.shiftKey?32:8,dx=e.key==='ArrowLeft'?-step:e.key==='ArrowRight'?step:0,dy=e.key==='ArrowUp'?-step:e.key==='ArrowDown'?step:0;
   if(kind==='move')place(layout.left+dx,layout.top+dy,layout.width);
   else place(layout.left,layout.top,layout.width+(dx||dy*metrics().ratio),true);
  });
 }
 // Refit after opening, new image dimensions, status changes, and sidebar/window resizing.
 const observer=new ResizeObserver(schedule);observer.observe(parent);observer.observe(pip);
 new MutationObserver(schedule).observe(pip,{attributes:true,attributeFilter:['hidden']});
 return {fit};
}
