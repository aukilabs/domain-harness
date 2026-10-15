import * as THREE from 'three';

// Robot motor health drawn on the URDF model: a badge at each joint (dot
// coloured by temperature, ring filled by current, the numbers beside it) and a
// battery gauge above the head. Badges are camera-facing sprites drawn over the
// model so they stay readable from any side.

// BBOS drops a motor's torque after 15 s above 50 C (sustained_temp_limit).
const TEMP_COOL_C = 35, TEMP_WARM_C = 42, TEMP_LIMIT_C = 50;
const CURRENT_FULL_A = 3;
// Above the head, in the model's ROS body frame (+Z up); the model is ~1.57 m.
const BATTERY_HEIGHT_M = 1.70;

const COOL = new THREE.Color('#22c55e'), WARM = new THREE.Color('#f59e0b'), HOT = new THREE.Color('#ef4444');

export function temperatureColor(celsius) {
 if (celsius <= TEMP_COOL_C) return COOL.clone();
 if (celsius <= TEMP_WARM_C) return COOL.clone().lerp(WARM, (celsius - TEMP_COOL_C) / (TEMP_WARM_C - TEMP_COOL_C));
 return WARM.clone().lerp(HOT, Math.min(1, (celsius - TEMP_WARM_C) / (TEMP_LIMIT_C - TEMP_WARM_C)));
}

export function batteryColor(percent) {
 return percent >= 50 ? '#22c55e' : percent >= 20 ? '#f59e0b' : '#ef4444';
}

function sprite(width, height, scale, center) {
 const canvas = document.createElement('canvas');
 canvas.width = width; canvas.height = height;
 const texture = new THREE.CanvasTexture(canvas);
 texture.colorSpace = THREE.SRGBColorSpace;
 const s = new THREE.Sprite(new THREE.SpriteMaterial({ map: texture, transparent: true, depthTest: false }));
 s.scale.set(scale * width / height, scale, 1);
 s.center.copy(center);
 s.renderOrder = 20;
 s.userData.key = null;
 return s;
}

function pill(ctx, x, y, w, h) {
 ctx.fillStyle = 'rgba(11,15,20,0.82)';
 ctx.beginPath(); ctx.roundRect(x, y, w, h, h / 2); ctx.fill();
}

function drawJoint(s, temperature, current) {
 const key = `${temperature.toFixed(0)}|${current.toFixed(1)}`;
 if (s.userData.key === key) return;
 s.userData.key = key;
 const canvas = s.material.map.image, ctx = canvas.getContext('2d');
 ctx.clearRect(0, 0, canvas.width, canvas.height);
 pill(ctx, 2, 4, canvas.width - 4, canvas.height - 8);
 const cx = 34, cy = canvas.height / 2, hot = temperature >= TEMP_LIMIT_C;
 // Current ring: track, then the filled share of CURRENT_FULL_A.
 ctx.lineWidth = 6; ctx.lineCap = 'round';
 ctx.strokeStyle = 'rgba(148,163,184,0.35)';
 ctx.beginPath(); ctx.arc(cx, cy, 22, 0, Math.PI * 2); ctx.stroke();
 const share = Math.min(1, Math.abs(current) / CURRENT_FULL_A);
 if (share > 0.01) {
  ctx.strokeStyle = '#38bdf8';
  ctx.beginPath(); ctx.arc(cx, cy, 22, -Math.PI / 2, -Math.PI / 2 + share * Math.PI * 2); ctx.stroke();
 }
 // Temperature dot, with a red halo at the limit.
 if (hot) { ctx.fillStyle = 'rgba(239,68,68,0.35)'; ctx.beginPath(); ctx.arc(cx, cy, 30, 0, Math.PI * 2); ctx.fill(); }
 ctx.fillStyle = '#' + temperatureColor(temperature).getHexString();
 ctx.beginPath(); ctx.arc(cx, cy, 15, 0, Math.PI * 2); ctx.fill();
 ctx.fillStyle = '#f8fafc'; ctx.textBaseline = 'middle';
 ctx.font = '700 30px system-ui, sans-serif';
 ctx.fillText(`${temperature.toFixed(0)}°`, 66, cy - 11);
 ctx.fillStyle = '#7dd3fc'; ctx.font = '600 22px system-ui, sans-serif';
 ctx.fillText(`${current.toFixed(1)} A`, 66, cy + 15);
 s.material.map.needsUpdate = true;
}

