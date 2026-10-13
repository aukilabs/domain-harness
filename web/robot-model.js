import * as THREE from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { DRACOLoader } from 'three/addons/loaders/DRACOLoader.js';

// Minimal URDF -> three.js builder for the Bracketbot visual model in web/robot/.
// Supports fixed, revolute, continuous and prismatic joints plus <mimic>; mesh files are
// resolved through manifest.json to Draco GLBs. The returned root is the URDF
// root link in ROS body axes (+X forward, +Y left, +Z up), so it can replace the
// pose glyph's box directly. A mesh that fails to load is skipped, never fatal.

const BASE = '/robot/';
const dracoLoader = new DRACOLoader().setDecoderPath('/vendor/');
const gltfLoader = new GLTFLoader().setDRACOLoader(dracoLoader);
const geometries = new Map();
let definition = null;

function numbers(text, fallback) {
 const values = (text || '').trim().split(/\s+/).map(Number);
 return values.length === 3 && values.every(Number.isFinite) ? values : fallback;
}

function applyOrigin(object, origin) {
 object.position.fromArray(numbers(origin?.getAttribute('xyz'), [0, 0, 0]));
 const [roll, pitch, yaw] = numbers(origin?.getAttribute('rpy'), [0, 0, 0]);
 // URDF rpy is fixed-axis X, then Y, then Z: R = Rz(yaw) Ry(pitch) Rx(roll).
 object.quaternion.setFromEuler(new THREE.Euler(roll, pitch, yaw, 'ZYX'));
}

async function load() {
 const [urdf, manifest] = await Promise.all([
  fetch(BASE + 'robot.urdf').then(r => { if (!r.ok) throw Error('robot.urdf ' + r.status); return r.text(); }),
  fetch(BASE + 'manifest.json').then(r => { if (!r.ok) throw Error('manifest.json ' + r.status); return r.json(); }),
 ]);
 const doc = new DOMParser().parseFromString(urdf, 'application/xml');
 if (doc.querySelector('parsererror')) throw Error('robot.urdf is not valid XML');
 const glb = new Map(Object.values(manifest).map(e => [e.stl, BASE + 'draco/' + e.draco.split('/').pop()]));
 return { doc, glb };
}

function geometry(url) {
 if (!geometries.has(url)) {
  geometries.set(url, gltfLoader.loadAsync(url).then(gltf => {
   const meshes = [];
   gltf.scene.updateMatrixWorld(true);
   gltf.scene.traverse(o => { if (o.isMesh) meshes.push({ geometry: o.geometry, matrix: o.matrixWorld.clone() }); });
   return meshes;
  }));
 }
 return geometries.get(url);
}

export async function createRobotModel() {
 definition ??= load();
 const { doc, glb } = await definition;
 const links = new Map(), joints = new Map(), children = new Set();
 for (const link of doc.querySelectorAll('robot > link')) {
  const group = new THREE.Group();
  group.name = link.getAttribute('name');
  links.set(group.name, group);
  for (const visual of link.querySelectorAll(':scope > visual')) {
   const file = visual.querySelector('geometry > mesh')?.getAttribute('filename');
   const url = glb.get(file);
   if (!url) continue;
   const holder = new THREE.Group();
   applyOrigin(holder, visual.querySelector(':scope > origin'));
   group.add(holder);
   const rgba = (visual.querySelector('material > color')?.getAttribute('rgba') || '0.8 0.8 0.8 1').split(/\s+/).map(Number);
   const material = new THREE.MeshStandardMaterial({ color: new THREE.Color(rgba[0], rgba[1], rgba[2]), roughness: .65, metalness: .1 });
   geometry(url).then(parts => {
    for (const part of parts) {
     const mesh = new THREE.Mesh(part.geometry, material);
     mesh.matrixAutoUpdate = false;
     mesh.matrix.copy(part.matrix);
     holder.add(mesh);
    }
   }).catch(e => console.warn('Robot mesh failed to load', url, e));
  }
 }
 for (const joint of doc.querySelectorAll('robot > joint')) {
  const parent = links.get(joint.querySelector('parent')?.getAttribute('link'));
  const child = links.get(joint.querySelector('child')?.getAttribute('link'));
  if (!parent || !child) continue;
  const origin = new THREE.Group(), motion = new THREE.Group();
  applyOrigin(origin, joint.querySelector(':scope > origin'));
  parent.add(origin); origin.add(motion); motion.add(child);
  children.add(child.name);
  const type = joint.getAttribute('type');
  if (type !== 'fixed') {
   const limit = joint.querySelector(':scope > limit'), mimic = joint.querySelector(':scope > mimic');
   joints.set(joint.getAttribute('name'), {
    type, motion,
    axis: new THREE.Vector3(...numbers(joint.querySelector(':scope > axis')?.getAttribute('xyz'), [1, 0, 0])).normalize(),
    lower: Number(limit?.getAttribute('lower') ?? -Infinity),
    upper: Number(limit?.getAttribute('upper') ?? Infinity),
    mimic: mimic && { joint: mimic.getAttribute('joint'), multiplier: Number(mimic.getAttribute('multiplier') ?? 1), offset: Number(mimic.getAttribute('offset') ?? 0) },
   });
  }
 }
 const followers = new Map();
 for (const [name, j] of joints) if (j.mimic) followers.set(j.mimic.joint, [...(followers.get(j.mimic.joint) || []), name]);
 function move(j, value) {
  const v = j.type === 'continuous' ? value : Math.min(j.upper, Math.max(j.lower, value));
  if (j.type === 'prismatic') { j.motion.position.copy(j.axis).multiplyScalar(v); j.motion.quaternion.identity(); }
  else { j.motion.position.set(0, 0, 0); j.motion.quaternion.setFromAxisAngle(j.axis, v); }
 }
 const roots = [...links.values()].filter(l => !children.has(l.name));
 if (roots.length !== 1) throw Error(`robot.urdf needs one root link, found ${roots.length}`);
 const root = new THREE.Group();
 root.add(roots[0]);
 return {
  root,
  joints: [...joints.keys()],
  // The moving frame of a joint, for attaching things that follow it.
  anchor(name) { return joints.get(name)?.motion ?? null; },
  setJoint(name, value) {
   const j = joints.get(name);
   if (!j || !Number.isFinite(value)) return false;
   move(j, value);
   for (const follower of followers.get(name) || []) {
    const f = joints.get(follower);
    move(f, value * f.mimic.multiplier + f.mimic.offset);
   }
   return true;
  },
 };
}
