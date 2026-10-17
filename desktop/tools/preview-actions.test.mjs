import test from 'node:test';
import assert from 'node:assert/strict';
import {applyPreviewAction} from '../src/preview-actions.ts';

function fixture() {
  const channels=['game','chat','media'];
  return {
    settings:{theme:'dark',language:'ko',launch_at_login:true,channel_separation_enabled:true,link_group_outputs:true,groups:channels.map(channel=>({id:channel,display_name:channel,sonar_channel:channel}))},
    audio:{endpoints:[],warnings:[]},separation_available:true,
    sonar:{connection:'connected',message:null,mode:'classic',volumes:[{channel:'microphone',volume:.63,muted:false}],sources:[],applications:[{process_id:1,process_name:'Player',display_name:'Player',route:'aux',activity:'active',routing_error:false}],
      routes:[...channels,'microphone'].map(channel=>({channel,device_id:channel==='microphone'?'mic':'first',device_name:channel})),
      devices:[{id:'second',name:'Headphones',role:'playback',enabled:true},{id:'mic',name:'Microphone',role:'capture',enabled:true}]},
    app_routing:[{process_id:1,stable_id:'player',requested:'aux',origin:'user',pending:false}],
  };
}
const route=(state,channel='game',device='second',expected='first')=>applyPreviewAction(state,{kind:'sonar',command:{operation:'route',channel,device_id:device,expected}},fixture);

test('preview linked outputs change together while microphone and original state stay intact',()=>{
  const state=fixture();const before=structuredClone(state);const next=route(state);
  assert.deepEqual(next.sonar.routes.map(route=>route.device_id),['second','second','second','mic']);
  assert.deepEqual(state,before);
});

test('enabling output sync adopts Gaming and restores saved volume for each group including zero',()=>{
  const state=fixture();state.settings.link_group_outputs=false;
  state.settings.groups.push({id:'aux',display_name:'aux',sonar_channel:'aux'});
  state.sonar.routes.push({channel:'aux',device_id:'second',device_name:'Headphones'});
  state.sonar.devices.push({id:'first',name:'Speakers',role:'playback',enabled:true});
  for(const [index,group] of state.settings.groups.entries()) {
    group.device_volumes={first:[.1,0,.28,.84][index]};
    state.sonar.volumes.push({channel:group.sonar_channel,volume:.63,muted:group.sonar_channel==='aux'});
    if(group.sonar_channel!=='game')state.sonar.routes.find(route=>route.channel===group.sonar_channel).device_id='second';
  }
  const before=structuredClone(state);
  const next=applyPreviewAction(state,{kind:'output_sync',enabled:true},fixture);
  assert.equal(next.settings.link_group_outputs,true);
  assert.deepEqual(next.sonar.routes.map(route=>route.device_id),['first','first','first','mic','first']);
  assert.deepEqual(next.sonar.volumes.map(volume=>volume.volume),[.63,.63,0,.28,.84]);
  assert.equal(next.sonar.volumes.at(-1).muted,true);
  for(const group of next.settings.groups.slice(1))assert.equal(group.device_volumes.second,.63);
  assert.deepEqual(state,before);
  const off=applyPreviewAction(next,{kind:'output_sync',enabled:false},fixture);
  assert.deepEqual(off.sonar,next.sonar);
});

test('enabling sync preserves current volume for same or unremembered devices and rejects unavailable Gaming',()=>{
  const state=fixture();state.settings.link_group_outputs=false;
  state.sonar.devices.push({id:'first',name:'Speakers',role:'playback',enabled:true});
  state.sonar.volumes.push(...['game','chat','media'].map(channel=>({channel,volume:.4,muted:false})));
  state.settings.groups[1].device_volumes={first:0};
  state.sonar.routes[2].device_id='second';
  const next=applyPreviewAction(state,{kind:'output_sync',enabled:true},fixture);
  assert.deepEqual(next.sonar.volumes.map(volume=>volume.volume),[.63,.4,.4,.4]);
  state.separation_available=false;
  assert.throws(()=>applyPreviewAction(state,{kind:'output_sync',enabled:true},fixture),/not ready/);
  state.separation_available=true;state.sonar.routes=state.sonar.routes.filter(route=>route.channel!=='game');
  assert.throws(()=>applyPreviewAction(state,{kind:'output_sync',enabled:true},fixture),/Gaming/);
});

