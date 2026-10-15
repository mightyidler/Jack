import test from 'node:test';
import assert from 'node:assert/strict';
import { LiveVolume } from '../src/live-volume.ts';

test('a held gesture sends immediately and coalesces slow I/O to the latest value', async () => {
  const calls=[];let release;
  const slider=new LiveVolume(.5,(level,expected) => {
    calls.push({level,expected});return new Promise(resolve => {release=() => resolve(level);});
  },assert.fail,0);
  slider.set(.3);assert.deepEqual(calls,[{level:.3,expected:.5}]);
  for(let i=0;i<100;i++)slider.set(i/100);
  assert.equal(calls.length,1);
  const done=slider.finish();release();await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(calls,[{level:.3,expected:.5},{level:.99,expected:.3}]);
  release();await done;
});
test('80ms rate limit keeps the latest trailing value and release flushes it', async () => {
  const calls=[];const slider=new LiveVolume(.5,async level => {calls.push({level,at:performance.now()});return level;},assert.fail);
  slider.set(.2);await new Promise(resolve => setImmediate(resolve));
  slider.set(.3);slider.set(.4);
  await new Promise(resolve => setTimeout(resolve,100));
  assert.equal(calls.length,2);assert.equal(calls[1].level,.4);assert.ok(calls[1].at-calls[0].at>=75);
  slider.set(.8);await slider.finish();assert.equal(calls.at(-1).level,.8);
});
test('cancel discards unsent values and a failed request never replays a mutation', async () => {
  const calls=[];let release;let failure;
  const slider=new LiveVolume(.5,level => {calls.push(level);return new Promise(resolve => {release=() => resolve(level);});},assert.fail);
  slider.set(.2);slider.set(.9);const ended=slider.finish(false);release();await ended;assert.deepEqual(calls,[.2]);
  const broken=new LiveVolume(.5,async () => {throw Error('external change');},error => {failure=error;},0);
  broken.set(.4);broken.set(.8);await broken.finish();assert.equal(failure.message,'external change');
});
