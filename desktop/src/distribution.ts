import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getVersion } from '@tauri-apps/api/app';
import './distribution.css';

interface Installation { installed:boolean; version:string|null; running:boolean }
interface Latest { version:string }
interface JackUpdate { current:string; version:string|null; notes:string|null }
interface Sonar {
  connection:string; mode:string|null;
  sources:{channel:string;endpoint_id:string}[];
  devices:{id:string;role:string;enabled:boolean}[];
  routes:{channel:string;device_id:string}[];
}
let installation:Installation|null=null;
let latest:Latest|null=null;
let update:JackUpdate|null=null;
let version='';
let loaded=false;
let working=false;
let notice='';
let failure='';
let downloaded=0;
let redraw:()=>void=()=>{};
export const distributionBusy=()=>working;

function node<K extends keyof HTMLElementTagNameMap>(tag:K,cls:string,text='') {
  const result=document.createElement(tag);result.className=cls;result.textContent=text;return result;
}
function action(text:string,run:()=>void,disabled=false) {
  const button=node('button','operation',text);button.type='button';button.disabled=disabled||working;
  button.addEventListener('click',run);return button;
}
async function perform(task:()=>Promise<void>) {
  if(working)return;
  working=true;failure='';redraw();
  try {await task();} catch(reason) {failure=String(reason);notice='';}
  finally {working=false;redraw();}
}
async function inspect() {installation=await invoke<Installation>('gg_action',{operation:'inspect',consent:false});}
function compare(a:string,b:string) {
  const left=a.split('.').map(Number),right=b.split('.').map(Number);
  for(let i=0;i<3;i++) {const difference=left[i]-right[i];if(difference)return difference;}
  return 0;
}
function consentDialog(english:boolean,title:string,body:string,accept:string,run:()=>void) {
  const dialog=node('dialog','confirm-dialog is-open');dialog.setAttribute('aria-label',title);
  const header=node('div','dialog-header');header.append(node('h2','dialog-title',title));
  const viewport=node('div','confirm-scroll');viewport.append(node('p','dialog-text',body));
  const buttons=node('div','dialog-actions');
  const cancel=node('button','dialog-button',english?'Cancel':'취소');cancel.type='button';
  const confirm=node('button','dialog-button',accept);confirm.type='button';
  cancel.onclick=()=>dialog.close();confirm.onclick=()=>{dialog.close();run();};
  buttons.append(cancel,confirm);dialog.append(header,viewport,buttons);
  dialog.addEventListener('close',()=>dialog.remove(),{once:true});
  document.body.append(dialog);dialog.showModal();cancel.focus();
}
function setupStatus(sonar:Sonar,english:boolean) {
  const text=(ko:string,en:string)=>english?en:ko;
  if(!installation)return text('GG 설치 상태 확인 필요','Check GG installation');
  if(!installation.installed)return text('SteelSeries GG 설치 필요','SteelSeries GG is required');
  if(!installation.running)return text('GG를 실행해 주세요','Open GG to continue');
  if(sonar.connection==='unsupported_mode')return text('GG에서 Sonar Classic 모드로 전환해 주세요','Switch to Sonar Classic in GG');
  if(sonar.connection==='disabled')return text('GG에서 Sonar를 켜 주세요','Enable Sonar in GG');
  if(sonar.connection==='incompatible')return text('이 GG 응답 형식은 지원되지 않습니다','This GG response is not supported');
  if(sonar.connection!=='connected'||sonar.mode!=='classic')return text('Sonar 연결을 기다리고 있습니다','Waiting for Sonar');
  if(!['game','chat'].every(channel=>sonar.sources.some(source=>source.channel===channel))) {
    return text('Sonar 가상 오디오 장치를 준비해 주세요. 필요하면 Windows를 다시 시작하세요.','Sonar virtual devices are not ready. Restart Windows if requested.');
  }
  const playback=sonar.routes.filter(route=>sonar.sources.some(source=>source.channel===route.channel));
  const outputReady=playback.length>0&&playback.every(route=>sonar.devices.some(device=>device.id===route.device_id&&device.enabled&&device.role==='playback'));
  const input=sonar.routes.find(route=>route.channel==='microphone');
  const inputReady=input&&sonar.devices.some(device=>device.id===input.device_id&&device.enabled&&device.role==='capture');
  if(!outputReady||!inputReady)return text('GG에서 실제 출력 장치와 마이크를 선택해 주세요','Select physical output devices and a microphone in GG');
  return text('Sonar Classic 연결됨 · 실제 소리·녹음은 별도 확인이 필요합니다','Sonar Classic connected · Check playback and recording separately');
}