test('preview microphone volume memory is independent, preserves zero, and leaves input state intact',()=>{
  const original=fixture();original.sonar.devices.push({id:'other-mic',name:'Other microphone',role:'capture',enabled:true});
  const before=structuredClone(original);
  let state=route(original,'microphone','other-mic','mic');
  assert.deepEqual(original,before);
  assert.deepEqual(state.settings.microphone_device_volumes,{mic:.63,'other-mic':.63});
  state.sonar.volumes[0].volume=0;
  state=route(state,'microphone','mic','other-mic');
  assert.equal(state.sonar.volumes[0].volume,.63);
  state=route(state,'microphone','other-mic','mic');
  assert.equal(state.sonar.volumes[0].volume,0);
  assert.deepEqual(state.sonar.routes.slice(0,3).map(value=>value.device_id),['first','first','first']);
});
test('preview unlinked outputs and microphone selection stay independent; stale and mismatched devices are rejected',()=>{
  const state=applyPreviewAction(fixture(),{kind:'output_sync',enabled:false},fixture);
  assert.deepEqual(route(state).sonar.routes.map(route=>route.device_id),['second','first','first','mic']);
  assert.deepEqual(route(state,'microphone','mic','mic').sonar.routes.map(route=>route.device_id),['first','first','first','mic']);
  assert.throws(()=>route(state,'game','mic'),/role/);
  assert.throws(()=>route(state,'game','second','stale'),/stale/);
});
test('preview aux group is bounded and deleting it keeps app assignments consistent',()=>{
  let state=applyPreviewAction(fixture(),{kind:'add_group'},fixture);
  state=applyPreviewAction(state,{kind:'add_group'},fixture);
  assert.equal(state.settings.groups.length,4);
  state=applyPreviewAction(state,{kind:'remove_group',group_id:state.settings.groups.at(-1).id},fixture);
  assert.equal(state.settings.groups.length,3);
  assert.equal(state.sonar.applications[0].route,'game');
  assert.equal(state.app_routing[0].requested,'game');
});
test('volume number preference survives other preferences and leaves audio intact',()=>{
  const original=fixture();
  let state=applyPreviewAction(original,{kind:'tray_volume_numbers',enabled:true},fixture);
  state=applyPreviewAction(state,{kind:'preferences',theme:'light',language:'en',launch_at_login:false},fixture);
  assert.equal(state.settings.tray_volume_numbers_always_visible,true);
  assert.deepEqual(state.sonar,original.sonar);
  assert.equal(original.settings.tray_volume_numbers_always_visible,undefined);
  state=applyPreviewAction(state,{kind:'tray_volume_numbers',enabled:false},fixture);
  assert.equal(state.settings.tray_volume_numbers_always_visible,false);
});
test('preview preferences and shortcut are local and reset restores the fixture',()=>{
  let state=applyPreviewAction(fixture(),{kind:'preferences',theme:'light',language:'en',launch_at_login:false},fixture);
  state=applyPreviewAction(state,{kind:'tray_shortcut',enabled:true,key:'KeyK',modifiers:6},fixture);
  assert.equal(state.settings.theme,'light');assert.equal(state.settings.language,'en');assert.equal(state.settings.launch_at_login,false);
  assert.deepEqual(state.settings.tray_shortcut,{enabled:true,key:'KeyK',modifiers:6});
  assert.deepEqual(applyPreviewAction(state,{kind:'reset'},fixture),fixture());
  assert.throws(()=>applyPreviewAction(state,{kind:'unknown'},fixture),/Unsupported/);
});
