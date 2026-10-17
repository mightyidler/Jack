// Bald web/index.html and app.js update row/toast; Jack row padding is 12px.
import { getVersion } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';
import { cssTimeMs } from './motion-time';
import { userError } from './user-error';
import './bald-updates.css';

let toastTimer:ReturnType<typeof setTimeout>|undefined;
let version='—';
let loadingVersion=false;
let updating=false;
let redraw:()=>void=()=>{};
let currentEnglish=()=>false;

export function showToast(message:string,duration?:number) {
  let toast=document.getElementById('toast');
  if(!toast) {
    toast=document.createElement('div');toast.id='toast';toast.className='toast';
    toast.role='status';toast.setAttribute('aria-live','polite');document.body.append(toast);
  }
  toast.textContent=message;toast.classList.add('is-visible');clearTimeout(toastTimer);
  const clock=duration??cssTimeMs(getComputedStyle(toast).getPropertyValue('--toast-visible-time'),3000);
  toastTimer=setTimeout(()=>toast.classList.remove('is-visible'),clock);
}

export function updateSection(english:boolean,native:boolean,render:()=>void,language:()=>boolean) {
  redraw=render;currentEnglish=language;
  const section=document.createElement('section');section.className='bald-updates';
  const heading=document.createElement('h2');heading.className='section-label';heading.textContent=english?'Updates':'업데이트';
  const card=document.createElement('div');card.className='card';
  const row=document.createElement('div');row.className='card-row';
  const label=document.createElement('div');label.className='row-label';
  const title=document.createElement('span');title.className='row-title';title.textContent=english?'Current version':'현재 버전';
  const value=document.createElement('span');value.className='row-desc';value.id='appVersionText';value.textContent=version;
  label.append(title,value);
  const button=document.createElement('button');button.type='button';button.className='btn btn--ghost';button.id='btnCheckUpdate';
  button.disabled=!native||updating;button.textContent=updating?(english?'Checking…':'확인 중…'):(english?'Check for updates':'업데이트 확인');
  button.onclick=()=>void checkForUpdates();
  row.append(label,button);card.append(row);section.append(heading,card);
  if(native&&!loadingVersion) {
    loadingVersion=true;
    queueMicrotask(()=>void getVersion().then(value=>{version=value;redraw();}).catch(()=>{}));
  }
  return section;
}

async function checkForUpdates() {
  if(updating)return;
  updating=true;redraw();
  try {
    const result=await invoke<{version:string|null}>('check_jack_update');
    if(!result.version) {showToast(currentEnglish()?"You're up to date.":'최신 버전입니다.');return;}
    showToast(currentEnglish()?'Installing update…':'업데이트 설치 중…');
    await invoke('install_jack_update');
    showToast(currentEnglish()?'Update installed.':'업데이트가 설치되었습니다.');
  } catch(error) {
    showToast(`${currentEnglish()?'Update failed':'업데이트 실패'}: ${userError(error,currentEnglish())}`);
  } finally {updating=false;redraw();}
}
