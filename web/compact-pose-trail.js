import * as THREE from 'three';

// Display conversion only: never relabel native coordinates as Home coordinates.
export function displayBasis(frame) {
 const directions={right:[1,0,0],left:[-1,0,0],up:[0,1,0],down:[0,-1,0],backward:[0,0,1],back:[0,0,1],forward:[0,0,-1]};
 if(frame?.units!=='meters'||frame.handedness!=='right')throw Error('Unsupported registry frame convention');
 const axes=['x','y','z'].map(k=>directions[frame.axes?.[k]]);
 if(axes.some(a=>!a))throw Error('Unknown registry frame axes');
 const basis=new THREE.Matrix4().makeBasis(...axes.map(a=>new THREE.Vector3(...a)));
 if(Math.abs(basis.determinant()-1)>1e-6)throw Error('Invalid registry frame axes');
 return basis;
}

export function createCompactPoseTrail(scene) {
 const group=new THREE.Group();scene.add(group);
 function reset(){for(const child of [...group.children]){group.remove(child);child.geometry.dispose();child.material.dispose();}}
 function update(data){
  const contract=data.resolved?.contract;if(!contract)throw Error('Registry-resolved compact poses required');
  const basis=displayBasis(contract.to_frame);
  const records=data.history||[{observation:data.observation}];
  reset();
  for(const [i,record] of records.entries()){
   const payload=record.observation.payload;
   if(Object.hasOwn(payload,'contract'))throw Error('Legacy pose rejected');
   const p=payload.tracking?.status==='valid'?payload.tracking.pose:null;if(!p)continue;
   const matrix=new THREE.Matrix4().compose(new THREE.Vector3(...p.translation),new THREE.Quaternion(...p.rotation_xyzw),new THREE.Vector3(1,1,1)).premultiply(basis);
   const points=[[0,0,0],[.16,0,0],[0,0,0],[0,.16,0],[0,0,0],[0,0,.16]].map(a=>new THREE.Vector3(...a).applyMatrix4(matrix));
   const glyph=new THREE.LineSegments(new THREE.BufferGeometry().setFromPoints(points),new THREE.LineBasicMaterial({color:0x76dfff,transparent:true,opacity:.1+.8*(i+1)/records.length}));group.add(glyph);
  }
  return {frame:contract.to_frame.frame_id,count:group.children.length,status:data.observation.payload.tracking.status};
 }
 return {reset,update};
}