export function distributionSettings(english:boolean,native:boolean,sonar:Sonar,repaint:()=>void,retry:()=>Promise<void>) {
  redraw=repaint;
  const text=(ko:string,en:string)=>english?en:ko;
  const section=node('section','distribution-settings');
  section.append(node('h2','section-label','SteelSeries GG / Sonar'));
  const card=node('div','card operations');
  const info=node('div','distribution-info');
  info.append(node('p','item-title',setupStatus(sonar,english)));
  let versions=installation?.version?`GG ${installation.version}`:text('설치 버전 미확인','Installed version unknown');
  if(latest) {
    versions+=` · ${text('최신','Latest')} ${latest.version}`;
    if(installation?.version) {
      const difference=compare(installation.version,latest.version);
      versions+=` · ${difference===0?text('최신 버전','Up to date'):difference>0?text('설치 버전이 더 높습니다','Installed version is newer'):text('업데이트 가능','Update available')}`;
    }
  } else versions+=` · ${text('최신 버전 미확인','Latest version not checked')}`;
  info.append(node('p','item-hint',versions));card.append(info);
  const installed=installation?.installed===true;
  card.append(action(text('상태·최신 버전 확인','Check status and latest version'),()=>void perform(async()=>{
    notice=text('GG 버전을 확인하고 있습니다','Checking GG version');
    await inspect();latest=null;
    latest=await invoke<Latest>('gg_action',{operation:'latest',consent:false});notice='';
  }),!native));
  card.append(action(installed?text('GG 열기 · Sonar 설정','Open GG · Set up Sonar'):text('GG 설치','Install GG'),()=>{
    if(installed) {void perform(async()=>{await invoke('gg_action',{operation:'open',consent:false});await inspect();});return;}
    askInstall();
  },!native||!installation));
  function askInstall() {
    consentDialog(english,text('SteelSeries GG 설치·업데이트','Install or update SteelSeries GG'),
      text('Jack에는 GG와 Sonar가 필요합니다. 공식 최신 설치 파일을 다운로드하고 게시자 서명을 확인한 뒤 실행합니다. GG 약관 동의와 관리자 승인이 필요할 수 있습니다. 기존 설정은 Jack이 초기화하지 않습니다.',
        'Jack requires GG and Sonar. The latest official installer will be downloaded, its publisher signature checked, then opened. GG terms and administrator approval may be required. Jack will not reset your existing settings.'),
      text('동의하고 계속','Agree and continue'),()=>void perform(async()=>{
        notice=text('GG 다운로드·설치 중입니다. 설치 창의 안내를 따라 주세요.','Downloading and installing GG. Follow its installer.');redraw();
        const result=await invoke<{reboot_required:boolean}>('gg_action',{operation:'install',consent:true});
        await inspect();
        notice=result.reboot_required?text('Windows를 다시 시작한 뒤 Sonar 설정을 이어가세요','Restart Windows, then continue Sonar setup'):text('GG에서 Sonar를 켜고 실제 장치를 선택해 주세요','Enable Sonar and select physical devices in GG');
        await retry();
      }));
  }
  if(installed&&latest&&(!installation?.version||compare(installation.version,latest.version)<0)) {
    card.append(action(text('최신 GG로 업데이트','Update to latest GG'),askInstall,!native));
  }
  card.append(action(text('Sonar 다시 연결','Reconnect Sonar'),()=>void perform(async()=>{await inspect();await retry();}),!native));
  section.append(card);

  const jack=node('section','distribution-settings');jack.append(node('h2','section-label',text('Jack 업데이트','Jack updates')));
  const updateCard=node('div','card operations');const updateInfo=node('div','distribution-info');
  updateInfo.append(node('p','item-title',`Jack ${update?.current??version}`),node('p','item-hint',
    update?(update.version?`${text('새 버전','New version')} ${update.version}`:text('최신 버전입니다','Up to date')):text('업데이트를 확인해 주세요','Check for updates')));
  updateCard.append(updateInfo,action(text('업데이트 확인','Check for updates'),()=>void perform(async()=>{
    update=await invoke<JackUpdate>('check_jack_update');notice='';
  }),!native));
  if(update?.version)updateCard.append(action(text('업데이트하고 다시 시작','Update and restart'),()=>{
    consentDialog(english,text('Jack 업데이트','Update Jack'),text('업데이트를 다운로드하고 서명을 확인합니다. 적용 전에 Jack이 변경한 앱 오디오 경로를 복원하고 다시 시작합니다. GG는 업데이트하지 않습니다.',
      'Download and verify the Jack update, restore app audio routes changed by Jack, then restart. GG will not be updated.'),text('업데이트','Update'),()=>void perform(async()=>{
        downloaded=0;notice=text('Jack 업데이트 다운로드 중','Downloading Jack update');redraw();
        await invoke('install_jack_update');
      }));
  },!native));
  jack.append(updateCard);section.append(jack);
  if(working||notice||failure) {
    const status=node('p','distribution-notice item-hint',failure||notice||text('확인 중입니다','Checking'));
    status.role='status';status.setAttribute('aria-live','polite');section.append(status);
  }
  if(native&&!loaded) {
    loaded=true;
    queueMicrotask(()=>void perform(async()=>{version=await getVersion();await inspect();}));
    void listen<{chunk:number;total:number|null}>('jack-update-progress',event=>{
      downloaded+=event.payload.chunk;
      notice=`${text('Jack 다운로드','Downloading Jack')} · ${event.payload.total?`${Math.min(100,Math.round(downloaded/event.payload.total*100))}%`:`${Math.round(downloaded/1024/1024)} MB`}`;
      redraw();
    });
  }
  return section;
}
