import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';

test('map cubes use the full grid cell size without decorative seams', () => {
  const source = readFileSync(new URL('../web/app.js', import.meta.url), 'utf8');
  const rendering = source.slice(source.indexOf('data.voxels.forEach'), source.indexOf("voxels.userData.kind"));
  assert.match(rendering, /const scale=data\.voxel_size;/);
  assert.match(rendering, /new THREE\.Vector3\(scale,scale,scale\)/);
  for (const size of [0.03, 0.1]) {
    const first = 0.5 * size, second = 1.5 * size;
    assert.ok(Math.abs((first + size / 2) - (second - size / 2)) < 1e-12);
  }
});
