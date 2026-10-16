import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { listen } from '@tauri-apps/api/event';
import './style.css';
import './accordion.css';
import { partitionApplications, type ApplicationGroup } from './application-groups';
import { bindAppDrag } from './app-drag';
import { LiveVolume } from './live-volume';
import { singleFlight } from './single-flight';
import { deviceLabel } from './device-name';
import './app-drag.css';
import './tray.css';
import './scrollbars.css';
import { mountScrollbar } from './scrollbars';
import { SlidingTabs } from './tabs-motion';
import './floating-surfaces.css';
import './preferences-dropdown.css';
import { preferenceDropdown } from './preferences-dropdown';
import './quick-motion.css';
import { QuickSurface } from './quick-motion';
import { VolumeMotion } from './volume-motion';
import './volume-motion.css';
import { bindFluidControls, holdFluidVolume, updateFluidVolume } from './fluid-controls';
import './fluid-controls.css';
import { setShortcutLabel } from './shortcut-motion';
import { cssTimeMs } from './motion-time';
import './rendering.css';
import { showToast, updateSection } from './bald-updates';
import { captureDividerOpacity, animateDividerOpacity } from './divider-motion';
import './divider-motion.css';
import { captureDevicePaint, animateDeviceChange } from './device-transition';
import './device-transition.css';

