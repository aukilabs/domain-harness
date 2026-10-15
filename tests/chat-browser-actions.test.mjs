import test from 'node:test';
import assert from 'node:assert/strict';
import {applyChatActions, chatResultPresentation} from '../web/view-camera.js';

const context = () => ({scene: 'home', frame: 'home-frame'});
const action = type => ({type, scene: 'home', frame: 'home-frame'});

test('browser actions execute in order and await each predecessor', async () => {
  const calls = [];
  const result = await applyChatActions([action('move_camera'), action('open_camera_pip')], {
    scene: 'home', context, apply: async value => {
      calls.push(value.type + ':start');
      await Promise.resolve();
      calls.push(value.type + ':end');
      return value.type + ' applied';
    },
  });
  assert.deepEqual(calls, ['move_camera:start', 'move_camera:end', 'open_camera_pip:start', 'open_camera_pip:end']);
  assert.equal(result.applied.length, 2);
  assert.equal(result.failure, null);
});

test('partial browser failure preserves receipts and never retries or runs the suffix', async () => {
  const calls = [];
  const result = await applyChatActions([action('move_camera'), action('open_camera_pip'), action('attach_camera')], {
    scene: 'home', context, apply: async value => {
      calls.push(value.type);
      if (value.type === 'open_camera_pip') throw Error('Camera is no longer available');
      return 'Map camera moved.';
    },
  });
  assert.deepEqual(calls, ['move_camera', 'open_camera_pip']);
  assert.equal(result.applied.length, 1);
  assert.equal(result.failure.index, 1);
  assert.equal(result.skipped, 1);
  const presentation = chatResultPresentation({reply: 'Both cameras are ready.', model: 'test', changes: []}, result);
  assert.match(presentation.content, /Map camera moved/);
  assert.match(presentation.content, /Camera not opened: Camera is no longer available/);
  assert.match(presentation.content, /1 remaining browser action was not attempted/);
  assert.doesNotMatch(presentation.content, /Both cameras are ready/);
});

test('changed scene prevents the next action while retaining earlier applied receipts', async () => {
  let selected = 'home', calls = 0;
  const result = await applyChatActions([action('move_camera'), action('detach_camera')], {
    scene: 'home', context: () => ({scene: selected, frame: 'home-frame'}), apply: async () => {
      calls++;
      selected = 'other';
      return 'Map camera moved.';
    },
  });
  assert.equal(calls, 1);
  assert.equal(result.applied.length, 1);
  assert.match(result.failure.message, /Selected scene changed/);
});

test('asynchronous reads recheck context immediately before their browser effect', async () => {
  let frame = 'home-frame', effects = 0;
  const result = await applyChatActions([action('open_camera_pip')], {
    scene: 'home', context: () => ({scene: 'home', frame}), apply: async (_value, guard) => {
      await Promise.resolve();
      frame = 'replacement-frame';
      guard();
      effects++;
    },
  });
  assert.equal(effects, 0);
  assert.equal(result.applied.length, 0);
  assert.match(result.failure.message, /Selected scene changed/);
});

test('unknown, malformed, stale, and over-limit actions do not run', async () => {
  let calls = 0;
  for (const actions of [null, {}, [null], [{type: 'drive_robot'}], [action('move_camera'), ...Array.from({length: 8}, () => action('detach_camera'))], [{...action('move_camera'), frame: 'old-frame'}]]) {
    const result = await applyChatActions(actions, {scene: 'home', context, apply: async () => calls++});
    assert.ok(result.failure);
    assert.equal(result.applied.length, 0);
  }
  assert.equal(calls, 0);
  const maximum = await applyChatActions(Array.from({length: 8}, () => action('detach_camera')), {scene: 'home', context, apply: async () => 'Detached.'});
  assert.equal(maximum.applied.length, 8);
  assert.equal(maximum.failure, null);
});

