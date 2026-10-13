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
 const scene=new THREE.Scene(),camera=new THREE.PerspectiveCamera();
 const geometry=new THREE.BoxGeometry(1,1,1);
 const material=new THREE.MeshBasicMaterial({transparent:true,opacity:.42,depthWrite:false,vertexColors:true,side:THREE.DoubleSide});
 const depth=new THREE.MeshBasicMaterial({colorWrite:false,side:THREE.DoubleSide});
 const labels=new THREE.Scene(),labelCamera=new THREE.OrthographicCamera(-1,1,1,-1,0,2);labelCamera.position.z=1;
 let mesh=null;const paths=new THREE.Group();
 function clearLabels(){for(const child of [...labels.children]){labels.remove(child);child.material.map.dispose();child.material.dispose();}}
 function hide(){canvas.hidden=true;canvas.dataset.voxelCount='0';canvas.dataset.noteCount='0';clearLabels();renderer.clear();}
 function show(frame){
  for(const child of [...paths.children]){child.geometry.dispose();child.material.dispose();paths.remove(child);}scene.remove(paths);
  const ar=frame.ar;if(ar?.status!=='ready')throw Error('Calibrated AR frame unavailable');
  if(mesh){scene.remove(mesh);if(mesh.isInstancedMesh)mesh.dispose();else mesh.geometry.dispose();}
  let renderCamera=camera;
  if(['sdk-camera','sdk-fisheye'].includes(ar.projection)){
   renderer.setSize(frame.width,frame.height,false);
   const positions=[],colors=[];
   for(const [u,v,z,r,g,b] of ar.vertices){
    positions.push(2*u/frame.width-1,1-2*v/frame.height,(40+.03)/(40-.03)-2*40*.03/((40-.03)*z));colors.push(r,g,b);
   }
   const projected=new THREE.BufferGeometry();projected.setAttribute('position',new THREE.Float32BufferAttribute(positions,3));projected.setAttribute('color',new THREE.Float32BufferAttribute(colors,3));
   mesh=new THREE.Mesh(projected,material);mesh.frustumCulled=false;renderCamera=new THREE.Camera();
  }else{
  renderer.setSize(frame.width,frame.height,false);
  camera.projectionMatrix.copy(projectionMatrix(ar.intrinsics,frame.width,frame.height));camera.projectionMatrixInverse.copy(camera.projectionMatrix).invert();
  camera.position.fromArray(ar.camera_to_map.translation);
  // SDK preserves optical-local axes (+X right,+Y down,+Z forward).
  // Three cameras look along -Z with +Y up, so rotate only the camera-local basis.
  camera.quaternion.fromArray(ar.camera_to_map.rotation_xyzw).multiply(new THREE.Quaternion(1,0,0,0));camera.updateMatrixWorld(true);
  mesh=new THREE.InstancedMesh(geometry,material,ar.voxels.length);mesh.frustumCulled=false;
  const matrix=new THREE.Matrix4(),color=new THREE.Color(),scale=new THREE.Vector3().setScalar(ar.voxel_size);
  ar.voxels.forEach((voxel,i)=>{
   matrix.compose(new THREE.Vector3(...voxel.center),new THREE.Quaternion(),scale);mesh.setMatrixAt(i,matrix);
   if(voxel.linear_rgb)color.setRGB(...voxel.linear_rgb,THREE.LinearSRGBColorSpace);else color.set('#39d6e0');mesh.setColorAt(i,color);
  });
  }
  scene.add(mesh);
  // Draw the closest voxel surface only, avoiding stacked transparent cube faces.
  renderer.clear();scene.overrideMaterial=depth;renderer.render(scene,renderCamera);scene.overrideMaterial=null;renderer.render(scene,renderCamera);
  for(const segment of ar.path_segments||[]){const positions=segment.points.flatMap(([u,v,z])=>[2*u/frame.width-1,1-2*v/frame.height,(40+.03)/(40-.03)-2*40*.03/((40-.03)*z)]);const geometry=new THREE.BufferGeometry();geometry.setAttribute('position',new THREE.Float32BufferAttribute(positions,3));paths.add(new THREE.Line(geometry,new THREE.LineBasicMaterial({color:segment.color,depthTest:true,depthWrite:false})));}
  if(paths.children.length)renderer.render(paths,new THREE.Camera());
  clearLabels();
  for(const note of ar.notes||[]){
   const c=document.createElement('canvas');const ctx=c.getContext('2d');ctx.font='600 32px system-ui';c.width=Math.min(2048,Math.ceil(ctx.measureText(note.text).width)+32);c.height=64;
   ctx.fillStyle='#142b24';ctx.beginPath();ctx.roundRect(2,2,c.width-4,60,12);ctx.fill();ctx.strokeStyle='#f8df7b';ctx.lineWidth=3;ctx.stroke();
   ctx.fillStyle='#fff4bb';ctx.font='600 32px system-ui';ctx.textAlign='center';ctx.fillText(note.text,c.width/2,43,c.width-24);
   const texture=new THREE.CanvasTexture(c);texture.colorSpace=THREE.SRGBColorSpace;
   const sprite=new THREE.Sprite(new THREE.SpriteMaterial({map:texture,depthTest:false,depthWrite:false}));
   sprite.position.set(2*note.pixel[0]/frame.width-1,1-2*note.pixel[1]/frame.height,0);
   const height=Math.max(22,frame.height*.07),width=Math.min(frame.width*.8,height*c.width/64);
   sprite.scale.set(2*width/frame.width,2*height/frame.height,1);labels.add(sprite);
  }
  renderer.clearDepth();renderer.render(labels,labelCamera);
  canvas.dataset.noteCount=String((ar.notes||[]).length);
  canvas.hidden=false;canvas.dataset.voxelCount=String(ar.voxel_count??ar.voxels.length);canvas.dataset.frame=ar.frame;canvas.dataset.sequence=String(frame.sequence);
 }
 return {show,hide};
}
