import type { Channel, Theme, ViewState } from './main';

// In-memory UI sandbox. This module has no native, storage or network access.
export function applyPreviewAction(state:ViewState, action:Record<string,unknown>, reset:()=>ViewState):ViewState {
  const next=structuredClone(state);
  switch(action.kind) {
    case 'preferences':
      next.settings.theme=action.theme as Theme;
      next.settings.language=String(action.language);
      next.settings.launch_at_login=Boolean(action.launch_at_login);
      break;
    case 'output_sync':
      next.settings.link_group_outputs=Boolean(action.enabled);
      break;
    case 'tray_volume_numbers':
      next.settings.tray_volume_numbers_always_visible=Boolean(action.enabled);
      break;
    case 'tray_shortcut':
      next.settings.tray_shortcut={enabled:Boolean(action.enabled),key:typeof action.key==='string'?action.key:null,modifiers:Number(action.modifiers)};
      break;
    case 'add_group': {
      const groups=next.settings.groups ?? [];
      if(groups.length>=4 || groups.some(group=>group.sonar_channel==='aux'))break;
      groups.push({id:'00000000-0000-0000-0000-000000000004',display_name:next.settings.language==='en'?'Aux':'보조',sonar_channel:'aux'});
      next.settings.groups=groups;
      break;
    }
    case 'remove_group': {
      const group=next.settings.groups?.find(group=>group.id===action.group_id);
      if(group?.sonar_channel!=='aux')break;
      next.settings.groups=next.settings.groups!.filter(group=>group.id!==action.group_id);
      for(const session of next.sonar.applications)if(session.route==='aux')session.route='game';
      for(const identity of next.app_routing ?? [])if(identity.requested==='aux')identity.requested='game';
      break;
    }
    case 'sonar': {
      const command=action.command as {operation:string;channel:Channel;device_id:string;expected:string};
      if(command.operation!=='route')throw new Error('Unsupported preview command');
      const device=next.sonar.devices.find(device=>device.id===command.device_id && device.enabled);
      const route=next.sonar.routes.find(route=>route.channel===command.channel);
      if(!device || !route || route.device_id!==command.expected)throw new Error('Preview device selection is stale');
      const microphone=command.channel==='microphone';
      if(device.role!==(microphone?'capture':'playback'))throw new Error('Preview device role does not match');
      if(microphone) {
        const volume=next.sonar.volumes.find(value=>value.channel==='microphone');
        if(!volume)throw new Error('Preview microphone volume is not ready');
        const saved=next.settings.microphone_device_volumes??={};
        for(const input of next.sonar.devices.filter(input=>input.role==='capture' && input.enabled)) {
          saved[input.id]??=volume.volume;
        }
        if(route.device_id)saved[route.device_id]=volume.volume;
        volume.volume=saved[device.id];
      }
      const activeChannels=new Set(next.settings.groups?.map(group=>group.sonar_channel) ?? ['game','chat','media']);
      for(const target of next.sonar.routes) {
        if(target.channel===command.channel || (!microphone && next.settings.link_group_outputs && activeChannels.has(target.channel))) {
          target.device_id=device.id;target.device_name=device.name;
        }
      }
      break;
    }
    case 'sonar_retry':
      next.sonar.connection='connected';next.sonar.mode='classic';next.separation_available=true;
      break;
    case 'reset':
      return reset();
    default:
      throw new Error(`Unsupported preview action: ${String(action.kind)}`);
  }
  return next;
}
