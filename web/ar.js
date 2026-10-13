import * as THREE from 'three';

// Intrinsics describe the full rectified image, with pixel origin at top left.
export function projectionMatrix({fx,fy,cx,cy},width,height,near=.03,far=40){
 return new THREE.Matrix4().set(
  2*fx/width,0,1-2*cx/width,0,
  0,2*fy/height,2*cy/height-1,0,
  0,0,-(far+near)/(far-near),-2*far*near/(far-near),
  0,0,-1,0);
}
export function createArOverlay(canvas){
 const renderer=new THREE.WebGLRenderer({canvas,alpha:true,antialias:true,preserveDrawingBuffer:true});
 renderer.setClearColor(0,0);renderer.autoClear=false;renderer.outputColorSpace=THREE.SRGBColorSpace;
 const labels=new THREE.Scene(),labelCamera=new THREE.OrthographicCamera(-1,1,1,-1,0,2);labelCamera.position.z=1;
 const paths=new THREE.Group(),pathCamera=new THREE.Camera();
 let rotation=0;
 function setRotation(degrees){rotation=degrees*Math.PI/180;for(const label of labels.children)label.material.rotation=rotation;
  if(!canvas.hidden){renderer.clear();renderer.render(paths,pathCamera);renderer.clearDepth();renderer.render(labels,labelCamera);}}
 function clearPaths(){for(const child of [...paths.children]){child.geometry.dispose();child.material.dispose();paths.remove(child);}}
 function clearLabels(){for(const child of [...labels.children]){labels.remove(child);child.material.map.dispose();child.material.dispose();}}
 function hide(){canvas.hidden=true;canvas.dataset.voxelCount='0';canvas.dataset.noteCount='0';canvas.dataset.pathCount='0';clearPaths();clearLabels();renderer.clear();}
 function show(frame,displayRotation=0){
  const ar=frame.ar;if(ar?.status!=='ready')throw Error('Calibrated AR frame unavailable');
  renderer.setSize(frame.width,frame.height,false);renderer.clear();clearPaths();
  rotation=displayRotation*Math.PI/180;
  // Pixel-space projection keeps a counter-rotated label's proportions correct
  // on non-square images. Only the glyph rotates; its projected anchor does not.
  labelCamera.left=0;labelCamera.right=frame.width;labelCamera.top=frame.height;labelCamera.bottom=0;labelCamera.updateProjectionMatrix();
  // PiP shows annotations only. Screen-space ribbons remain legible regardless
  // of camera distance, and a dark outline separates them from the camera image.
  const pixelScale=frame.width/(canvas.clientWidth||frame.width);
  for(const [width,colorOverride] of [[9,'#17121d'],[5,null]]){
   for(const segment of ar.path_segments||[]){
    const positions=pathRibbonPositions(segment.points,frame.width,frame.height,width*pixelScale);
    const geometry=new THREE.BufferGeometry();geometry.setAttribute('position',new THREE.Float32BufferAttribute(positions,3));
    paths.add(new THREE.Mesh(geometry,new THREE.MeshBasicMaterial({color:colorOverride||segment.color,depthTest:false,depthWrite:false,side:THREE.DoubleSide})));
   }
  }
  renderer.render(paths,pathCamera);
  clearLabels();
  for(const note of ar.notes||[]){
   const c=document.createElement('canvas');const ctx=c.getContext('2d');ctx.font='600 32px system-ui';c.width=Math.min(2048,Math.ceil(ctx.measureText(note.text).width)+32);c.height=64;
   ctx.fillStyle='#142b24';ctx.beginPath();ctx.roundRect(2,2,c.width-4,60,12);ctx.fill();ctx.strokeStyle='#f8df7b';ctx.lineWidth=3;ctx.stroke();
   ctx.fillStyle='#fff4bb';ctx.font='600 32px system-ui';ctx.textAlign='center';ctx.fillText(note.text,c.width/2,43,c.width-24);
   const texture=new THREE.CanvasTexture(c);texture.colorSpace=THREE.SRGBColorSpace;
   const sprite=new THREE.Sprite(new THREE.SpriteMaterial({map:texture,depthTest:false,depthWrite:false,rotation}));
   sprite.position.set(note.pixel[0],frame.height-note.pixel[1],0);
   const height=Math.max(22,frame.height*.07),width=Math.min(frame.width*.8,height*c.width/64);
   sprite.scale.set(width,height,1);labels.add(sprite);
  }
  renderer.clearDepth();renderer.render(labels,labelCamera);
  canvas.dataset.noteCount=String((ar.notes||[]).length);
  canvas.hidden=false;canvas.dataset.voxelCount='0';canvas.dataset.pathCount=String((ar.path_segments||[]).length);canvas.dataset.frame=ar.frame;canvas.dataset.sequence=String(frame.sequence);
 }
 return {show,hide,setRotation};
}

// A ribbon, rather than WebGL lineWidth (usually fixed to one pixel).
export function pathRibbonPositions(points,width,height,thickness){
 const vertices=[],half=thickness/2;
 const push=(x,y)=>vertices.push(2*x/width-1,1-2*y/height,0);
 for(let i=1;i<points.length;i++){
  const [ax,ay]=points[i-1],[bx,by]=points[i],dx=bx-ax,dy=by-ay,length=Math.hypot(dx,dy);
  if(length<1e-6)continue;
  const nx=-dy/length*half,ny=dx/length*half;
  for(const [x,y] of [[ax+nx,ay+ny],[ax-nx,ay-ny],[bx+nx,by+ny],[bx+nx,by+ny],[ax-nx,ay-ny],[bx-nx,by-ny]])push(x,y);
 }
 return vertices;
}
