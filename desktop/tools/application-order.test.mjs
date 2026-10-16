import test from 'node:test';
import assert from 'node:assert/strict';
import { ApplicationOrder, applicationOrderKey } from '../src/application-order.ts';

const sort=(order,channel,keys)=>order.sort(channel,keys,key=>key);

test('drop positions survive inventory refresh: middle, first and last, in and across groups',()=>{
  const order=new ApplicationOrder();
  order.place('chat','game-app','b',['a','b']);
  assert.deepEqual(sort(order,'chat',['game-app','a','b']),['a','game-app','b']);
  order.place('chat','game-app',null,['a','game-app','b']);
  assert.deepEqual(sort(order,'chat',['game-app','a','b']),['a','b','game-app']);
  order.place('chat','game-app','a',['a','b','game-app']);
  assert.deepEqual(sort(order,'chat',['b','a','game-app']),['game-app','a','b']);
  order.place('media','game-app',null,[]);
  assert.deepEqual(sort(order,'media',['game-app']),['game-app']);
  assert.deepEqual(JSON.parse(order.serialize()).chat,['a','b']);
});

test('verified identities retain order after restart and PID/representative changes',()=>{
  const key=pid=>applicationOrderKey({session:{process_id:pid},process_ids:[pid]},new Map([[pid,{stable_id:'path:player.exe'}]]));
  const order=new ApplicationOrder();
  order.place('media',key(10),null,['music','browser']);
  const reopened=new ApplicationOrder(order.serialize());
  assert.equal(key(10),key(200));
  assert.deepEqual(sort(reopened,'media',[key(200),'browser','music']),['music','browser',key(200)]);
  assert.deepEqual(sort(reopened,'media',['browser','new-app','music']),['music','browser','new-app']);
  assert.deepEqual(sort(reopened,'media',[key(200),'browser','music']),['music','browser',key(200)]);
});

test('failed assignment can roll back exactly without losing the source order',()=>{
  const order=new ApplicationOrder('{"game":["a","moving","b"],"chat":["c","d"]}');
  const before=order.serialize();
  const undo=order.place('chat','moving','d',['c','d']);
  assert.deepEqual(sort(order,'chat',['moving','d','c']),['c','moving','d']);
  undo();
  assert.equal(order.serialize(),before);
});

test('corrupt preferences and unknown apps never hide or mutate the inventory',()=>{
  const inventory=['x','y','z'];
  for(const saved of ['broken','null','[]','{"game":["x",42,"x"]}']) {
    const order=new ApplicationOrder(saved);
    const sorted=sort(order,'game',inventory);
    assert.deepEqual([...sorted].sort(),inventory);
    assert.deepEqual(inventory,['x','y','z']);
  }
});
