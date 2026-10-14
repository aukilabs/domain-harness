// One display transform for RGB and AR. All camera operations remain in raw pixels.
export function displaySize(width,height,rotation){return rotation%180?[height,width]:[width,height];}
export function nativePixel(x,y,width,height,rotation){
 switch(rotation){
  case 90:return [y,height-x];
  case 180:return [width-x,height-y];
  case 270:return [width-y,x];
  default:return [x,y];
 }
}
export function createVideoPresentation(stage,heading,onChange=()=>{}){
 const layer=document.createElement('div');layer.className='sensor-presentation';
 layer.append(...stage.childNodes);stage.append(layer);
 const select=document.createElement('select');select.setAttribute('aria-label','Camera display orientation');select.className='sensor-orientation';
 for(const [value,text] of [['auto','Auto ↻'],['0','0°'],['90','90° ↻'],['180','180°'],['270','270° ↻']])select.add(new Option(text,value));
 heading.insertBefore(select,heading.querySelector('#sensor-close'));
 let key='',width=16,height=9,auto=0,rotation=0,candidate=null,confirmations=0,source='unavailable';
 function apply(){
  rotation=select.value==='auto'?auto:Number(select.value);
  const [w,h]=displaySize(width,height,rotation);
  stage.style.aspectRatio=w+'/'+h;stage.dataset.displayRatio=String(w/h);stage.dataset.rotation=String(rotation);
  layer.style.width=(rotation%180?100*width/height:100)+'%';
  layer.style.transform=({0:'none',90:'rotate(90deg) translateY(-100%)',180:'rotate(180deg) translate(-100%,-100%)',270:'rotate(270deg) translateX(-100%)'})[rotation];
  select.title=select.value!=='auto'?'Saved display rotation for this feed':source==='unavailable'?'Automatic orientation unavailable; retaining last display angle. Choose an angle if needed.':'Upright from '+(source==='capture_pose'?'capture-time pose':'publisher declaration');
  onChange();
 }
 select.addEventListener('change',()=>{try{localStorage.setItem('pip-orientation:'+key,select.value);}catch{}apply();});
 return {
  select(id){key=id;auto=0;candidate=null;confirmations=0;source='unavailable';let saved;try{saved=localStorage.getItem('pip-orientation:'+key);}catch{}
   select.value=['0','90','180','270'].includes(saved)?saved:'auto';apply();},
  update(frame){width=frame.width;height=frame.height;
   const p=frame.presentation,angle=p?.rotation_clockwise_deg;
   if([0,90,180,270].includes(angle)){
    // Require two frames for subsequent quarter-turn changes to avoid boundary jitter.
    if(candidate===angle)confirmations++;else{candidate=angle;confirmations=1;}
    if(source==='unavailable'||p.source==='declaration'||confirmations>=2)auto=angle;
    source=p.source;
   }else{source='unavailable';}
   apply();
  },
  pixel(clientX,clientY){const rect=stage.getBoundingClientRect(),[w,h]=displaySize(width,height,rotation);
   return nativePixel((clientX-rect.left)/rect.width*w,(clientY-rect.top)/rect.height*h,width,height,rotation);},
 };
}