function drawBattery(s, percent, voltage) {
 const key = `${percent.toFixed(0)}|${voltage.toFixed(1)}`;
 if (s.userData.key === key) return;
 s.userData.key = key;
 const canvas = s.material.map.image, ctx = canvas.getContext('2d');
 ctx.clearRect(0, 0, canvas.width, canvas.height);
 pill(ctx, 2, 4, canvas.width - 4, canvas.height - 8);
 const color = batteryColor(percent);
 // Battery body, terminal nub and the fill.
 const x = 22, y = 22, w = 96, h = 52;
 ctx.lineWidth = 5; ctx.strokeStyle = '#e2e8f0';
 ctx.beginPath(); ctx.roundRect(x, y, w, h, 8); ctx.stroke();
 ctx.fillStyle = '#e2e8f0'; ctx.beginPath(); ctx.roundRect(x + w + 3, y + 16, 8, 20, 3); ctx.fill();
 ctx.fillStyle = color;
 ctx.beginPath(); ctx.roundRect(x + 6, y + 6, Math.max(4, (w - 12) * percent / 100), h - 12, 4); ctx.fill();
 ctx.textBaseline = 'middle';
 ctx.fillStyle = color; ctx.font = '700 40px system-ui, sans-serif';
 ctx.fillText(`${percent.toFixed(0)}%`, 140, 40);
 ctx.fillStyle = '#cbd5e1'; ctx.font = '500 22px system-ui, sans-serif';
 ctx.fillText(`${voltage.toFixed(1)} V`, 142, 74);
 s.material.map.needsUpdate = true;
}

function dispose(s) { s.parent?.remove(s); s.material.map.dispose(); s.material.dispose(); }

// Badge columns beside each arm, in the model's ROS body frame (+Y left, +Z up):
// badges keep their joint's height but are pushed apart vertically, with a
// leader line back to the joint, so stacked wrist joints stay readable.
const COLUMN_Y_M = 0.36, BADGE_SPACING_M = 0.085;
// Share of the way from each joint out to its column the badge sits at.
const COLUMN_REACH = 0.5;
const sideOf = name => /^(l|left)/.test(name) ? 1 : -1;

function dot(color) {
 const s = new THREE.Mesh(new THREE.SphereGeometry(0.018, 16, 12), new THREE.MeshBasicMaterial({ color, depthTest: false }));
 s.renderOrder = 19;
 return s;
}

function leader() {
 const line = new THREE.Line(new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(), new THREE.Vector3()]),
  new THREE.LineBasicMaterial({ transparent: true, opacity: 0.8, depthTest: false }));
 line.renderOrder = 18;
 return line;
}

function remove(entry) {
 dispose(entry.badge);
 for (const o of [entry.dot, entry.line]) { o.parent?.remove(o); o.geometry.dispose(); o.material.dispose(); }
}

