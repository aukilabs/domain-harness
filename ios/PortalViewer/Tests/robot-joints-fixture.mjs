// Joint positions from the Home viewer's own URDF model (web/robot-model.js),
// for checking the phone's RobotKinematics against it. Writes
// Tests/robot-joints.json. Run from the repository root after changing the URDF
// or robot-model.js:
//   node ios/PortalViewer/Tests/robot-joints-fixture.mjs
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../../', import.meta.url));

// robot-model.js reads the URDF with DOMParser and a few child selectors; this
// is just enough XML for that. Meshes are not loaded (fetch fails for them).
class Element {
  constructor(tag, attributes, parent) { this.tagName = tag; this.attributes = attributes; this.parent = parent; this.children = []; }
  getAttribute(name) { return this.attributes[name] ?? null; }
  *descendants() { for (const c of this.children) { yield c; yield* c.descendants(); } }
  querySelectorAll(selector) {
    const parts = selector.split('>').map(s => s.trim());
    if (parts[0] === ':scope') return this.children.filter(c => c.tagName === parts[1]);
    if (parts.length === 1) return [...this.descendants()].filter(c => c.tagName === parts[0]);
    return [...this.descendants()].filter(c => c.tagName === parts[1] && c.parent?.tagName === parts[0]);
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
}
globalThis.DOMParser = class {
  parseFromString(text) {
    const document = new Element('#document', {}, null);
    let current = document;
    for (const [, close, tag, body, selfClose] of text.replace(/<\?[\s\S]*?\?>|<!--[\s\S]*?-->/g, '')
      .matchAll(/<(\/)?([\w:-]+)([^>]*?)(\/)?>/g)) {
      if (close) { current = current.parent; continue; }
      const attributes = Object.fromEntries([...body.matchAll(/([\w:-]+)="([^"]*)"/g)].map(m => [m[1], m[2]]));
      const element = new Element(tag, attributes, current);
      current.children.push(element);
      if (!selfClose) current = element;
    }
    return document;
  }
};
globalThis.fetch = async url => {
  const path = root + 'web' + url;
  try { const text = readFileSync(path, 'utf8'); return { ok: true, status: 200, text: async () => text, json: async () => JSON.parse(text) }; }
  catch { return { ok: false, status: 404 }; }
};
console.warn = () => {};

const { createRobotModel } = await import(root + 'web/robot-model.js');
const robot = await createRobotModel();

// Angles that move every joint off zero, inside its limits; grippers drive their mimics.
const angles = {
  lj0: -0.2, lj1: 0.4, lj2: -0.3, lj3: 0.5, lj4: -0.6, lj5: 0.7, lj6: -0.8,
  rj0: -0.3, rj1: -0.4, rj2: 0.3, rj3: -0.5, rj4: 0.6, rj5: -0.7, rj6: 0.8,
  left_left_gripper: 0.01, right_left_gripper: 0.02,
};
const poses = {};
for (const [label, values] of [['zero', {}], ['moved', angles]]) {
  for (const name of robot.joints) robot.setJoint(name, values[name] ?? 0);
  robot.root.updateMatrixWorld(true);
  poses[label] = { angles: values, positions: Object.fromEntries(robot.joints.map(name => {
    const p = robot.anchor(name).getWorldPosition(robot.anchor(name).position.clone());
    return [name, [p.x, p.y, p.z].map(v => Number(v.toFixed(6)))];
  })) };
}
writeFileSync(root + 'ios/PortalViewer/Tests/robot-joints.json', JSON.stringify(poses, null, 1) + '\n');
console.log(`Wrote ${robot.joints.length} joints in ${Object.keys(poses).length} poses`);
