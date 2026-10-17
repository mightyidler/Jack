import type { ViewState } from './main';

// Remembered volumes are persistence data, not a reason to remount controls.
export function controlsKey(value:ViewState):string {
  return JSON.stringify([
    layoutKey(value),
    value.audio.endpoints.map(({id,name,flow,form_factor})=>[id,name,flow,form_factor]),
    value.separation_available,value.sonar.connection,value.sonar.mode,value.sonar.routes,value.sonar.devices,value.sonar.sources,
    value.sonar.volumes.map(volume=>volume.channel).sort(),
  ]);
}

export function applicationsKey(value:ViewState):string {
  return JSON.stringify([value.sonar.applications,value.app_routing]);
}

export function layoutKey(value:ViewState):string {
  const settings=value.settings;
  return JSON.stringify([settings.theme,settings.language,settings.launch_at_login,settings.link_group_outputs,
    settings.channel_separation_enabled,settings.tray_shortcut,settings.tray_volume_numbers_always_visible,
    settings.groups?.map(({id,display_name,sonar_channel,icon_asset_id})=>[id,display_name,sonar_channel,icon_asset_id]),
    value.separation_available,value.sonar.connection,value.sonar.mode,value.sonar.sources]);
}
