// Browser-local viewpoint actions. Never dispatch device commands or scene edits.
const cameraActionTypes = new Set(['move_camera', 'attach_camera', 'detach_camera', 'open_camera_pip']);
const cameraFailureLabels = {
  move_camera: 'Map camera not moved', attach_camera: 'Camera not attached',
  detach_camera: 'Camera not detached', open_camera_pip: 'Camera not opened',
};

// Execute once in server order. A failure preserves applied receipts and stops
// the remaining actions; it never replays the successful prefix.
export async function applyChatActions(actions, {scene, context, apply}) {
  const applied = [];
  if (!Array.isArray(actions) || actions.length > 8) {
    return {applied, failure: {index: 0, message: 'Expected at most 8 browser actions'}, skipped: 0};
  }
  for (let index = 0; index < actions.length; index++) {
    const action = actions[index];
    const guard = () => {
      const current = context();
      if (current.scene !== scene || (action.scene !== undefined && action.scene !== current.scene)
        || (action.frame !== undefined && action.frame !== current.frame))
        throw Error('Selected scene changed; ask again in the current scene');
    };
    try {
      if (!action || !cameraActionTypes.has(action.type)) throw Error('Unsupported camera action');
      guard();
      // Asynchronous executors also call guard after awaited reads and before
      // applying their side effect, so changing scenes cannot redirect it.
      const message = await apply(action, guard);
      applied.push({type: action.type, message});
    } catch (error) {
      return {applied, failure: {index, type: action?.type, message: error?.message || String(error)}, skipped: actions.length - index - 1};
    }
  }
  return {applied, failure: null, skipped: 0};
}

export function chatResultPresentation(result, browser) {
  const changes = Array.isArray(result.changes) ? result.changes : [];
  const errors = Array.isArray(result.errors) ? result.errors : [];
  const description = typeof result.camera_description === 'string' ? result.camera_description : '';
  const actionMessages = (browser?.applied || []).map(action => action.message).filter(Boolean);
  if (browser?.failure) {
    const failure = browser.failure;
    actionMessages.push(`${cameraFailureLabels[failure.type] || 'Browser action not applied'}: ${failure.message}`);
    if (browser.applied.length) actionMessages.push(`${browser.applied.length} earlier browser action${browser.applied.length === 1 ? ' was' : 's were'} applied and not replayed.`);
    if (browser.skipped) actionMessages.push(`${browser.skipped} remaining browser action${browser.skipped === 1 ? ' was' : 's were'} not attempted.`);
  }
  // A server tool receipt only requests a browser action. In particular, never
  // show the model's success claim when the browser could not apply it.
  const reply = browser?.failure ? '' : typeof result.reply === 'string' ? result.reply : '';
  const parts = [reply];
  // These are independent server results, not the planner's action claims.
  // Keep successful visual answers and failed server steps even when PiP fails.
  if (description && !reply.includes(description)) parts.push(description);
  for (const error of errors) {
    if (typeof error?.error !== 'string' || reply.includes(error.error)) continue;
    parts.push(`Home tool ${error.tool || 'operation'} failed: ${error.error}`);
  }
  // Saved summaries stay visible, including when a later browser action fails.
  // Raw saved-object JSON belongs in the expandable explanation, not the reply.
  for (const change of changes) {
    const summary = change.summary || 'Saved scene change.';
    if (!parts.some(part => part.includes(summary))) parts.push(summary);
  }
  parts.push(...actionMessages);
  const content = parts.filter(Boolean).join('\n\n') || 'Home returned no reply.';
  const saved = changes.map(change => {
    const after = typeof change.after === 'string' ? change.after
      : change.after !== undefined ? JSON.stringify(change.after, null, 2) : '';
    return [change.summary || 'Saved scene change.', after].filter(Boolean).join('\n');
  }).join('\n\n');
  const explanation = browser?.failure
    ? (typeof result.camera_explanation === 'string' ? result.camera_explanation : '')
    : (typeof result.explanation === 'string' ? result.explanation : '');
  const details = [explanation.trim(), saved].filter(Boolean).join('\n\n');
  const state = changes.length ? `saved ${changes.length} change${changes.length === 1 ? '' : 's'} confirmed by Home` : 'no scene changes saved';
  const failed = browser?.failure || errors.length || ['partial', 'incomplete'].includes(result.status);
  const completion = failed ? (changes.length || browser?.applied?.length || description ? 'partial completion' : 'incomplete') : '';
  return {
    content,
    display_content: content,
    explanation: details,
    // `finished` means the server loop ended, not that every user intent was
    // satisfied or that requested browser effects have rendered successfully.
    status: `${result.source === 'command' ? 'Home · direct command' : result.model || 'Qwen'} · ${state}` + (completion ? ' · ' + completion : '') + (actionMessages.length ? ' · ' + actionMessages.join(' ') : ''),
  };
}

export function moveMapCamera(action, {scene, frame, camera, controls, map2D, cameraPan, beforeMove = () => {}}) {
  if (action.type !== 'move_camera' || action.scene !== scene || action.frame !== frame)
    throw Error('Selected scene changed; ask again in the current scene');
  const vector = v => Array.isArray(v) && v.length === 3 && v.every(n => Number.isFinite(n) && Math.abs(n) <= 10000);
  if (!vector(action.position) || !vector(action.look_at)) throw Error('Invalid camera coordinates');
  const distance = Math.hypot(...action.position.map((n, i) => n - action.look_at[i]));
  if (distance < .01 || distance > 60) throw Error('Invalid camera target distance');
  // Validate before detaching a followed sensor or changing the current view.
  beforeMove();
  if (map2D.isActive()) map2D.toggle();
  controls.minDistance = .01;
  controls.maxDistance = 60;
  controls.minPolarAngle = 0;
  controls.maxPolarAngle = Math.PI;
  camera.up.set(0, 1, 0);
  // Share double-click's easing, damping flush, retargeting and input cancellation.
  // Build separate destinations: never mutate the live camera before the flight.
  cameraPan.start(camera.position.clone().fromArray(action.position), controls.target.clone().fromArray(action.look_at));
  return 'Map camera moving.';
}
