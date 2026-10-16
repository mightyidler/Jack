import test from 'node:test';
import assert from 'node:assert/strict';
import { bottomEdgeGroup, edgeScrollDelta } from '../src/app-drag-geometry.ts';

const panel={left:0,right:400,top:0,bottom:680};
const card=(top,bottom)=>({left:12,right:388,top,bottom});

test('bottom padding retains the final group as its slot grows and collapses',()=>{
  for(const bottom of [612,640,668,700,724]) {
    assert.equal(bottomEdgeGroup(200,678,panel,[card(200,420),card(436,556),card(572,bottom)]),2);
  }
  assert.equal(bottomEdgeGroup(200,678,panel,[card(400,668),card(684,800)]),0);
});

test('edge fallback never captures points outside the panel/cards or ordinary gaps',()=>{
  const cards=[card(400,668)];
  for(const [x,y] of [[200,681],[200,600],[5,678],[395,678],[-1,678],[401,678]]) {
    assert.equal(bottomEdgeGroup(x,y,panel,cards),-1);
  }
  assert.equal(bottomEdgeGroup(200,678,panel,[]),-1);
});

test('edge scroll speed is independent of 60/120/144 Hz refresh rates',()=>{
  for(const hz of [60,120,144]) {
    const distance=Array.from({length:hz},()=>edgeScrollDelta(200,678,panel,1000/hz)).reduce((a,b)=>a+b,0);
    assert.ok(Math.abs(distance-600)<.001);
  }
  assert.equal(edgeScrollDelta(200,2,panel,1000/60),-10);
  assert.equal(edgeScrollDelta(200,340,panel,16),0);
});

test('a stalled frame or pointer outside the viewport cannot fling the list',()=>{
  assert.equal(edgeScrollDelta(200,678,panel,5000),19.2);
  assert.equal(edgeScrollDelta(200,678,panel,-5),0);
  assert.equal(edgeScrollDelta(200,681,panel,16),0);
  assert.equal(edgeScrollDelta(401,678,panel,16),0);
});

test('upper scroll zone starts below the header and continues when overshooting it',()=>{
  const headerBottom=116;
  assert.equal(edgeScrollDelta(200,118,panel,1000/60,headerBottom),-10);
  assert.equal(edgeScrollDelta(200,140,panel,1000/60,headerBottom),-6);
  assert.equal(edgeScrollDelta(200,100,panel,1000/60,headerBottom),-10);
  assert.equal(edgeScrollDelta(200,164,panel,1000/60,headerBottom),0);
  assert.equal(edgeScrollDelta(200,-1,panel,1000/60,headerBottom),0);
  assert.equal(edgeScrollDelta(200,678,panel,1000/60,headerBottom),10);
});

test('upper edge follows different header heights and panel offsets',()=>{
  for(const headerBottom of [96,114,150]) {
    const offsetPanel={left:20,right:420,top:30,bottom:710};
    assert.equal(edgeScrollDelta(220,headerBottom+2,offsetPanel,1000/60,headerBottom),-10);
    assert.equal(edgeScrollDelta(220,headerBottom+60,offsetPanel,1000/60,headerBottom),0);
  }
});
