import test from 'node:test';
import assert from 'node:assert/strict';
import {deviceLabel,deviceLabels} from '../src/device-name.ts';
import {VolumeWheel} from '../src/volume-wheel.ts';
import {userError,errorCode} from '../src/user-error.ts';
import {outputVolume} from '../src/output-volume.ts';
import {controlsKey,applicationsKey,layoutKey} from '../src/view-update.ts';

function fixture() {
  return {settings:{theme:'dark',language:'ko',launch_at_login:true,channel_separation_enabled:true,link_group_outputs:false,
    groups:[{id:'game',display_name:'Game',sonar_channel:'game',device_volumes:{usb:0}}],microphone_device_volumes:{mic:.5}},
    audio:{endpoints:[],warnings:[]},separation_available:true,
    sonar:{connection:'connected',mode:'classic',message:null,volumes:[{channel:'game',volume:.63,muted:false}],
      routes:[{channel:'game',device_id:'usb',device_name:'Speakers (USB)'}],devices:[{id:'usb',name:'Speakers (USB)',role:'playback',enabled:true}],
      sources:[{channel:'game',endpoint_id:'game'}],applications:[]},app_routing:[]};
}

test('short device labels omit generic transport details and keep duplicate numbering stable',()=>{
  const devices=[{id:'b',name:'Speakers (USB)',role:'playback'},{id:'a',name:'Headphones (USB)',role:'playback'},
    {id:'c',name:'Speakers (USB)',role:'playback'},{id:'mic',name:'Microphone (USB)',role:'capture'}];
  const labels=deviceLabels(devices);
  assert.equal(labels.get('a'),'Headphones');assert.equal(labels.get('b'),'Speakers · 1');
  assert.equal(labels.get('c'),'Speakers · 2');assert.equal(labels.get('mic'),'Microphone');
  assert.equal(deviceLabel('Speakers (USB Audio Device)'),'Speakers');
  assert.equal(deviceLabel('Headphones (Razer BlackShark V2)'),'Razer BlackShark V2');
  assert.deepEqual(deviceLabels([...devices].reverse()),new Map([...labels].reverse()));
  assert.equal(devices[0].name,'Speakers (USB)');
});

test('wheel accumulates small deltas, supports line/page units and fine adjustment',()=>{
  const wheel=new VolumeWheel();
  assert.equal(wheel.step(-40,0,false,0),0);assert.equal(wheel.step(-60,0,false,10),.05);
  assert.equal(wheel.step(100,0,true,20),-.01);
  assert.equal(wheel.step(-3,1,false,30),.05);
  assert.equal(wheel.step(-1,2,true,40),.01);
});
test('wheel drops old or reversed residual deltas and rejects invalid input',()=>{
  const wheel=new VolumeWheel();
  wheel.step(-90,0,false,0);assert.equal(wheel.step(-10,0,false,300),0);
  assert.equal(wheel.step(90,0,false,310),0);assert.equal(wheel.step(10,0,false,320),-.05);
  assert.equal(wheel.step(NaN,0,false,330),0);wheel.reset();assert.equal(wheel.step(-10,0,false,340),0);
});

test('structured error codes select localized actionable text without exposing paths or raw details',()=>{
  const error={code:'settings_save',detail:'Access denied C:\\Users\\private\\settings.json'};
  assert.match(userError(error),/저장.*권한/);assert.match(userError(error,true),/save settings/);
  assert.doesNotMatch(userError(error),/private|denied|object Object/);
  assert.equal(errorCode(new Error('Volume changed externally; refresh before retrying')),'external_change');
  assert.equal(errorCode('Sonar is not ready; audio recovery: failed'),'recovery');
  assert.equal(errorCode({code:'unknown',detail:'secret'}),'internal');
});

test('disconnected output preserves remembered zero, distinguishes absent volume and never fabricates a zero reading',()=>{
  const state=fixture();assert.equal(outputVolume(state,'game').level,.63);
  state.sonar.devices[0].enabled=false;
  assert.deepEqual(outputVolume(state,'game'),{available:false,level:0,disconnected:true});
  delete state.settings.groups[0].device_volumes.usb;assert.equal(outputVolume(state,'game').level,.63);
  state.sonar.volumes=[];assert.equal(outputVolume(state,'game').level,null);
  state.sonar.devices[0].enabled=true;assert.equal(outputVolume(state,'game').disconnected,false);
  assert.equal(outputVolume(state,'game').available,false);
});

test('volume values and remembered gains do not remount controls; missing volumes and routes do update device controls',()=>{
  const before=fixture(),after=structuredClone(before);
  after.sonar.volumes[0].volume=.4;after.settings.groups[0].device_volumes.usb=.4;after.settings.microphone_device_volumes.mic=0;
  assert.equal(controlsKey(before),controlsKey(after));
  after.sonar.volumes=[];assert.notEqual(controlsKey(before),controlsKey(after));assert.equal(layoutKey(before),layoutKey(after));
  after.sonar.routes[0].device_id='other';assert.notEqual(controlsKey(before),controlsKey(after));assert.equal(layoutKey(before),layoutKey(after));
});
test('application changes leave controls intact; language and group changes rebuild the layout',()=>{
  const before=fixture(),after=structuredClone(before);
  after.sonar.applications.push({process_id:1,display_name:'Music',route:'game'});
  assert.equal(controlsKey(before),controlsKey(after));assert.notEqual(applicationsKey(before),applicationsKey(after));
  after.settings.sonar_app_overrides={'path:app':'chat'};
  assert.equal(layoutKey(before),layoutKey(after),'persisted routing intent must not remount unrelated controls');
  after.settings.language='en';assert.notEqual(layoutKey(before),layoutKey(after));
});
