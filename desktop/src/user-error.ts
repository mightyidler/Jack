export type ErrorCode='connection'|'classic_required'|'external_change'|'device_unavailable'|'settings_save'|'recovery'|'startup'|'shortcut'|'busy'|'session_changed'|'verification'|'internal';
const copy:Record<ErrorCode,[string,string]>={
  connection:['Sonar에 연결할 수 없습니다. GG에서 Sonar가 켜져 있는지 확인한 뒤 다시 시도해 주세요.','Could not connect to Sonar. Check that Sonar is enabled in GG, then retry.'],
  classic_required:['Classic 모드가 필요합니다. GG에서 Sonar를 Classic 모드로 바꿔 주세요.','Classic mode is required. Switch Sonar to Classic mode in GG.'],
  external_change:['다른 곳에서 오디오 설정이 변경됐습니다. 갱신된 값을 확인한 뒤 다시 조작해 주세요.','Audio settings changed elsewhere. Check the refreshed values, then try again.'],
  device_unavailable:['선택한 장치의 연결이 끊겼습니다. 장치를 다시 연결하거나 다른 장치를 선택해 주세요.','The selected device disconnected. Reconnect it or choose another device.'],
  settings_save:['설정을 저장하지 못했습니다. 저장 공간과 사용자 설정 폴더의 쓰기 권한을 확인해 주세요.','Could not save settings. Check free disk space and write access to your settings folder.'],
  recovery:['오디오 설정 복구가 끝나지 않았습니다. GG와 장치 연결을 확인한 뒤 다시 시도해 주세요.','Audio settings could not be fully restored. Check GG and your devices, then retry.'],
  startup:['Windows 시작 실행 설정을 변경하지 못했습니다. 시작 프로그램 권한을 확인한 뒤 다시 시도해 주세요.','Could not change Windows startup settings. Check startup permissions, then retry.'],
  shortcut:['단축키를 등록하지 못했습니다. 다른 앱과 겹치지 않는 키 조합을 선택해 주세요.','Could not register the shortcut. Choose a key combination that is not used by another app.'],
  busy:['다른 작업이 진행 중입니다. 완료된 뒤 다시 시도해 주세요.','Another operation is in progress. Wait for it to finish, then retry.'],
  session_changed:['앱 또는 그룹 상태가 변경됐습니다. 현재 목록을 확인한 뒤 다시 시도해 주세요.','The app or group changed. Check the current list, then retry.'],
  verification:['변경 결과를 확인하지 못했습니다. GG의 현재 설정을 확인한 뒤 다시 시도해 주세요.','Could not confirm the change. Check the current settings in GG, then retry.'],
  internal:['작업을 완료하지 못했습니다. 다시 시도해 주세요. 문제가 계속되면 Jack을 다시 실행해 주세요.','Could not complete the operation. Retry, or restart Jack if the problem continues.'],
};

// Older event producers still send strings. IPC commands use stable codes.
export function errorCode(error:unknown):ErrorCode {
  if(error && typeof error==='object' && 'code' in error && typeof error.code==='string' && Object.hasOwn(copy,error.code))return error.code as ErrorCode;
  const text=(error instanceof Error?error.message:String(error)).toLowerCase();
  if(/recovery|restore audio routes/.test(text))return 'recovery';
  if(/settings save failed/.test(text))return 'settings_save';
  if(/startup/.test(text))return 'startup';
  if(/shortcut|hotkey/.test(text))return 'shortcut';
  if(/externally|changed before|selection is stale/.test(text))return 'external_change';
  if(/classic|streamer/.test(text))return 'classic_required';
  if(/device/.test(text)&&/disconnected|disappeared|not found/.test(text))return 'device_unavailable';
  if(/session changed|process.*no longer|group no longer/.test(text))return 'session_changed';
  if(/preparing an update|already being applied|operation is running/.test(text))return 'busy';
  if(/readback|did not apply|could not be confirmed/.test(text))return 'verification';
  if(/sonar|steelseries|not ready|timed out|coreprops/.test(text))return 'connection';
  return 'internal';
}

export function userError(error:unknown,english=false):string {
  return copy[errorCode(error)][english?1:0];
}
