import * as THREE from 'three';

// The map camera's rig: the camera and the orbit target it circles, and the one place their limits live.
// The keyboard and the wheel move it through translate() and dolly(). Everything else (OrbitControls, flights,
// Fit, the assistant) may set the camera and target directly; settle(), last thing each frame, keeps the result
// above the floor.
// - The camera stays `clearance` above the floor and the target never sinks below it.
// - A keyboard or wheel move rests the target where the view meets the floor, within OrbitControls' distance
//   limits, so it never floats in empty air beside the camera (the fog, label tiers and move and pan speeds all
//   scale with the orbit distance). Sinking stops where that point would come nearer than the minimum distance,
//   so OrbitControls never has to push the camera back up. A target set any other way (a flight to a note, Fit,
//   the assistant) stays where it was put until the keyboard or wheel moves it.
export function createCameraRig({camera,controls,floor=()=>-Infinity,clearance=.05,paused=()=>false}){
 const view=new THREE.Vector3(),move=new THREE.Vector3(),start=new THREE.Vector3();
 const near=()=>controls.minDistance||0,far=()=>controls.maxDistance??Infinity;
 // Distance along `direction` from the camera down to the floor, or Infinity when the view doesn't descend.
 function toFloor(direction){const height=camera.position.y-floor();return direction.y<-1e-9&&Number.isFinite(height)?height/-direction.y:Infinity;}
 // Keeps the camera above the floor and, looking along `direction`, high enough that the floor stays the minimum
 // distance away. It never lifts the camera: one already lower only stops sinking. True when it held the camera.
 function keepUp(previousY,direction){
  const low=floor()+Math.max(clearance,near()*Math.max(0,-direction.y));if(camera.position.y>=low)return false;
  camera.position.y=Math.max(camera.position.y,Math.min(previousY,low));return true;
 }
 // Rests the target where `direction` meets the floor, within the distance limits; with no floor ahead it keeps
 // `distance`.
 function anchor(direction,distance){
  const hit=toFloor(direction),d=THREE.MathUtils.clamp(Number.isFinite(hit)?hit:distance,near(),far());
  controls.target.copy(camera.position).addScaledVector(direction,d);
  controls.target.y=d===hit?floor():Math.max(controls.target.y,floor());
 }
 // The unit view direction (camera to target) into `view`; returns the orbit distance.
 function aim(){view.subVectors(controls.target,camera.position);const distance=view.length();view.divideScalar(distance||1);return distance;}
 return {
  // Moves camera and target together by `step` (the keyboard). True when the floor stopped the camera sinking.
  translate(step){
   const previousY=camera.position.y,distance=aim();
   camera.position.add(step);const held=keepUp(previousY,view);anchor(view,distance);
   return held;
  },
  // The wheel: moves the camera along `ray` (unit length, through the cursor) rather than zooming toward the
  // target. `step` is the zoom amount on OrbitControls' log scale (negative is forward), taken as that share of the
  // way to the floor under the cursor, capped at the orbit distance or the height, whichever is more: steps shrink
  // near the floor and never pass through it. The target keeps its height where the view meets it while that stays
  // within the distance limits. Past the near one it moves onto the floor ahead, so the camera flies on instead of
  // stopping at a target floating in the air; backing away stops at the far one. False when it can go no further.
  dolly(ray,step){
   const distance=aim(),height=camera.position.y-floor(),level=controls.target.y;start.copy(camera.position);
   const reach=Math.min(toFloor(ray),Math.max(distance,Number.isFinite(height)?height:0));
   move.copy(ray).multiplyScalar(reach*(1-Math.exp(step)));if(move.lengthSq()<1e-12)return false;
   camera.position.add(move);const held=keepUp(start.y,view);
   const along=Math.abs(view.y)>1e-9?(level-camera.position.y)/view.y:distance;
   if(along>=near()&&along<=far()){controls.target.copy(camera.position).addScaledVector(view,along);controls.target.y=level;}
   else if(step>0){camera.position.copy(start);return false;}
   else anchor(view,distance);
   return !held;
  },
  // Last thing each frame (after OrbitControls, flights and the keyboard). Orbiting stays above the horizon, but
  // views the assistant sets (and detaching an attached camera) lift that limit, so the floor is held here too.
  settle(){
   if(paused())return;
   const low=floor();
   if(camera.position.y<low+clearance)camera.position.y=low+clearance;
   if(controls.target.y<low)controls.target.y=low;
  },
 };
}
