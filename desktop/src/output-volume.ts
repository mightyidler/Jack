import type { Channel, ViewState } from './main';

export function outputVolume(state:ViewState,channel:Exclude<Channel,'microphone'>) {
  const route=state.sonar.routes.find(route=>route.channel===channel);
  const volume=state.sonar.volumes.find(volume=>volume.channel===channel);
  const source=state.sonar.sources.some(source=>source.channel===channel);
  const connected=!!route && state.sonar.devices.some(device=>device.id===route.device_id && device.role==='playback' && device.enabled);
  const group=state.settings.groups?.find(group=>(group.sonar_channel??({'00000000-0000-0000-0000-000000000001':'game','00000000-0000-0000-0000-000000000002':'chat','00000000-0000-0000-0000-000000000003':'media'} as Record<string,string>)[group.id])===channel);
  const remembered=route?group?.device_volumes?.[route.device_id]:undefined;
  const available=state.separation_available && source && connected && !!volume;
  return {available,level:available?volume!.volume:remembered??volume?.volume??null,disconnected:!connected};
}