test('saved scene edits remain confirmed even if browser actions fail', () => {
  const presentation = chatResultPresentation({reply: 'Created and shown.', model: 'test', changes: [{summary: 'Created note cheese.', after: {id: 'cheese'}}]}, {
    applied: [], failure: {index: 0, type: 'move_camera', message: 'Selected scene changed'}, skipped: 0,
  });
  assert.match(presentation.display_content, /Created note cheese/);
  assert.match(presentation.display_content, /Map camera not moved/);
  assert.match(presentation.status, /saved 1 change confirmed by Home/);
  assert.doesNotMatch(presentation.display_content, /No scene changes were saved|Created and shown/);
  assert.doesNotMatch(presentation.status, /no scene changes saved/);
});

test('successful browser requests retain their actual waiting state', () => {
  const presentation = chatResultPresentation({reply: 'Camera selected.', changes: []}, {
    applied: [{type: 'open_camera_pip', message: 'Camera PiP opened · waiting for live frames.'}], failure: null, skipped: 0,
  });
  assert.match(presentation.content, /waiting for live frames/);
  assert.doesNotMatch(presentation.display_content, /No scene changes were saved/);
  assert.equal(chatResultPresentation({reply: 'Read only.', changes: []}, null).display_content, 'Read only.');
});

test('failed PiP preserves independent visual answers, server errors, and saved receipts', () => {
  const presentation = chatResultPresentation({
    reply: 'All requested operations succeeded, and the camera is visible.',
    camera_description: 'The captured image shows a yellow chair beside a desk.',
    errors: [{tool: 'move_note', error: 'Note moved concurrently; reread it', target: 'chair'}],
    changes: [{summary: 'Created note cheese.', after: {id: 'cheese'}}],
    status: 'partial', model: 'test',
  }, {
    applied: [], failure: {index: 0, type: 'open_camera_pip', message: 'Camera is no longer available'}, skipped: 0,
  });
  assert.match(presentation.content, /captured image shows a yellow chair/);
  assert.match(presentation.content, /Home tool move_note failed: Note moved concurrently/);
  assert.match(presentation.content, /Camera not opened: Camera is no longer available/);
  assert.match(presentation.display_content, /Created note cheese/);
  assert.match(presentation.status, /partial completion/);
  assert.doesNotMatch(presentation.display_content, /All requested operations succeeded|camera is visible|No scene changes were saved/);
});

test('structured visual answers and errors do not duplicate complete reply text', () => {
  const description = 'The captured image shows a blue wall.';
  const error = 'Peer tracking is unavailable';
  const presentation = chatResultPresentation({reply: `${error}\n\n${description}`, camera_description: description,
    errors: [{tool: 'get_peer_pose', error}], changes: [], status: 'incomplete'}, null);
  assert.equal(presentation.content.split(description).length - 1, 1);
  assert.equal(presentation.content.split(error).length - 1, 1);
  const finished = chatResultPresentation({reply: 'I need more information.', changes: [], status: 'finished'}, null);
  assert.doesNotMatch(finished.status, /completed|complete|finished/);
});

test('short answers keep explanations separate and saved summaries visible', () => {
  const result = chatResultPresentation({reply:'The closest pair is A and B, 1.19 m apart.', explanation:'Compared all three pairs.', changes:[]}, null);
  assert.equal(result.display_content, 'The closest pair is A and B, 1.19 m apart.');
  assert.equal(result.explanation, 'Compared all three pairs.');
  const saved = chatResultPresentation({reply:'Created note cheese.', explanation:'', changes:[{summary:'Created note cheese.',after:{id:'cheese'}}]}, null);
  assert.equal(saved.display_content, 'Created note cheese.');
  assert.match(saved.explanation, /"id": "cheese"/);
  const failed = chatResultPresentation({reply:'Opened.', explanation:'The camera is working.', changes:[]}, {
    applied:[],failure:{type:'open_camera_pip',message:'Peer unavailable'},skipped:0,
  });
  assert.equal(failed.explanation, '');
  assert.match(failed.display_content, /Peer unavailable/);
});
