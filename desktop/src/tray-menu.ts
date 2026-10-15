import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './tray-menu.css';

interface Preferences { theme:'system'|'light'|'dark'; language:string }
export async function mountTrayMenu(app:HTMLElement) {
  const root=document.documentElement;
  root.classList.add('tray-menu-view');root.dataset.menuOpen='false';
  let settings:Preferences={theme:'system',language:'ko'};
  let pending=false, visibilityRevision=0,frame=0;
  const menu=document.createElement('div');menu.className='tray-context-menu';menu.role='menu';
  menu.setAttribute('aria-label','Jack');
  const action=async(kind:string)=>{
    if(pending)return;
    pending=true;
    try{await invoke('tray_menu_action',{action:kind});}
    catch(error){console.error(error);}
    finally{pending=false;}
  };
  const open=document.createElement('button');open.type='button';open.role='menuitem';open.addEventListener('click',()=>void action('open'));
  const quit=document.createElement('button');quit.type='button';quit.role='menuitem';quit.className='tray-context-quit';quit.addEventListener('click',()=>void action('quit'));
  menu.append(open,quit);app.replaceChildren(menu);
  const theme=()=>{
    root.dataset.theme=settings.theme==='system'?(matchMedia('(prefers-color-scheme: light)').matches?'light':'dark'):settings.theme;
    root.dataset.fastlaneTheme=root.dataset.theme;
    root.lang=settings.language;
    open.textContent=settings.language==='en'?'Open Jack':'Jack 열기';
    quit.textContent=settings.language==='en'?'Quit Jack':'Jack 종료';
  };
  const visible=(show:boolean)=>{
    visibilityRevision++;cancelAnimationFrame(frame);
    if(!show){root.dataset.menuOpen='false';return;}
    // Keep the closed frame as the transition's computed starting point.
    void menu.offsetHeight;
    frame=requestAnimationFrame(()=>{root.dataset.menuOpen='true';open.focus({preventScroll:true});});
  };
  theme();
  await listen<boolean>('jack-tray-menu-visible',event=>visible(event.payload));
  await listen<Preferences>('jack-preferences',event=>{settings=event.payload;theme();});
  const initialRevision=visibilityRevision;
  void invoke<boolean>('window_visible').then(value=>{if(visibilityRevision===initialRevision)visible(value);});
  void invoke<{settings:Preferences}>('audio_action',{action:{kind:'read'}}).then(value=>{settings=value.settings;theme();}).catch(console.error);
  matchMedia('(prefers-color-scheme: light)').addEventListener('change',theme);
  document.addEventListener('pointerdown',event=>{if(!menu.contains(event.target as Node))void action('hide');});
  document.addEventListener('contextmenu',event=>event.preventDefault());
  document.addEventListener('keydown',event=>{
    if(event.key==='Escape'){event.preventDefault();void action('hide');}
    if(['ArrowUp','ArrowDown','Home','End'].includes(event.key)){
      event.preventDefault();
      (event.key==='Home'?open:event.key==='End'?quit:document.activeElement===open?quit:open).focus();
    }
    if(event.key==='F12'||(event.ctrlKey&&event.shiftKey&&event.key.toLowerCase()==='i')){
      event.preventDefault();
      if(import.meta.env.DEV)void invoke('open_devtools').catch(console.error);
    }
  });
}
