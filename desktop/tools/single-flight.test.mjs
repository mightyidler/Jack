import test from 'node:test';
import assert from 'node:assert/strict';
import {singleFlight} from '../src/single-flight.ts';

test('slow polling and focus events share one read and never create a trailing queue',async()=>{
  let calls=0;let release;
  const read=singleFlight(()=>{calls++;return new Promise(resolve=>{release=resolve;});});
  const first=read();
  const waiting=Array.from({length:100},()=>read());
  assert.ok(waiting.every(value=>value===first));
  await Promise.resolve();assert.equal(calls,1);
  release('snapshot');
  assert.deepEqual(await Promise.all(waiting),Array(100).fill('snapshot'));
  assert.equal(calls,1);
  const next=read();await Promise.resolve();assert.equal(calls,2);release('next');
  assert.equal(await next,'next');
});

test('failed reads release the gate, including synchronous transport failures',async()=>{
  let calls=0;
  const read=singleFlight(()=>{if(++calls===1)throw Error('transport closed');return Promise.resolve('reconnected');});
  await assert.rejects(read(),/transport closed/);
  assert.equal(await read(),'reconnected');
});