// Show `health` ({joints: {name: {temperature_c, current_a}}, battery: {voltage,
// percent}}) on the glyph's URDF model; null hides it. Joints the model lacks
// are skipped, and markers for joints no longer reported are removed.
export function applyRobotHealth(glyph, health) {
 glyph.healthMarkers ??= new Map();
 const robot = glyph.robot;
 const joints = robot ? Object.fromEntries(Object.entries(health?.joints || {}).filter(([name, v]) =>
  robot.anchor(name) && Number.isFinite(v?.temperature_c) && Number.isFinite(v?.current_a))) : {};
 for (const [name, entry] of glyph.healthMarkers) {
  if (!(name in joints) || entry.dot.parent !== robot?.anchor(name)) { remove(entry); glyph.healthMarkers.delete(name); }
 }
 if (Object.keys(joints).length) {
  glyph.group.updateMatrixWorld(true);
  const local = name => glyph.group.worldToLocal(robot.anchor(name).getWorldPosition(new THREE.Vector3()));
  for (const side of [1, -1]) {
   // Highest joint first; each badge at its joint's height or below the previous one.
   const column = Object.keys(joints).filter(n => sideOf(n) === side).map(n => [n, local(n)]).sort((a, b) => b[1].z - a[1].z);
   let lowest = Infinity;
   for (const [name, at] of column) {
    const value = joints[name], color = temperatureColor(value.temperature_c);
    let entry = glyph.healthMarkers.get(name);
    if (!entry) {
     entry = { badge: sprite(176, 76, 0.07, new THREE.Vector2(side > 0 ? 0 : 1, 0.5)), dot: dot(color), line: leader() };
     robot.anchor(name).add(entry.dot);
     glyph.group.add(entry.badge, entry.line);
     glyph.healthMarkers.set(name, entry);
    }
    const z = Math.min(at.z, lowest - BADGE_SPACING_M);
    lowest = z;
    entry.badge.position.set(at.x, at.y + (side * COLUMN_Y_M - at.y) * COLUMN_REACH, z);
    entry.line.geometry.setFromPoints([at, entry.badge.position]);
    entry.dot.material.color.copy(color);
    entry.line.material.color.copy(color);
    drawJoint(entry.badge, value.temperature_c, value.current_a);
   }
  }
 }
 const battery = robot && health?.battery;
 if (!battery || !Number.isFinite(battery.percent) || !Number.isFinite(battery.voltage)) {
  if (glyph.batteryBadge) { dispose(glyph.batteryBadge); glyph.batteryBadge = null; }
  return;
 }
 if (!glyph.batteryBadge) {
  glyph.batteryBadge = sprite(256, 96, 0.11, new THREE.Vector2(0.5, 0));
  glyph.batteryBadge.position.set(0, 0, BATTERY_HEIGHT_M);
  glyph.group.add(glyph.batteryBadge);
 }
 drawBattery(glyph.batteryBadge, battery.percent, battery.voltage);
}

// The latest pointing request above the battery: blue while the arm works,
// red with the robot's own reason when it refused or stopped.
const POINTING_TEXT = {planned: 'Pointing: planned', moving: 'Pointing…', holding: 'Pointing', returning: 'Pointing: returning', done: 'Pointing: done'};
function drawPointing(s, pointing) {
 const failed = pointing.phase === 'rejected' || pointing.phase === 'fault';
 const title = failed ? (pointing.phase === 'fault' ? 'Pointing stopped' : 'Pointing refused') : (POINTING_TEXT[pointing.phase] || pointing.phase);
 const detail = failed ? (pointing.reason || pointing.code || '') : (pointing.arm || '').replace('arm_', '') + (pointing.arm ? ' arm' : '');
 const key = `${title}|${detail}`;
 if (s.userData.key === key) return;
 s.userData.key = key;
 const canvas = s.material.map.image, ctx = canvas.getContext('2d');
 ctx.clearRect(0, 0, canvas.width, canvas.height);
 pill(ctx, 2, 4, canvas.width - 4, canvas.height - 8);
 ctx.fillStyle = failed ? '#ef4444' : '#38bdf8';
 ctx.beginPath(); ctx.arc(34, canvas.height / 2, 14, 0, Math.PI * 2); ctx.fill();
 ctx.textBaseline = 'middle';
 ctx.fillStyle = failed ? '#fca5a5' : '#e0f2fe'; ctx.font = '700 28px system-ui, sans-serif';
 ctx.fillText(title, 62, canvas.height / 2 - 14);
 ctx.fillStyle = '#cbd5e1'; ctx.font = '500 22px system-ui, sans-serif';
 let text = detail;
 while (text.length > 4 && ctx.measureText(text).width > canvas.width - 80) text = text.slice(0, -2);
 ctx.fillText(text === detail ? text : text + '…', 62, canvas.height / 2 + 16);
 s.material.map.needsUpdate = true;
}

// Show `pointing` ({request_id, phase, arm, reason, code}) on the glyph's model; null hides it.
export function applyPointingStatus(glyph, pointing) {
 if (!glyph.robot || !pointing?.phase) {
  if (glyph.pointingBadge) { dispose(glyph.pointingBadge); glyph.pointingBadge = null; }
  return;
 }
 if (!glyph.pointingBadge) {
  glyph.pointingBadge = sprite(480, 96, 0.11, new THREE.Vector2(0.5, 0));
  glyph.pointingBadge.position.set(0, 0, BATTERY_HEIGHT_M + 0.13);
  glyph.group.add(glyph.pointingBadge);
 }
 drawPointing(glyph.pointingBadge, pointing);
}
