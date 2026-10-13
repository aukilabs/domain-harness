// Browser-local viewpoint action. Never dispatch device commands or scene edits.
export function moveMapCamera(action, {scene, frame, camera, controls, map2D}) {
  if (action.type !== 'move_camera' || action.scene !== scene || action.frame !== frame)
    throw Error('Selected scene changed; ask again in the current scene');
  const vector = v => Array.isArray(v) && v.length === 3 && v.every(n => Number.isFinite(n) && Math.abs(n) <= 10000);
  if (!vector(action.position) || !vector(action.look_at)) throw Error('Invalid camera coordinates');
  const distance = Math.hypot(...action.position.map((n, i) => n - action.look_at[i]));
  if (distance < .01 || distance > 60) throw Error('Invalid camera target distance');
  if (map2D.isActive()) map2D.toggle();
  // Flush pending orbit damping before installing the requested view, so an old
  // drag cannot pull it away on the following animation frame.
  const damping = controls.enableDamping;
  controls.enableDamping = false;
  controls.update();
  controls.minDistance = .01;
  controls.maxDistance = 60;
  controls.minPolarAngle = 0;
  controls.maxPolarAngle = Math.PI;
  camera.up.set(0, 1, 0);
  camera.position.fromArray(action.position);
  controls.target.fromArray(action.look_at);
  controls.update();
  camera.updateMatrixWorld();
  controls.enableDamping = damping;
  return 'Map camera moved.';
}