type Flow = 'render' | 'capture';
export type Theme = 'system' | 'dark' | 'light';
interface Group { id:string; display_name:string; sonar_channel?:OutputGroup|null; icon_asset_id?:string|null }
interface TrayShortcut { enabled:boolean; key:string|null; modifiers:number }
interface Settings { theme: Theme; language: string; launch_at_login: boolean; channel_separation_enabled: boolean; link_group_outputs: boolean; groups?: Group[]; tray_shortcut?:TrayShortcut; tray_volume_numbers_always_visible?:boolean; microphone_device_volumes?:Record<string,number> }
interface Endpoint { id: string; name: string; flow: Flow; form_factor: number | null; defaults: string[]; volume: {level: number; muted: boolean} | null }
export type Channel = 'game' | 'chat' | 'media' | 'aux' | 'microphone';
type MixerChannel = Channel | 'master';
// Backend capabilities do not define Jack's visible product controls.
const outputGroups = ['game','chat','media'] as const;
type OutputGroup = 'game' | 'chat' | 'media' | 'aux';
interface SonarState {
  connection: string; message: string | null; mode: string | null;
  volumes: {channel: MixerChannel; volume: number; muted: boolean}[];
  routes: {channel: Channel; device_id: string; device_name: string | null}[];
  devices: {id: string; name: string; role: 'playback' | 'capture'; enabled: boolean}[];
  applications: {process_id: number; process_name: string; display_name: string; route: string; activity: string; routing_error: boolean}[];
  sources: {channel: Channel; endpoint_id: string}[];
}
interface AppRoute {process_id:number;stable_id:string|null;requested:Channel|null;origin:string;pending:boolean;message?:string|null;icon_data_url?:string|null}
export interface ViewState { settings: Settings; audio: {endpoints: Endpoint[]; warnings: string[]}; separation_available: boolean; sonar: SonarState; app_routing?:AppRoute[]; routing_warnings?:string[] }
const native = isTauri();
const params = new URLSearchParams(location.search);
const interactivePreview = !native && import.meta.env.DEV && params.has('interactive');
const controlsAvailable = native || interactivePreview;
const quick = params.get('view') === 'tray';
document.documentElement.classList.toggle('quick-view',quick);
if (quick) document.documentElement.dataset.popupOpen=String(!native);
document.documentElement.classList.add('fastlane');
if (import.meta.env.DEV && params.get('fixture') === 'design') document.documentElement.dataset.fixture = 'design';
const app = document.querySelector<HTMLDivElement>('#app')!;
app.addEventListener('selectstart',event => event.preventDefault());
let state: ViewState | null = null;
let tab = params.get('tab') === 'settings' ? 'settings' : params.get('tab') === 'capture' ? 'capture' : 'render';
let busy = false;
let dragging = false;
let windowDragging=false;
let shortcutRecording=false;
let shortcutCaptureReady=false;
let cancelDrag: (() => void) | null = null;
const volumeMotions=new Map<string,VolumeMotion>();
let previousVolumeMotions=new Map<string,ReturnType<VolumeMotion['snapshot']>>();
function retireVolumeMotions() {
  previousVolumeMotions=new Map([...volumeMotions].map(([key,motion])=>[key,motion.snapshot()]));
  volumeMotions.forEach(motion=>motion.dispose());
  volumeMotions.clear();
}
let error = '';
let visible = true;
let reported = false;
let revision = 0;
let inputExpanded = import.meta.env.DEV && params.has('expanded');
let quickOutputGroup: OutputGroup | null = null;
let dismissOutputMenu: (() => void) | null = null;
const slidingTabs = new SlidingTabs();
let disposeScrollbars: (() => void)[] = [];
let quickSurface: QuickSurface | null = null;
let quickPreparationKey='';
let cancelQuickPreparation:(()=>void)|null=null;
let desiredQuickHeight=0;
let appliedQuickHeight=0;
let quickSizeFlight: Promise<void> | null=null;
interface QuickLayout { anchor:number; origin:number }
function applyQuickLayout(layout:QuickLayout) {
  document.documentElement.style.setProperty('--quick-anchor-y',`${layout.anchor}px`);
  document.documentElement.style.setProperty('--quick-anchor-origin',String(layout.origin));
}
function resizeQuickTo(contentHeight: number): Promise<void> {
  if(!native || !quick)return Promise.resolve();
  desiredQuickHeight=Math.ceil(contentHeight+58); // 40px gutter + 18px panel padding/border
  if(!quickSizeFlight) {
    quickSizeFlight=(async()=>{
      while(appliedQuickHeight!==desiredQuickHeight) {
        const height=desiredQuickHeight;
        applyQuickLayout(await invoke<QuickLayout>('resize_quick',{height})); appliedQuickHeight=height;
      }
    })().catch(reason=>{error=String(reason);throw reason;}).finally(()=>{quickSizeFlight=null;});
  }
  return quickSizeFlight;
}
async function reportNative() {
  if (!native || reported || !state) return;
  reported = true;
  await document.fonts.ready;
  const imgs = [...document.querySelectorAll<HTMLImageElement>('img')];
  const icons = [...document.querySelectorAll<HTMLElement>('[data-icon-asset]')];
  const imageProof = await Promise.all([...imgs,...icons].map(async slot => {
    const source = slot instanceof HTMLImageElement ? slot.getAttribute('src')! : slot.dataset.iconAsset!;
    const image = slot instanceof HTMLImageElement ? slot : new Image();
    if (!(slot instanceof HTMLImageElement)) image.src = source;
    await image.decode().catch(() => {});
    const rect=slot.getBoundingClientRect(); const css=getComputedStyle(slot);
    return {src:source,loaded:image.complete&&image.naturalWidth>0,width:rect.width,height:rect.height,color:css.color};
  }));
  const typography=[...document.querySelectorAll<HTMLElement>('.title,.tagline,.item-title,.item-hint,.device-name,.sonar-percent,.tab')].map(slot => {
    const css=getComputedStyle(slot); return {className:slot.className,fontSize:css.fontSize,lineHeight:css.lineHeight,fontWeight:css.fontWeight,color:css.color};
  });
  await invoke('ui_ready',{report:{endpointCount:state.audio.endpoints.length,theme:document.documentElement.dataset.theme,fontReady:document.fonts.check('400 17px "Pretendard Variable"'),separationEnabled:state.settings.channel_separation_enabled,images:imageProof,typography}});
}
const messages = {
  ko: {output:'사운드 출력', input:'마이크 입력', settings:'설정', outputSync:'출력 장치 동기화', outputSyncHint:'모든 오디오 그룹의 출력 장치가 함께 변경됩니다',   outputs:'출력 장치', inputs:'입력 장치', startup:'Windows 시작 시 자동 실행', startupHint:'로그인할 때 Jack을 자동으로 실행합니다', environment:'환경', language:'표시 언어', theme:'테마', system:'시스템', dark:'다크', light:'라이트', reset:'설정 초기화', quit:'Jack 종료', resetTitle:'설정을 초기화할까요?', resetHint:'Jack의 설정과 그룹 배정을 초기화합니다. 현재 Windows 장치 선택은 유지됩니다.', cancel:'취소', confirm:'초기화', empty:'연결된 장치가 없습니다', loading:'장치를 확인하고 있습니다', retry:'다시 시도', preview:'디자인 미리보기 · 장치 제어는 앱에서 사용할 수 있습니다', minimize:'최소화', close:'트레이로 숨기기', volume:'볼륨', },
  en: {output:'Sound output', input:'Microphone', settings:'Settings', outputSync:'Sync group output devices', outputSyncHint:'Changing one group’s output device\napplies it to every group.',   outputs:'Output devices', inputs:'Input devices', startup:'Launch when Windows starts', startupHint:'Launch Jack automatically when you sign in', environment:'Environment', language:'Language', theme:'Theme', system:'System', dark:'Dark', light:'Light', reset:'Reset settings', quit:'Quit Jack', resetTitle:'Reset your settings?', resetHint:'Reset Jack settings and group assignments. Your current Windows device selections will be kept.', cancel:'Cancel', confirm:'Reset', empty:'No connected devices', loading:'Checking your audio devices', retry:'Retry', preview:'Design preview · Device controls are available in the app', minimize:'Minimize', close:'Hide to tray', volume:'Volume', },
};
function t() { return state?.settings.language === 'en' ? messages.en : messages.ko; }
function element<K extends keyof HTMLElementTagNameMap>(tag: K, className = '', text?: string): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag); el.className = className; if (text !== undefined) el.textContent = text; return el;
}
type IconSize = 16 | 20 | 24;
const compactIcons = new Set(['Chat','Chevron down','Desktop','Gaming','Headphones','Media','Mic','Puzzle','Refresh','Speaker']);
function icon(name: string, size: IconSize = 24) {
  const img = element('span','icon');
  const file = name === 'X' && size === 16 ? 'X-16px' :
    name === 'X' || name === 'Minus' || (size <= 20 && compactIcons.has(name)) ? `${name}-20px` : name;
  const asset = `/assets/icons/${file}.svg`;
  img.dataset.iconAsset = asset;
  img.style.width = img.style.height = `${size}px`;
  img.style.maskImage = `url("${encodeURI(asset)}")`;
  img.setAttribute('aria-hidden','true');
  return img;
}
function chevron(direction: 'left' | 'right' | 'down', size: IconSize) {
  const img = icon('Chevron down',size); img.classList.add(`chevron-${direction}`); return img;
}
function button(label: string, className: string, action: () => void) { const btn = element('button',className,label); btn.type = 'button'; btn.addEventListener('click',action); return btn; }
function sectionLabel(label: string) { return element('h2','section-label',label); }
function theme() {
  const selected = state?.settings.theme ?? 'system';
  const light = selected === 'light' || (selected === 'system' && matchMedia('(prefers-color-scheme: light)').matches);
  document.documentElement.dataset.theme = params.get('fixture') === 'design' && !params.has('theme') && import.meta.env.DEV ? 'dark' : light ? 'light' : 'dark';
  document.documentElement.lang = state?.settings.language ?? 'ko';
  document.documentElement.dataset.fastlaneTheme = document.documentElement.dataset.theme;
  document.documentElement.dataset.trayVolumeNumbers=state?.settings.tray_volume_numbers_always_visible?'always':'adjusting';
}
async function act(action: Record<string, unknown>) {
  if (!controlsAvailable || busy) return;
  revision++;
  const focusId = (document.activeElement as HTMLElement)?.id;
  busy = true; error = ''; updateDisabled();
  if(action.kind==='sonar_retry') {
    const retry=app.querySelector<HTMLButtonElement>('[data-sonar-retry]');
    if(retry) {
      retry.setAttribute('aria-busy','true');
      retry.querySelector('.retry-label')!.textContent=state?.settings.language==='en'?'Connecting…':'연결 중…';
    }
  }
  try {
    if (interactivePreview && state) {
      const { applyPreviewAction } = await import('./preview-actions');
      state = applyPreviewAction(state,action,fixture);
      if((action.kind==='tray_volume_numbers' || action.kind==='reset') && parent!==window)parent.postMessage({type:'jack-preview-volume-numbers',enabled:!!state.settings.tray_volume_numbers_always_visible},location.origin);
    } else state = await invoke<ViewState>('audio_action',{action});
  }
  catch (e) { error = String(e); showToast(error); if (native) try { state = await invoke<ViewState>('audio_action',{action:{kind:'read'}}); } catch { /* preserve the original error */ } }
  finally { busy = false; render(); if (focusId) document.getElementById(focusId)?.focus(); }
}
function updateDisabled() { app.querySelectorAll<HTMLButtonElement | HTMLSelectElement>('[data-control]').forEach(el => {
  if (el.id === 'group-add') {
    // Busy actions are already rejected by act(); keep the button's paint stable.
    el.setAttribute('aria-disabled',String(el.disabled || busy || !controlsAvailable));
  } else el.disabled = busy || !controlsAvailable;
}); }
function control<T extends HTMLButtonElement | HTMLSelectElement>(el: T) { el.dataset.control = ''; el.disabled = busy || !controlsAvailable; return el; }
function retryButton(action:()=>void,sonar=false) {
  const btn=control(button('','retry',action));
  btn.append(icon('Refresh',20),element('span','retry-label',t().retry));
  if(sonar) {
    btn.dataset.sonarRetry='';btn.id=quick?'quick-sonar-retry':'sonar-retry';
    btn.setAttribute('aria-busy','false');
  }
  return btn;
}
function preferences(patch: Partial<Settings>) { if (state) { const next = {...state.settings,...patch}; void act({kind:'preferences',theme:next.theme,language:next.language,launch_at_login:next.launch_at_login}); } }
function switchButton(label: string, enabled: boolean, action: () => void) {
  const btn = button('','switch',() => {
    btn.setAttribute('aria-checked',String(btn.getAttribute('aria-checked') !== 'true'));
    action();
  });
  btn.role='switch'; btn.setAttribute('aria-label',label); btn.setAttribute('aria-checked',String(enabled));
  const track=element('span','switch-track'); track.setAttribute('aria-hidden','true');
  const knob=element('span','knob'); knob.setAttribute('aria-hidden','true');
  btn.append(track,knob); return btn;
}
function render() {
  if (dragging || windowDragging || shortcutRecording) return;
  if (quick) { renderQuick(); return; }
  dismissOutputMenu?.();
  const focused = (document.activeElement as HTMLElement)?.id;
  const oldFrame=app.querySelector<HTMLElement>('.window');
  const oldPanel = document.getElementById('panel');
  const dividerOpacity=captureDividerOpacity(oldPanel);
  const devicePaint=captureDevicePaint(oldPanel?.classList.contains(tab)?oldPanel:null);
  retireVolumeMotions();
  const scrollTop = oldPanel?.classList.contains(tab) ? oldPanel.scrollTop : 0;
  disposeScrollbars.forEach(dispose => dispose()); disposeScrollbars=[];
  theme(); const m = t(); let frame:HTMLElement = element('div','window');
  const edge = element('div','scroll-edge'); frame.append(edge);
  const header = element('header','header'); const top = element('div','header-top');
  const brand = element('div','brand'); const logo = element('img','logo'); logo.src = '/assets/figma/logo.png'; logo.alt = ''; logo.draggable = false;
  const titles = element('div','titles'); titles.append(element('h1','title','Jack'),element('p','tagline','Just Audio Control Knob')); brand.append(logo,titles);
  const windowActions = element('div','window-actions');
  const minimize = button('','window-button',() => { if (native) void invoke('minimize_window'); }); minimize.setAttribute('aria-label',m.minimize); minimize.append(icon('Minus',20));
  const close = button('','window-button',() => { if (native) void invoke('hide_to_tray'); }); close.setAttribute('aria-label',m.close); close.append(icon('X',20)); windowActions.append(minimize,close); top.append(brand,windowActions);
  const tabs = element('nav','tabs'); tabs.role = 'tablist';
  const pill = element('span','tab-indicator'); pill.setAttribute('aria-hidden','true'); tabs.append(pill);
  const tabIds = ['render','capture','settings'];
  [m.output,m.input,m.settings].forEach((label,i) => {
    const btn = button(label,'tab',() => { tab = tabIds[i]; render(); document.getElementById(`tab-${tab}`)?.focus(); }); btn.id = `tab-${tabIds[i]}`; btn.role = 'tab'; btn.tabIndex = tab === tabIds[i] ? 0 : -1; btn.setAttribute('aria-selected',String(tab === tabIds[i])); btn.setAttribute('aria-controls','panel');
    btn.addEventListener('keydown',e => { if (['ArrowLeft','ArrowRight','Home','End'].includes(e.key)) { e.preventDefault(); tab = tabIds[e.key === 'Home' ? 0 : e.key === 'End' ? 2 : (i + (e.key === 'ArrowRight' ? 1 : 2)) % 3]; render(); document.getElementById(`tab-${tab}`)?.focus(); } }); tabs.append(btn);
  }); header.append(top,tabs); frame.append(header);
  let content = element('main',`content ${tab}`); content.id = 'panel'; content.role = 'tabpanel'; content.setAttribute('aria-labelledby',`tab-${tab}`);
  if (!state) { content.append(element('p','empty',error || m.loading),retryButton(() => void refresh())); }
  else if (tab === 'settings') renderSettings(content);
  else {
    if (tab === 'render') {
      const card = element('div','card separation'); const description = element('div','description'); description.append(element('p','item-title',m.outputSync),element('p','item-hint',m.outputSyncHint));
      const toggle = control(switchButton(m.outputSync,state.settings.link_group_outputs,() => void act({kind:'output_sync',enabled:!state!.settings.link_group_outputs}))); toggle.id = 'output-sync-toggle'; card.append(description,toggle); content.append(card);
    }
    if (!state.separation_available) content.append(sonarConnection());
    else if (tab === 'capture') content.append(deviceList('capture'));
    else content.append(sonarChannels());
  }
  frame.append(content);
  if (!native && params.get('fixture') !== 'design') frame.append(element('p','preview-label',m.preview));
  const previous=oldPanel?.classList.contains(tab) ? oldPanel.querySelector<HTMLButtonElement>('.switch[id]') : null;
  const next=previous ? content.querySelector<HTMLButtonElement>(`#${previous.id}`) : null;
  if(oldFrame && oldPanel && previous && next) {
    const directChild=(node:HTMLElement,parent:HTMLElement):HTMLElement=>{
      let child=node; while(child.parentElement!==parent) child=child.parentElement!; return child;
    };
    // Preserve each switch's section independently: General and Shortcuts now
    // have separate cards, and neither its knob nor disclosure may be remounted.
    const preserved=new Map<Element,HTMLElement>();
    for(const oldToggle of oldPanel.querySelectorAll<HTMLButtonElement>('.switch[id]')) {
      const newToggle=content.querySelector<HTMLButtonElement>(`#${oldToggle.id}`);
      if(!newToggle)continue;
      const kept=directChild(oldToggle,oldPanel),replacement=directChild(newToggle,content);
      preserved.set(replacement,kept);
      const oldLabel=kept.querySelector('.section-label'),newLabel=replacement.querySelector('.section-label');
      if(oldLabel&&newLabel)oldLabel.textContent=newLabel.textContent;
      const oldDescription=oldToggle.closest('.startup,.separation')?.querySelector('.description');
      const newDescription=newToggle.closest('.startup,.separation')?.querySelector('.description');
      if(oldDescription&&newDescription)oldDescription.replaceWith(newDescription);
      oldToggle.disabled=newToggle.disabled;
      oldToggle.setAttribute('aria-label',newToggle.getAttribute('aria-label')!);
      oldToggle.setAttribute('aria-checked',newToggle.getAttribute('aria-checked')!);
      const oldCard=oldToggle.closest<HTMLElement>('.shortcut-settings');
      const newCard=newToggle.closest<HTMLElement>('.shortcut-settings');
      if(!oldCard||!newCard)continue;
      oldCard.dataset.enabled=newCard.dataset.enabled;
      const oldShortcutLabel=oldCard.querySelector('.shortcut-label'),newShortcutLabel=newCard.querySelector('.shortcut-label');
      if(oldShortcutLabel&&newShortcutLabel)oldShortcutLabel.textContent=newShortcutLabel.textContent;
      const oldInner=oldCard.querySelector<HTMLElement>('.shortcut-editor-inner');
      const newInner=newCard.querySelector<HTMLElement>('.shortcut-editor-inner');
      if(oldInner && newInner) {
        const oldCapture=oldInner.querySelector<HTMLButtonElement>('.shortcut-capture');
        const newCapture=newInner.querySelector<HTMLButtonElement>('.shortcut-capture');
        if(oldCapture && newCapture) {
          oldCapture.disabled=newCapture.disabled;
          oldCapture.setAttribute('aria-label',newCapture.getAttribute('aria-label')!);
          setShortcutLabel(oldCapture,newCapture.textContent??'',false);
        } else oldInner.replaceChildren(...newInner.childNodes);
      }
      const editor=oldCard.querySelector<HTMLElement>('.shortcut-editor');
      if(editor){editor.inert=newCard.dataset.enabled!=='true';editor.setAttribute('aria-hidden',String(editor.inert));}
    }
    const children=[...content.children].map(child=>preserved.get(child)??child);
    [...oldPanel.children].filter(child=>!children.includes(child)).forEach(child=>child.remove());
    children.forEach((child,index)=>{if(oldPanel.children[index]!==child)oldPanel.insertBefore(child,oldPanel.children[index]??null);});
    oldFrame.querySelector('.header')!.replaceWith(header);
    frame=oldFrame; content=oldPanel;
  } else app.replaceChildren(frame);
  slidingTabs.mount(tabs,pill);
  disposeScrollbars.push(mountScrollbar(content,frame,'main'));
  content.scrollTop = scrollTop;
  animateDividerOpacity(content,dividerOpacity);
  animateDeviceChange(content,devicePaint);
  if (focused) document.getElementById(focused)?.focus({preventScroll:true});
}
function renderSettings(content: HTMLElement) {
  const m = t(); const s = state!.settings;
  const english=s.language==='en';
  const startup = element('section'); startup.append(sectionLabel(english?'General':'일반')); const card = element('div','card startup-settings');
  const startupRow=element('div','startup');
  const description = element('div','description'); description.append(element('p','item-title',m.startup),element('p','item-hint',m.startupHint));
  const toggle = control(switchButton(m.startup,s.launch_at_login,() => preferences({launch_at_login:!state!.settings.launch_at_login}))); toggle.id = 'startup-toggle'; startupRow.append(description,toggle);card.append(startupRow); startup.append(card);
  const shortcuts=element('section');shortcuts.append(sectionLabel(english?'Shortcuts':'단축키'));
  const shortcut=s.tray_shortcut??{enabled:false,key:null,modifiers:0};
  const shortcutCard=element('div','card shortcut-settings');shortcutCard.dataset.enabled=String(shortcut.enabled);
  const shortcutRow=element('div','startup');const shortcutDescription=element('div','description');
  const shortcutTitle=english?'Quick volume control':'빠른 볼륨 조절';
  shortcutDescription.append(element('p','item-title',shortcutTitle),element('p','item-hint',english?'Press a shortcut to adjust audio channel volumes.':'단축키를 눌러 오디오 채널의 볼륨을 조절합니다'));
  const shortcutToggle=control(switchButton(shortcutTitle,shortcut.enabled,()=>{
    const current=state!.settings.tray_shortcut??{enabled:false,key:null,modifiers:0};
    void act({kind:'tray_shortcut',...current,enabled:!current.enabled});
  }));shortcutToggle.id='tray-shortcut-toggle';shortcutToggle.setAttribute('aria-controls','tray-shortcut-editor');
  shortcutRow.append(shortcutDescription,shortcutToggle);
  const editor=element('div','shortcut-editor');editor.id='tray-shortcut-editor';editor.inert=!shortcut.enabled;editor.setAttribute('aria-hidden',String(!shortcut.enabled));
  const editorInner=element('div','shortcut-editor-inner');const editorContent=element('div','shortcut-editor-content');
  const shortcutChoice=element('div','shortcut-choice');shortcutChoice.append(element('span','shortcut-label',english?'Set shortcut':'단축키 지정'));
  const capture=control(button('','shortcut-capture',()=>void recordShortcut(capture)));capture.id='tray-shortcut-key';
  setShortcutLabel(capture,shortcutLabel(shortcut,english),false,false);
  capture.setAttribute('aria-label',english?'Choose tray shortcut':'트레이 모달 단축키 지정');
  shortcutChoice.append(capture);editorContent.append(element('div','shortcut-divider'),shortcutChoice);editorInner.append(editorContent);editor.append(editorInner);shortcutCard.append(shortcutRow,editor);shortcuts.append(shortcutCard);
  const env = element('section'); env.append(sectionLabel(m.environment)); const envCard = element('div','card environment');
  function selectRow(label: string, id: string, values: string[], labels: string[], selected: string, action: (value:string) => void) {
    const row = element('div','setting-row'); const text = element('label','item-title',label); text.htmlFor = id;
    const {wrap,trigger}=preferenceDropdown(id,label,values.map((value,i)=>({value,label:labels[i],lang:value==='ko'?'ko':value==='en'?'en':undefined})),selected,action);
    control(trigger); row.append(text,wrap); return row;
  }
  const numbersRow=element('div','startup');const numbersDescription=element('div','description');
  const numbersTitle=english?'Always show volume levels':'볼륨 수치 항상 표시';
  const numbersHint=element('p','item-hint',english?'Keep volume levels visible in quick controls.':'빠른 볼륨 조절에서 수치를 항상 표시합니다');numbersHint.id='tray-volume-numbers-hint';
  numbersDescription.append(element('p','item-title',numbersTitle),numbersHint);
  const numbersToggle=control(switchButton(numbersTitle,!!s.tray_volume_numbers_always_visible,()=>void act({kind:'tray_volume_numbers',enabled:!state!.settings.tray_volume_numbers_always_visible})));
  numbersToggle.id='tray-volume-numbers-toggle';numbersToggle.setAttribute('aria-describedby',numbersHint.id);numbersRow.append(numbersDescription,numbersToggle);
  envCard.append(selectRow(m.language,'language-select',['ko','en'],['한국어','English'],s.language,v => preferences({language:v})),element('div','setting-divider'),selectRow(m.theme,'theme-select',['system','light','dark'],[m.system,m.light,m.dark],s.theme,v => preferences({theme:v as Theme})),element('div','setting-divider'),numbersRow); env.append(envCard);
  const ops = element('div','card operations'); const reset = control(button(m.reset,'operation danger',resetDialog)); reset.id = 'reset-button';
  const quit = button(m.quit,'operation',() => { if (native && !busy) void invoke('quit').catch(e => { error = String(e); showToast(error); render(); }); }); quit.disabled = !native; ops.append(reset,element('div','setting-divider'),quit); content.append(startup,shortcuts,env,updateSection(english,native,render,()=>state?.settings.language==='en'),ops);
}
function shortcutLabel(shortcut:TrayShortcut,english:boolean) {
  if(!shortcut.key)return english?'Set shortcut':'단축키 지정';
  return [...(shortcut.modifiers&2?['Ctrl']:[]),...(shortcut.modifiers&1?['Alt']:[]),...(shortcut.modifiers&4?['Shift']:[]),...(shortcut.modifiers&8?['Win']:[]),shortcut.key.replace(/^(Key|Digit|Numpad)/,'')].join(' + ');
}
async function recordShortcut(button:HTMLButtonElement) {
  if(!controlsAvailable || busy || shortcutRecording)return;
  shortcutRecording=true;shortcutCaptureReady=false;revision++;
  setShortcutLabel(button,state?.settings.language==='en'?'Press a key':'키를 누르세요',true);
  button.setAttribute('aria-live','polite');
  try {
    if (native) await invoke('shortcut_recording',{recording:true});
    if(shortcutRecording)shortcutCaptureReady=true;
    else if (native) await invoke('shortcut_recording',{recording:false});
  } catch(reason) {shortcutRecording=false;error=String(reason);render();}
}
async function finishShortcutRecording(binding?:TrayShortcut) {
  if(!shortcutRecording)return;
  shortcutRecording=false;shortcutCaptureReady=false;
  const capture=document.getElementById('tray-shortcut-key');
  const shortcut=binding??state?.settings.tray_shortcut??{enabled:false,key:null,modifiers:0};
  if(capture)setShortcutLabel(capture,shortcutLabel(shortcut,state?.settings.language==='en'),false);
  try {
    if(binding)await act({kind:'tray_shortcut',...binding});
    if (native) await invoke('shortcut_recording',{recording:false});
    if(!binding)render();
  } catch(reason){error=String(reason);render();}
}
function resetDialog() {
  const m=t(); const dialog=element('dialog','confirm-dialog'); dialog.setAttribute('aria-labelledby','reset-title');
  const header=element('div','dialog-header'); const title=element('h2','dialog-title',m.resetTitle); title.id='reset-title'; header.append(title);
  const viewport=element('div','confirm-scroll'); viewport.append(element('p','dialog-text',m.resetHint));
  const buttons=element('div','dialog-actions');
  let closing=false;
  const close=(after?: () => void) => {
    if (closing) return; closing=true; dialog.classList.remove('is-open'); dialog.classList.add('is-closing');
    setTimeout(() => { dialog.close(); after?.(); },matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 150);
  };
  const cancel=button(m.cancel,'dialog-button',() => close());
  const confirm=button(m.confirm,'dialog-button danger',() => close(() => void act({kind:'reset'})));
  buttons.append(cancel,confirm); dialog.append(header,viewport,buttons); app.append(dialog);
  const disposeScrollbar=mountScrollbar(viewport,dialog,'modal');
  dialog.addEventListener('cancel',event => { event.preventDefault(); close(); });
  dialog.addEventListener('close',() => { disposeScrollbar(); dialog.remove(); document.getElementById('reset-button')?.focus(); });
  dialog.showModal(); requestAnimationFrame(() => dialog.classList.add('is-open')); cancel.focus();
}
function deviceList(flow: Flow, heading = true) {
  const m = t(); const section = element('section','device-section'); if (heading) section.append(sectionLabel(flow === 'render' ? m.outputs : m.inputs)); const list = element('div','card device-list');
  // Sonar owns the virtual Windows default. This picker changes its physical mic route.
  const devices = state!.sonar.devices.filter(d => d.role === 'capture' && d.enabled);
  const route = state!.sonar.routes.find(r => r.channel === 'microphone');
  const selected = route?.device_id;
  const micVolume = state!.sonar.volumes.find(v => v.channel === 'microphone');
  if (!devices.length) list.append(element('p','empty',m.empty));
  devices.forEach((device,i) => {
    const active = device.id === selected; const row = control(button('','device-row',() => {
      if (!active && route) void act({kind:'sonar',command:{operation:'route',channel:'microphone',device_id:device.id,expected:route.device_id}});
    })); row.id = `device-${flow}-${device.id}`;
    row.dataset.deviceMotionKey=JSON.stringify([flow,device.id]);
    row.disabled ||= !state!.separation_available || !route;
    row.dataset.endpoint = device.id;
    row.classList.toggle('selected',active); row.title = deviceLabel(device.name);
    // Device IDs and availability come from GG; Windows metadata only supplies the icon shape.
    const image = icon('Mic',quick ? 20 : 24);
    const label = element('span','device-name',deviceLabel(device.name));
    const check=icon('Check',quick ? 20 : 24);check.classList.add('check');
    const level=active ? micVolume?.volume ?? 0 : state!.settings.microphone_device_volumes?.[device.id] ?? 0;
    // Keep selection and volume feedback in one fixed slot on every input row.
    // Its parent fades on selection; its children own only the check/value swap.
    const status=element('span','input-volume-status');status.setAttribute('aria-hidden','true');
    status.append(check,element('span','sonar-percent',`${Math.round(level*100)}%`));
    const content=element('span','volume-content');content.append(image,label,status);
    const fill=element('span','volume-fill');fill.style.width='100%';fill.style.transform=`scaleX(${level})`;
    const surface=element('span','volume-surface');surface.append(fill);
    const clip=element('span','volume-surface-clip');clip.setAttribute('aria-hidden','true');clip.append(surface);
    row.append(clip,content);
    if (active && micVolume) {
      bindVolume(row,fill,{id:device.id,name:device.name,flow:'capture',form_factor:null,defaults:[],volume:{level:micVolume.volume,muted:micVolume.muted}},'microphone');
      bindFluidControls(row);
    }
    list.append(row);
    if (i < devices.length - 1) {
      const divider=element('div','device-divider');
      divider.dataset.dividerKey=JSON.stringify([flow,device.id,devices[i+1].id]);
      divider.setAttribute('aria-hidden','true');
      divider.classList.toggle('hidden',active || devices[i+1].id === selected);
      list.append(divider);
    }
  }); section.append(list); return section;
}
function sonarConnection() {
  const section = element('div','sonar-status');
  const sonar = state!.sonar;
  const english = state!.settings.language === 'en';
  const messages: Record<string,[string,string]> = english ? {
    disabled:['Sonar is turned off','Enable Sonar in GG, then retry.'],
    waiting:['Connecting to Sonar','Jack will connect when Sonar is ready.'],
    incompatible:['Unable to connect to Sonar','Check your GG version, then retry.'],
    unsupported_mode:['Classic mode required','Switch Sonar to Classic mode in GG.'],
    disconnected:['Connect to Sonar','Retry to open GG and reconnect.'],
  } : {
    disabled:['Sonar가 꺼져 있습니다','GG에서 Sonar를 켠 뒤 다시 시도해 주세요.'],
    waiting:['Sonar 연결 중','Sonar가 준비되면 자동으로 연결됩니다.'],
    incompatible:['Sonar를 연결할 수 없습니다','GG 버전을 확인한 뒤 다시 시도해 주세요.'],
    unsupported_mode:['Classic 모드가 필요합니다','GG에서 Sonar를 Classic 모드로 바꿔 주세요.'],
    disconnected:['Sonar 연결이 필요합니다','다시 시도하면 GG를 실행하고 연결합니다.'],
  };
  const [title,subtitle]=messages[sonar.connection]??messages.disconnected;
  const copy=element('div','sonar-status-copy');
  copy.append(element('h2','sonar-status-title',title),element('p','sonar-status-subtitle',subtitle));
  section.append(copy,retryButton(() => void act({kind:'sonar_retry'}),true));
  return section;
}
function configuredGroups(): (Group & {channel:OutputGroup})[] {
  const groups = state!.settings.groups ?? outputGroups.map((channel,i) => ({id:`00000000-0000-0000-0000-00000000000${i+1}`,display_name:state!.settings.language === 'en' ? ['Game','Chat','Media'][i] : ['게이밍','채팅','미디어'][i],sonar_channel:channel}));
  return groups.flatMap(group => {
    const channel = group.sonar_channel ?? ({'00000000-0000-0000-0000-000000000001':'game','00000000-0000-0000-0000-000000000002':'chat','00000000-0000-0000-0000-000000000003':'media'} as Record<string,OutputGroup>)[group.id];
    return channel ? [{...group,channel}] : [];
  });
}
function groupLabels(): Record<MixerChannel,string> {
  const labels: Record<MixerChannel,string> = state!.settings.language === 'en' ? {game:'Game',chat:'Chat',media:'Media',aux:'Aux',microphone:'Microphone',master:'Master'} : {game:'게이밍',chat:'채팅',media:'미디어',aux:'보조',microphone:'마이크',master:'마스터'};
  for (const group of configuredGroups()) if (group.channel === 'aux') labels[group.channel] = group.display_name;
  return labels;
}
function playbackIcon(name: string, size: IconSize = quick ? 20 : 24) {
  // Names are used only to select a visual asset, never to identify a route write.
  const matches = state!.audio.endpoints.filter(d => d.flow === 'render' && d.name === name);
  const factor = matches.length === 1 ? matches[0].form_factor : null;
  return icon(factor === 9 ? 'Desktop' : factor === 1 ? 'Speaker' : 'Headphones',size);
}
function sonarChannels(showApplications = true) {
  const english = state!.settings.language === 'en';
  const sonar = state!.sonar;
  const section = element('section','sonar-channels');
  if (!state!.separation_available) return section;
  if (showApplications) section.append(sectionLabel(english ? 'Audio groups' : '오디오 그룹'));
  const labels = groupLabels();
  const groups = configuredGroups();
  const routingByProcess = new Map<number,AppRoute>();
  let applicationsByChannel = new Map<string,ApplicationGroup[]>();
  let otherApplications:ApplicationGroup[]=[];
  if (showApplications) {
    const routing = state!.app_routing ?? [];
    for (const entry of routing) routingByProcess.set(entry.process_id,entry);
    // Unidentified sessions are internal inventory entries, not useful app controls.
    const applications=sonar.applications.filter(session =>
      routingByProcess.get(session.process_id)?.stable_id ||
      [session.display_name,session.process_name].some(name => name.trim() && !/^unk(?:nown|own)$/i.test(name.trim())));
    const partition=partitionApplications(applications,routing,groups.map(group=>group.channel));
    applicationsByChannel=partition.channels;otherApplications=partition.other;
  }
  for (const group of groups) {
    const channel = group.channel;
    const sourceAvailable = sonar.sources.some(s => s.channel === channel);
    const volume = sonar.volumes.find(v => v.channel === channel);
    const route = sonar.routes.find(r => r.channel === channel);
    const outputAvailable=sourceAvailable && !!volume && !!route && sonar.devices.some(device=>device.id===route.device_id && device.role==='playback' && device.enabled);
    const pickerAvailable=sourceAvailable && !!route && sonar.devices.some(device=>device.role==='playback' && device.enabled);
    const level=outputAvailable?volume!.volume:0;
    const card = element('div','card sonar-channel');
    card.dataset.channel=channel;card.dataset.dropDisabled=String(!sourceAvailable);
    card.dataset.outputUnavailable=String(!outputAvailable);
    const head = element('div','sonar-channel-head');
    const groupIcon = {game:'Gaming',chat:'Chat',media:'Media',aux:'Puzzle'}[channel];
    const title=element('div','sonar-channel-title'); title.append(icon(groupIcon,quick ? 20 : 24),element('h2','item-title',labels[channel])); head.append(title);
    if (showApplications && channel === 'aux') {
      const remove = control(button('','sonar-group-remove',() => void act({kind:'remove_group',group_id:group.id})));
      remove.id = `group-remove-${group.id}`; remove.setAttribute('aria-label',english ? `Delete ${labels[channel]} group` : `${labels[channel]} 그룹 삭제`); remove.append(icon('X',16)); head.append(remove);
    }
    card.append(head);
    {
      const row = control(button('','device-row selected',() => {})); row.id = `sonar-volume-${channel}`;
      row.dataset.deviceMotionKey=JSON.stringify(['output',channel]);
      row.dataset.endpoint=route?.device_id??'';
      row.dataset.deviceMotionOrder=JSON.stringify(sonar.devices.filter(device=>device.role==='playback' && device.enabled).map(device=>device.id));
      row.disabled ||= !outputAvailable;
      const name = route?.device_name ?? (english ? 'Output disconnected' : '출력 장치 연결 해제');
      const fill = element('span','volume-fill'); fill.style.width = `${level * 100}%`;
      const deviceIcon=playbackIcon(name),deviceName=element('span','device-name',deviceLabel(name));
      if(!quick) {
        const drum=element('span','device-drum'),face=element('span','device-drum-face');
        face.append(deviceIcon,deviceName);drum.append(face);row.append(fill,drum);
        row.dataset.deviceDrumItems=JSON.stringify(sonar.devices.filter(device=>device.role==='playback' && device.enabled).map(device=>({
          id:device.id,label:deviceLabel(device.name),icon:playbackIcon(device.name).dataset.iconAsset!,
        })));
      } else row.append(fill,deviceIcon,deviceName);
      row.append(element('span','sonar-percent',`${Math.round(level * 100)}%`));
      if (!native && import.meta.env.DEV && params.has('live-volume')) row.disabled = !outputAvailable;
      bindVolume(row,fill,{id:channel,name:labels[channel],flow:'render',form_factor:null,defaults:[],volume:{level,muted:volume?.muted ?? false}},channel);
      const deviceControl=element('div','sonar-device-control');deviceControl.append(row);card.append(deviceControl);
      {
        const wrapper = element('div','sonar-device-picker');
        if (quick) {
          const open = button('','sonar-output-open',() => { quickOutputGroup = channel; renderQuick(); document.getElementById('quick-output-back')?.focus(); });
          open.disabled = busy || !pickerAvailable;
          open.id = `sonar-output-${channel}`; open.setAttribute('aria-label',`${labels[channel]} ${t().outputs}`); open.append(chevron('right',20)); wrapper.append(open);
        } else {
          let openedBeforePointer = false;
          const open = button('','sonar-output-open',() => {
            if (openedBeforePointer) { dismissOutputMenu?.(); openedBeforePointer = false; return; }
            openOutputMenu(channel,open);
          });
          open.addEventListener('pointerdown',() => openedBeforePointer = open.getAttribute('aria-expanded') === 'true',{capture:true});
          open.disabled = busy || !pickerAvailable; open.id = `sonar-output-${channel}`;
          open.setAttribute('aria-label',`${labels[channel]} ${t().outputs}`); open.setAttribute('aria-haspopup','dialog'); open.setAttribute('aria-expanded','false');
          open.append(chevron('down',24)); wrapper.append(open);
        }
        deviceControl.append(wrapper);
        bindFluidControls(deviceControl);
      }
    }
    if (showApplications) {
      const applications = element('div','sonar-app-list');
      for (const application of applicationsByChannel.get(channel) ?? []) {
        if (applications.childElementCount) applications.append(element('div','sonar-app-divider'));
        applications.append(sonarApplication(application,labels,routingByProcess));
      }
      card.append(applications);
    }
    section.append(card);
  }
  if (showApplications) {
    if(otherApplications.length) {
      const card=element('div','card sonar-channel');
      card.append(element('h2','item-title',english?'Other applications':'다른 앱'));
      const applications=element('div','sonar-app-list');
      for(const application of otherApplications) {
        if(applications.childElementCount)applications.append(element('div','sonar-app-divider'));
        applications.append(sonarApplication(application,labels,routingByProcess));
      }
      card.append(applications);section.append(card);
    }
    const add = control(button('','sonar-group-add',() => void act({kind:'add_group'}))); add.id = 'group-add';
    add.append(icon('Plus'),element('span','',english ? 'Add group' : '그룹 추가'));
    add.disabled = !controlsAvailable || groups.length >= 4;
    add.setAttribute('aria-disabled',String(add.disabled || busy));
    add.title = groups.length >= 4 ? (english ? 'Maximum four groups' : '최대 4개 그룹') : '';
    section.append(add);
  }
  return section;
}
function openOutputMenu(channel: OutputGroup, trigger: HTMLButtonElement) {
  const existing = document.getElementById('sonar-output-menu');
  if (existing?.matches(':popover-open')) { existing.hidePopover(); return; }
  existing?.remove();
  dismissOutputMenu?.();
  const route = state!.sonar.routes.find(r => r.channel === channel);
  if (!route || !state!.separation_available || busy) return;
  const menu = element('div','sonar-output-menu'); menu.id = 'sonar-output-menu'; menu.popover = 'auto'; menu.tabIndex = -1;
  menu.role = 'dialog'; menu.setAttribute('aria-label',`${groupLabels()[channel]} ${t().outputs}`);
  const devices = state!.sonar.devices.filter(d => d.role === 'playback' && d.enabled);
  const list = element('div','device-list');
  if (!devices.length) list.append(element('p','empty',t().empty));
  for (const device of devices) {
    const selected = device.id === route.device_id;
    const row = control(button('','device-row',() => {
      menu.hidePopover(); trigger.focus();
      if (!selected || state!.settings.link_group_outputs) void act({kind:'sonar',command:{operation:'route',channel,device_id:device.id,expected:route.device_id}});
    }));
    row.id = `main-output-${channel}-${device.id}`; row.dataset.endpoint = device.id;
    row.classList.toggle('route-selected',selected); row.setAttribute('aria-pressed',String(selected));
    const name = deviceLabel(device.name);
    row.title = name; row.append(playbackIcon(device.name),element('span','device-name',name));
    if (selected) row.append(icon('Check',quick ? 20 : 24));
    list.append(row);
  }
  const viewport=element('div','menu-viewport'); viewport.append(list); menu.append(viewport); app.append(menu);
  const disposeScrollbar=mountScrollbar(viewport,menu,'menu');
  const anchor = trigger.closest('.sonar-device-control')!.getBoundingClientRect();
  const margin = 12;
  const width = Math.min(anchor.width,window.innerWidth - margin * 2);
  const below = Math.max(0,window.innerHeight - anchor.bottom - 8 - margin);
  const above = Math.max(0,anchor.top - 8 - margin);
  menu.style.width = `${width}px`; menu.style.left = `${Math.max(margin,Math.min(anchor.left,window.innerWidth - width - margin))}px`;
  const desiredHeight=devices.length*48+Math.max(0,devices.length-1)*4+8;
  const opensUp=below<desiredHeight && above>below;
  menu.dataset.direction=opensUp ? 'up' : 'down';
  menu.style.maxHeight = `${Math.max(48,opensUp ? above : below)}px`;
  viewport.style.maxHeight=`${Math.max(40,(opensUp ? above : below)-8)}px`;
  // Position before entering the top layer; the first open frame must already
  // use the final anchor rather than the popover's automatic initial position.
  const initialHeight=Math.min(desiredHeight,Math.max(48,opensUp ? above : below));
  menu.style.top = `${opensUp ? Math.max(margin,anchor.top-initialHeight-8) : anchor.bottom+8}px`;
  menu.showPopover();
  const height = menu.offsetHeight;
  menu.style.top = `${opensUp ? Math.max(margin,anchor.top - height - 8) : anchor.bottom + 8}px`;
  trigger.setAttribute('aria-expanded','true');
  const dismissal = new AbortController();
  const close = () => { if (menu.matches(':popover-open')) menu.hidePopover(); };
  let cleaned = false;
  const cleanup = () => {
    if (cleaned) return;
    cleaned = true; dismissal.abort(); disposeScrollbar(); menu.remove(); trigger.setAttribute('aria-expanded','false');
    if (dismissOutputMenu === dismiss) dismissOutputMenu = null;
  };
  const dismiss = () => { close(); cleanup(); };
  dismissOutputMenu = dismiss;
  window.addEventListener('resize',close,{signal:dismissal.signal});
  document.getElementById('panel')?.addEventListener('scroll',close,{signal:dismissal.signal});
  menu.addEventListener('toggle',() => {
    if (menu.matches(':popover-open')) return;
    trigger.setAttribute('aria-expanded','false');
    const restore = menu.contains(document.activeElement) || document.activeElement === document.body;
    setTimeout(cleanup,matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 150);
    if (restore && trigger.isConnected) trigger.focus({preventScroll:true});
  });
  menu.addEventListener('keydown',e => {
    if (!['ArrowUp','ArrowDown','Home','End'].includes(e.key)) return;
    const choices = [...menu.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
    if (!choices.length) return;
    e.preventDefault();
    const index = choices.indexOf(document.activeElement as HTMLButtonElement);
    const next = e.key === 'Home' ? 0 : e.key === 'End' ? choices.length - 1 : index < 0 ? (e.key === 'ArrowDown' ? 0 : choices.length - 1) : (index + (e.key === 'ArrowDown' ? 1 : choices.length - 1)) % choices.length;
    choices[next].focus();
  });
  (menu.querySelector<HTMLButtonElement>('.route-selected:not(:disabled)') ?? menu).focus({preventScroll:true});
}
function sonarApplication(application: ApplicationGroup, labels: Record<MixerChannel,string>, routingByProcess: ReadonlyMap<number,AppRoute>) {
  const session = application.session;
  const row = element('div','sonar-app');
  const name = session.display_name || session.process_name;
  const meta=routingByProcess.get(session.process_id);
  const info=element('div','sonar-app-info');info.append(element('span','device-name',name));
  const programIcon=element('img','sonar-app-icon');programIcon.alt='';programIcon.width=programIcon.height=24;programIcon.draggable=false;
  let iconSource: string | null | undefined;
  for (const pid of application.process_ids) {
    iconSource=routingByProcess.get(pid)?.icon_data_url;
    if (iconSource) break;
  }
  if (iconSource) programIcon.src=iconSource; else programIcon.style.visibility='hidden';
  const handle=element('span','sonar-app-handle');handle.setAttribute('aria-hidden','true');handle.append(icon('reorder'));
  row.append(programIcon,info,handle);row.id=`sonar-app-${session.process_id}`;row.tabIndex=0;row.role='button';
  row.setAttribute('aria-label',`${name}, ${labels[session.route as MixerChannel] ?? session.route}`);
  row.title=state!.settings.language==='en' ? `${name}: drag to another group · Shift+↑/↓` : `${name}: 다른 그룹으로 드래그 · Shift+↑/↓`;
  const movable=!!meta?.stable_id && !session.routing_error;
  row.setAttribute('aria-disabled',String(!movable || busy));
  const moveTo=(channel:string) => {
    if (!native && import.meta.env.DEV) {
      // Preview gesture/model only: no audio policy or GG requests in a browser.
      state!.sonar.applications.filter(member => application.process_ids.includes(member.process_id)).forEach(member => member.route=channel);
      state!.app_routing?.filter(member => application.process_ids.includes(member.process_id)).forEach(member => {member.requested=channel as Channel;member.origin='user';});
      render();
      return Promise.resolve();
    }
    return act({kind:'assign_application',process_id:session.process_id,process_name:session.process_name,channel,automatic:false});
  };
  bindAppDrag(row,{canStart:() => movable && !busy && !dragging && (native || import.meta.env.DEV),onStart:cancel => {dragging=true;cancelDrag=cancel;},onFinish:() => {dragging=false;cancelDrag=null;render();},onDrop:moveTo});
  row.addEventListener('keydown',event => {
    if (!event.shiftKey || !['ArrowUp','ArrowDown'].includes(event.key) || !movable || busy || dragging || !controlsAvailable) return;
    const groups=configuredGroups().filter(group => state!.sonar.sources.some(source => source.channel===group.channel));
    const index=groups.findIndex(group => group.channel===session.route);const target=groups[index+(event.key==='ArrowDown'?1:-1)];
    if (target) {event.preventDefault();void moveTo(target.channel);}
  });
  return row;
}
function quickOutputDevices(channel: OutputGroup) {
  const section = element('section','quick-output-devices');
  const label = groupLabels()[channel];
  const back = button('','quick-output-back',() => { quickOutputGroup = null; renderQuick(); document.getElementById(`sonar-output-${channel}`)?.focus(); });
  back.id = 'quick-output-back'; back.setAttribute('aria-label',state!.settings.language === 'en' ? `Back from ${label}` : `${label}에서 뒤로 가기`);
  back.append(chevron('left',20),element('h2','item-title',label)); section.append(back);
  const route = state!.sonar.routes.find(r => r.channel === channel);
  if (!state!.separation_available || !route) { if (state!.sonar.connection !== 'connected') section.append(sonarConnection()); else section.append(element('p','empty',t().empty)); return section; }
  const devices = state!.sonar.devices.filter(d => d.role === 'playback' && d.enabled);
  const list = element('div','device-list');
  if (!devices.length) list.append(element('p','empty',t().empty));
  for (const device of devices) {
    const selected = device.id === route.device_id;
    const row = control(button('','device-row',() => {
      if (selected && !state!.settings.link_group_outputs) return;
      void act({kind:'sonar',command:{operation:'route',channel,device_id:device.id,expected:route.device_id}}).then(() => {
        if (!error) { quickOutputGroup = null; renderQuick(); document.getElementById(`sonar-output-${channel}`)?.focus(); }
      });
    }));
    row.id = `quick-output-${channel}-${device.id}`; row.dataset.endpoint = device.id;
    row.classList.toggle('route-selected',selected); row.setAttribute('aria-pressed',String(selected));
    const name = deviceLabel(device.name);
    row.title = name; row.append(playbackIcon(device.name),element('span','device-name',name));
    if (selected) row.append(icon('Check',quick ? 20 : 24));
    list.append(row);
  }
  section.append(list); return section;
}
function scheduleQuickOutputPreparation() {
  cancelQuickPreparation?.();cancelQuickPreparation=null;
  if(!state || quickOutputGroup || busy || dragging || !quickSurface)return;
  const prepare=()=>{
    cancelQuickPreparation=null;
    if(!state || quickOutputGroup || busy || dragging || quickSurface!.moving)return;
    for(const group of configuredGroups()) {
      if(quickSurface!.hasPrepared(group.channel))continue;
      const page=element('div','quick-content');page.dataset.pageId='2';page.dataset.viewKey=group.channel;
      page.append(quickOutputDevices(group.channel));
      quickSurface!.prepare(page);
    }
  };
  if('requestIdleCallback' in window) {
    const job=window.requestIdleCallback(prepare);
    cancelQuickPreparation=()=>window.cancelIdleCallback(job);
  } else {
    const job=setTimeout(prepare,0);
    cancelQuickPreparation=()=>clearTimeout(job);
  }
}
function renderQuick() {
  retireVolumeMotions();
  const focused=(document.activeElement as HTMLElement)?.id;
  if(quickOutputGroup && !configuredGroups().some(group=>group.channel===quickOutputGroup)) quickOutputGroup=null;
  const subview=quickOutputGroup??'';
  theme(); const m=t();
  let shell=app.querySelector<HTMLElement>('.quick-shell');
  if(!shell) {
    shell=element('div','quick-shell'); const panel=element('main','quick-panel'); panel.setAttribute('aria-label','Jack');
    const viewport=element('div','quick-viewport'); const stage=element('div','quick-stage');
    viewport.append(stage); panel.append(viewport); shell.append(panel); app.replaceChildren(shell);
    quickSurface=new QuickSurface(stage,resizeQuickTo,scheduleQuickOutputPreparation);
    shell.addEventListener('pointerdown',event=>{if(event.target===shell && native)void invoke('hide_to_tray');});
  }
  const panel=shell.querySelector<HTMLElement>('.quick-panel')!;
  // Prepared buttons capture the expected route. Invalidate them whenever that
  // data changes so a cached view never submits an obsolete route write.
  const preparationKey=state?JSON.stringify([state.settings,state.separation_available,state.sonar.connection,state.sonar.routes,state.sonar.devices,state.audio.endpoints.map(({id,name,flow,form_factor})=>[id,name,flow,form_factor]),busy]):'';
  if(preparationKey!==quickPreparationKey) {
    cancelQuickPreparation?.();cancelQuickPreparation=null;
    quickSurface!.clearPrepared();quickPreparationKey=preparationKey;
  }
  panel.dataset.subview=subview;
  const prepared=quickOutputGroup?quickSurface!.takePrepared(quickOutputGroup):null;
  const content=prepared??element('div','quick-content'); content.dataset.pageId=subview?'2':'1';content.dataset.viewKey=subview;
  if(!state)content.append(element('p','empty',error||m.loading));
  else if(quickOutputGroup){if(!prepared)content.append(quickOutputDevices(quickOutputGroup));}
  else if(!state.separation_available)content.append(sonarConnection());
  else {
    content.append(sonarChannels(false),element('div','quick-divider'));
    const accordion=element('section','t-acc'); accordion.dataset.open=String(inputExpanded);
    const head=button('','t-acc-head quick-input-toggle',()=>{
      inputExpanded=!inputExpanded; const expanded=inputExpanded;
      quickSurface!.resizeContent(body,()=>{
        accordion.dataset.open=String(expanded); head.setAttribute('aria-expanded',String(expanded)); inner.inert=!expanded;
      });
    });
    head.id='input-expand'; head.setAttribute('aria-expanded',String(inputExpanded)); head.setAttribute('aria-controls','quick-inputs');
    head.append(element('span','',m.inputs)); const arrow=element('span','t-acc-chevron'); arrow.append(chevron('down',16)); head.append(arrow);
    const body=element('div','t-acc-panel');body.id='quick-inputs';
    const inner=element('div','t-acc-panel-inner');inner.inert=!inputExpanded;inner.append(deviceList('capture',false));
    body.append(inner);accordion.append(head,body);content.append(accordion);
  }
  quickSurface!.show(content,!native || visible);
  if(focused)document.getElementById(focused)?.focus({preventScroll:true});
}
function bindVolume(row: HTMLButtonElement, fill: HTMLElement, device: Endpoint, channel: Channel) {
  const volume=device.volume!;
  let next=volume.level; let pointer:number|null=null; let stream:LiveVolume|null=null;
  let bounds:DOMRect|null=null; let held=false; let releasing=false; let epoch=0;
  const available=()=>!row.disabled && (controlsAvailable || (import.meta.env.DEV && params.has('live-volume'))) && !busy && (!dragging || releasing);
  row.role='slider'; row.setAttribute('aria-label',`${deviceLabel(device.name)} ${t().volume}`);
  row.setAttribute('aria-valuemin','0');row.setAttribute('aria-valuemax','100');row.setAttribute('aria-valuenow',String(Math.round(next*100)));
  let percent=row.querySelector<HTMLElement>('.sonar-percent');
  if(!percent){percent=element('span','sonar-percent',`${Math.round(next*100)}%`);row.append(percent);}
  const inputStatus=row.querySelector<HTMLElement>('.input-volume-status');
  const content=row.querySelector<HTMLElement>(':scope > .volume-content')??element('span','volume-content');
  const clip=row.querySelector<HTMLElement>(':scope > .volume-surface-clip')??element('span','volume-surface-clip');
  [...row.children].filter(child=>child!==fill && child!==content && child!==clip).forEach(child=>content.append(child));
  clip.setAttribute('aria-hidden','true');
  const surface=clip.querySelector<HTMLElement>('.volume-surface')??element('span','volume-surface');
  surface.append(fill);clip.append(surface);row.append(clip,content);
  // A microphone channel is shared by Sonar, but each physical input owns its
  // painted level. Never carry another device's position or velocity into it.
  const motionKey=JSON.stringify([channel,channel==='microphone'?device.id:null]);
  const initial=row.disabled?undefined:previousVolumeMotions.get(motionKey);
  let paintedPercent=-1;
  const motion=new VolumeMotion(fill,next,level=>{
    row.classList.toggle('volume-changing',Math.abs(level-next)>.0002);
    const value=Math.round(level*100);
    if(value!==paintedPercent){paintedPercent=value;percent!.textContent=`${value}%`;}
  },initial);
  volumeMotions.set(motionKey,motion);motion.set(next);
  const update=(value:number,overrun=0)=>{
    const clamped=Math.min(1,Math.max(0,value));next=Math.round(clamped*100)/100;
    updateFluidVolume(row,overrun,clamped);
    motion.set(next);row.setAttribute('aria-valuenow',String(Math.round(next*100)));
    stream?.set(next);
  };
  const begin=(isPointer:boolean)=>{
    revision++;epoch++;error='';dragging=true;held=isPointer;releasing=false;bounds=row.getBoundingClientRect();
    row.classList.remove('volume-returning');
    if(inputStatus)row.classList.add('volume-interacting');
    stream=new LiveVolume(next,async(level,expected)=>{
      const actual=native?await invoke<number>('audio_volume',{target:{kind:'sonar',channel},level,expected}):level;
      const entry=state?.sonar.volumes.find(value=>value.channel===channel);if(entry)entry.volume=actual;
      if(state && channel==='microphone' && state.sonar.routes.some(route=>route.channel===channel && route.device_id===device.id)) {
        (state.settings.microphone_device_volumes??={})[device.id]=actual;
      }
      return actual;
    },failure=>{error=String(failure);showToast(error);});
    if(held){row.classList.add('adjusting');void holdFluidVolume(row,true);motion.grab();}
  };
  const end=async(flush:boolean)=>{
    const active=stream;if(!active)return;
    stream=null;const captured=pointer;pointer=null;cancelDrag=null;
    if(captured!==null && row.hasPointerCapture(captured))row.releasePointerCapture(captured);
    const ending=epoch;releasing=true;
    row.classList.remove('adjusting','volume-interacting');row.classList.add('volume-returning');
    // A short tap can settle the spring before the color fade has finished.
    // Keep this painted node alive for the spring, color fade, and both halves
    // of the input status swap, so rendering cannot cut off the returning check.
    const colorTime=getComputedStyle(fill).getPropertyValue('--volume-color-duration').trim();
    const reduced=matchMedia('(prefers-reduced-motion: reduce)').matches;
    const colorDuration=held && !reduced
      ? cssTimeMs(colorTime,250) : 0;
    const statusDuration=inputStatus && !reduced
      ? cssTimeMs(getComputedStyle(inputStatus).getPropertyValue('--volume-status-duration'),150)*2 : 0;
    const faded=new Promise<void>(resolve=>setTimeout(resolve,Math.max(colorDuration,statusDuration)));
    const released=motion.release();busy=true;
    // Keyboard changes can finish their spring after the color/status fades.
    // Let the number's exit finish before a render replaces this painted node.
    const numberDuration=!inputStatus && !reduced && !(quick && state?.settings.tray_volume_numbers_always_visible)
      ? cssTimeMs(getComputedStyle(percent!).getPropertyValue('--volume-number-duration'),150) : 0;
    const numberFaded=numberDuration>0?released.then(()=>new Promise<void>(resolve=>setTimeout(resolve,numberDuration))):released;
    const pairReleased=holdFluidVolume(row,false);
    await active.finish(flush);
    if(ending===epoch)busy=false;
    await Promise.all([numberFaded,faded,pairReleased]);
    if(ending!==epoch)return;
    releasing=false;row.classList.remove('volume-returning');dragging=false;
    // Volume state is already current. Replacing these controls now would cut
    // off a picker press or dismiss the menu opened during the release spring.
    const choosingOutput=!!document.querySelector('.sonar-output-open:active,.sonar-output-open[aria-expanded=true]') || (quick&&quickOutputGroup!==null);
    if(!choosingOutput)render();
    if(native)void refresh(true);
  };
  const cancel=()=>{if(stream)void end(false);};
  const fromPointer=(x:number)=>{
    if(!bounds)return;
    // The hit track stays fixed while its paint expands. Keep the pointer map
    // captured at gesture start instead of measuring after every style write.
    const overrun=x<bounds.left?x-bounds.left:x>bounds.right?x-bounds.right:0;
    update((x-bounds.left)/bounds.width,overrun);
  };
  row.addEventListener('pointerdown',event=>{
    if(!available() || pointer!==null || event.button!==0)return;
    event.preventDefault();pointer=event.pointerId;begin(true);cancelDrag=cancel;row.focus({preventScroll:true});row.setPointerCapture(pointer);
    fromPointer(event.clientX);
  });
  row.addEventListener('pointermove',event=>{if(pointer===event.pointerId)fromPointer(event.clientX);});
  row.addEventListener('pointerup',event=>{if(pointer===event.pointerId){fromPointer(event.clientX);void end(true);}});
  const cancelPointer=(event:PointerEvent)=>{if(pointer===event.pointerId)cancel();};
  row.addEventListener('pointercancel',cancelPointer);row.addEventListener('lostpointercapture',cancelPointer);
  row.addEventListener('keydown',event=>{
    if(!available() || pointer!==null || !['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End'].includes(event.key))return;
    event.preventDefault();begin(false);update(event.key==='Home'?0:event.key==='End'?1:next+(['ArrowRight','ArrowUp'].includes(event.key)?1:-1)*(event.shiftKey?.01:.05));void end(true);
  });
}
const refresh=singleFlight(async (force = false) => {
  if (busy || dragging || windowDragging || shortcutRecording || (!force && (document.hidden || !visible))) return;
  // A focused, closed picker must not stop external changes from reaching the UI.
  // Keep an actually open native menu intact while a background read finishes.
  const editing = () => (quick && quickSurface?.moving) || !!document.querySelector('.sonar-output-open:active, dialog[open], [popover]:popover-open') || (CSS.supports('selector(select:open)')
    ? !!document.querySelector('select:open') : document.activeElement?.tagName === 'SELECT');
  const requestedRevision = revision;
  try {
    if (native && !editing()) {
      const next = await invoke<ViewState>('audio_action',{action:{kind:'read'}});
      if (requestedRevision !== revision || busy || dragging || windowDragging || shortcutRecording || editing()) return;
      const visible = (value: ViewState | null) => value ? JSON.stringify([
        value.settings,value.audio.endpoints.map(({id,name,flow,form_factor})=>[id,name,flow,form_factor]),value.separation_available,
        value.sonar.connection,value.sonar.message,value.sonar.mode,value.sonar.volumes,value.sonar.routes,value.sonar.devices,value.sonar.applications,
        value.app_routing?.map(({process_id,stable_id,requested,origin,pending,message,icon_data_url})=>[process_id,stable_id,requested,origin,pending,message,icon_data_url]),value.routing_warnings,
      ]) : '';
      if (visible(next) !== visible(state)) { state = next; render(); void reportNative().catch(e => { error = String(e); render(); }); }
    }
  } catch (e) { if (requestedRevision === revision) { const message=String(e);if(state&&error!==message)showToast(message);error=message;render(); } }
});
function fixture(): ViewState {
  const names = ['Razer BlackShark V2 Pro','LG TV SSCR2(NVIDIA High Definition Audio)','Sony WH-1000XM6','Speaker'];
  const preview: ViewState = {settings:{theme:'system' as Theme,language:'ko',launch_at_login:true,channel_separation_enabled:true,link_group_outputs:true},separation_available:params.has('sonar'),sonar:{connection:'disconnected',message:null,mode:null,volumes:[],routes:[],devices:[],applications:[],sources:[]},audio:{warnings:[] as string[],endpoints:(['render','capture'] as Flow[]).flatMap(flow => names.map((name,i) => ({id:`preview-${flow}-${i}`,name,flow,form_factor:[3,9,3,1][i],defaults:i === 0 ? ['console','multimedia'] : [],volume:{level:.25735,muted:false}})))}};
  if (params.has('sonar')) preview.sonar = {connection:'connected',message:null,mode:'classic',volumes:(['master','game','chat','media','aux','microphone'] as MixerChannel[]).map(channel => ({channel,volume:.4,muted:false})),routes:(['game','chat','media','aux','microphone'] as Channel[]).map(channel => ({channel,device_id:'preview-output',device_name:names[0]})),devices:[{id:'preview-output',name:names[0],role:'playback',enabled:true}],applications:[{process_id:123,process_name:'Discord',display_name:'Discord',route:'chat',activity:'active',routing_error:false}],sources:(['game','chat','media','aux'] as Channel[]).map(channel => ({channel,endpoint_id:'preview-'+channel}))};
  if (params.has('sonar') && params.get('fixture') === 'design') {
    preview.sonar.applications = [['VALORANT','game'],['VALORANT','game'],['Discord','chat'],['Kakaotalk','chat'],['Apple Music','media'],['Spotify','media']].map(([name,route],i) => ({process_id:100+i,process_name:name,display_name:name,route,activity:'active',routing_error:false}));
    preview.sonar.volumes.forEach(v => v.volume = .63);
    preview.sonar.devices = names.map((name,i) => ({id:i === 0 ? 'preview-output' : `preview-output-${i}`,name,role:'playback',enabled:true}));
    preview.settings.groups = outputGroups.map((channel,i) => ({id:`00000000-0000-0000-0000-00000000000${i+1}`,display_name:['게이밍','채팅','미디어'][i],sonar_channel:channel}));
    if (params.get('groups') === '4') preview.settings.groups.push({id:'00000000-0000-0000-0000-000000000004',display_name:'보조',sonar_channel:'aux',icon_asset_id:'Frame-1'});
  }
  if (params.has('sonar')) {
    preview.sonar.devices.push(...names.slice(0,3).map((name,i) => ({id:`preview-capture-${i}`,name:`Microphone (${name})`,role:'capture' as const,enabled:true})));
    const mic = preview.sonar.routes.find(route => route.channel === 'microphone')!;
    mic.device_id = 'preview-capture-0'; mic.device_name = `Microphone (${names[0]})`;
    // Seed every input before the first gesture, not when it is first selected.
    const micLevel=preview.sonar.volumes.find(volume=>volume.channel==='microphone')!.volume;
    preview.settings.microphone_device_volumes=Object.fromEntries(preview.sonar.devices
      .filter(device=>device.role==='capture' && device.enabled)
      .map(device=>[device.id,micLevel]));
    // Raw Windows VADs must not appear in a physical-device picker.
    preview.audio.endpoints.push({id:'preview-sonar-vad',name:'SteelSeries Sonar - Microphone',flow:'capture',form_factor:4,defaults:['console'],volume:{level:.8,muted:false}});
  }
  if (params.get('missing-source') === 'aux') {
    preview.sonar.sources = preview.sonar.sources.filter(source => source.channel !== 'aux');
    if (params.has('missing-volume')) preview.sonar.volumes = preview.sonar.volumes.filter(volume => volume.channel !== 'aux');
  }
  if (params.has('grouped-apps')) preview.app_routing = preview.sonar.applications.map(session => ({process_id:session.process_id,stable_id:`path:preview/${session.process_name}`,requested:session.route as Channel,origin:'automatic',pending:false}));
  if (params.get('theme') === 'light' || params.get('theme') === 'dark') preview.settings.theme = params.get('theme') as Theme;
  if (params.get('lang') === 'en') preview.settings.language = 'en';
  preview.settings.tray_volume_numbers_always_visible=params.has('volume-numbers');
  if (params.has('long')) {
    const suffix=' — USB High Definition Audio Device with a very long device name';
    preview.audio.endpoints.forEach(device=>device.name+=suffix);
    preview.sonar.devices.forEach(device=>device.name+=suffix);
    preview.sonar.routes.forEach(route=>route.device_name=(route.device_name ?? '')+suffix);
  }
  if (params.has('empty')) {
    preview.audio.endpoints=[];preview.sonar.devices=[];preview.sonar.routes=[];preview.sonar.applications=[];
  }
  if (params.has('disconnected')) {
    preview.separation_available=false;preview.sonar.connection='disconnected';preview.sonar.mode=null;
  }
  return preview;
}
if (params.has('tray-menu')) {
  void import('./tray-menu').then(module=>module.mountTrayMenu(app));
} else if (import.meta.env.DEV && params.has('assets')) {
  app.className = 'asset-sheet';
  ['Gaming','Chat','Media','Puzzle','Desktop','Headphones','Speaker','Mic','Check','Plus','reorder'].forEach(name => { const card = element('div','asset-cell',name); card.append(icon(name)); app.append(card); });
} else {
  if (!native && import.meta.env.DEV) { state = fixture(); }
  else if (!native) { error = 'Jack 앱에서 열어 주세요. / Open this screen in Jack.'; }
  render();
  if (!native && import.meta.env.DEV && params.has('program-icons')) {
    void fetch('/@fs/C:/Projects/Jack/artifacts/program-icons/fixture.json').then(response => response.json()).then((icons:{name:string;stable_id:string;icon_data_url:string}[]) => {
      if (!state || !icons.length) return;
      state.sonar.applications=icons.slice(0,3).map((image,index) => ({process_id:100+index,process_name:image.name,display_name:image.name,route:outputGroups[index],activity:'active',routing_error:false}));
      state.app_routing=icons.slice(0,3).map((image,index) => ({process_id:100+index,stable_id:image.stable_id,requested:outputGroups[index],origin:'user',pending:false,icon_data_url:image.icon_data_url}));
      render();
    }).catch(reason => {error=String(reason);render();});
  }
  if (native) {
    if(!quick)void listen<TrayShortcut>('jack-shortcut-key',event=>{
      if(shortcutRecording&&shortcutCaptureReady)void finishShortcutRecording(event.payload);
    });
  }
  if (controlsAvailable) {
    if(!quick) {
      document.addEventListener('keydown',event=>{
        if(!shortcutRecording)return;
        event.preventDefault();event.stopImmediatePropagation();
        if(!shortcutCaptureReady || event.repeat)return;
        if(event.key==='Escape'){void finishShortcutRecording();return;}
        if(['Control','Alt','Shift','Meta','AltGraph'].includes(event.key))return;
        if(!/^(Key[A-Z]|Digit[0-9]|F([1-9]|1[0-9]|2[0-4])|Space|Enter|Tab|Backspace|PrintScreen|ScrollLock|Pause|CapsLock|NumLock|Numpad([0-9]|Multiply|Add|Subtract|Decimal|Divide|Enter)|AudioVolume(Mute|Down|Up)|Media(TrackNext|TrackPrevious|Stop|PlayPause)|ContextMenu|Arrow(Left|Right|Up|Down)|Home|End|PageUp|PageDown|Insert|Delete|Minus|Equal|BracketLeft|BracketRight|Backslash|Semicolon|Quote|Comma|Period|Slash|Backquote)$/.test(event.code))return;
        void finishShortcutRecording({enabled:true,key:event.code,modifiers:(event.altKey?1:0)|(event.ctrlKey?2:0)|(event.shiftKey?4:0)|(event.metaKey?8:0)});
      },{capture:true});
      window.addEventListener('blur',()=>{if(shortcutRecording)void finishShortcutRecording();});
      document.addEventListener('pointerdown',event=>{
        if(shortcutRecording && !(event.target instanceof Element && event.target.closest('#tray-shortcut-key')))void finishShortcutRecording();
      },{capture:true});
    }
  }
  if (native) {
    if(!quick)document.addEventListener('mousedown',event=>{
      if(event.button!==0 || windowDragging)return;
      const target=event.target;
      if(!(target instanceof Element) || !target.closest('.header') || target.closest('.tabs,button,a,input,select,textarea,[contenteditable=true]'))return;
      // One permanent handler for logo, text AND header gaps. Do not combine
      // Tauri's automatic drag region with a second pointerdown drag request.
      event.preventDefault();event.stopPropagation();windowDragging=true;revision++;
      void invoke('begin_window_drag').catch(reason=>{error=String(reason);}).finally(()=>{
        windowDragging=false;render();void refresh(true);
      });
    },{capture:true});
    void refresh(true);
    void listen<Settings>('jack-preferences',event=>{
      if(state)state.settings=event.payload;
      theme();if(!busy)render();
    });
    void listen<string>('jack-error',e => { error = e.payload; showToast(error); render(); });
    void listen<boolean>('jack-visibility',e => { visible = e.payload; if (quick) document.documentElement.dataset.popupOpen=String(visible); if (!visible) cancelDrag?.(); if (visible) void refresh(); });
    void listen<QuickLayout>('jack-popup-layout',e=>{if(quick)applyQuickLayout(e.payload);});
    void listen('jack-popup-hidden',()=>{if(quick && !visible){quickOutputGroup=null;renderQuick();}});
    void invoke<boolean>('window_visible').then(value => { visible = value; if (quick) document.documentElement.dataset.popupOpen=String(value); });
    void getCurrentWindow().onFocusChanged(async e => { if (!e.payload) cancelDrag?.(); if(quick)return; visible = await invoke<boolean>('window_visible'); if (visible) void refresh(); });
    setInterval(() => void refresh(),1500);
  }
  document.addEventListener('visibilitychange',() => { if (document.hidden) cancelDrag?.(); else void refresh(); });
  matchMedia('(prefers-color-scheme: light)').addEventListener('change',theme);
  document.addEventListener('keydown',e => {
    if (native && (e.key === 'F12' || (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === 'i'))) {
      e.preventDefault();
      if (import.meta.env.DEV) void invoke('open_devtools').catch(reason => console.error(reason));
      return;
    }
    if (e.key !== 'Escape') return;
    if (cancelDrag) { e.preventDefault(); cancelDrag(); return; }
    if (quick) { e.preventDefault(); if (native) void invoke('hide_to_tray'); }
  });
}
