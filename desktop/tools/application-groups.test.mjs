import test from 'node:test';
import assert from 'node:assert/strict';
import { groupApplications } from '../src/application-groups.ts';

const app = (pid, overrides = {}) => ({process_id:pid,process_name:'Player',display_name:'Player',route:'media',activity:'active',routing_error:false,...overrides});
const identity = (pid, overrides = {}) => ({process_id:pid,stable_id:'path:c:/player.exe',requested:'media',origin:'user',message:null,...overrides});

test('verified multi-process app renders once and keeps all members without mutating input', () => {
  const sessions = [app(10,{activity:'inactive'}),app(20)];
  const before = JSON.stringify(sessions);
  const result = groupApplications(sessions,[identity(10),identity(20)]);
  assert.equal(result.length,1);
  assert.equal(result[0].session.process_id,20);
  assert.deepEqual(result[0].process_ids,[20,10]);
  assert.equal(JSON.stringify(sessions),before);
  assert.equal(groupApplications([app(10)],[identity(10)])[0].session.process_id,10);
});
test('same names with different or unverified identity stay distinct', () => {
  const sessions = [app(10),app(20)];
  assert.equal(groupApplications(sessions,[identity(10),identity(20,{stable_id:'aumid:another-app'})]).length,2);
  assert.equal(groupApplications(sessions,[]).length,2);
});
test('conflicting effective routes or assignment origins are not hidden by aggregation', () => {
  assert.equal(groupApplications([app(10),app(20,{route:'chat'})],[identity(10),identity(20)]).length,2);
  assert.equal(groupApplications([app(10),app(20)],[identity(10),identity(20,{origin:'external'})]).length,2);
});
test('an error in any member remains visible on the merged row', () => {
  const result = groupApplications([app(10),app(20,{routing_error:true})],[identity(10,{message:'Pending'}),identity(20,{message:'Pending'})]);
  assert.equal(result.length,1);
  assert.equal(result[0].session.routing_error,true);
});
test('verified pending policy moves the row while inactive effective session stays unchanged', () => {
  const session = app(10,{activity:'inactive'});
  const pending = identity(10,{requested:'chat',pending:true,message:'Waiting for playback'});
  assert.equal(groupApplications([session],[pending])[0].session.route,'chat');
  assert.equal(session.route,'media');
  assert.equal(groupApplications([session],[{...pending,pending:false,message:'Routing failed'}])[0].session.route,'media');
  assert.equal(groupApplications([session],[{...pending,stable_id:null}])[0].session.route,'media');
});
