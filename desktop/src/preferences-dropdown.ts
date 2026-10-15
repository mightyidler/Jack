import { mountScrollbar } from './scrollbars';

interface PreferenceOption { value:string; label:string; lang?:string }

// Bald's dropdown-wrap / trigger / menu / item structure and positioning.
// Keep Jack's supported preference values and existing persistence callbacks.
export function preferenceDropdown(id: string, label: string, options: PreferenceOption[], selected: string, change: (value:string) => void) {
  const wrap=document.createElement('div'); wrap.className='dropdown-wrap';
  const trigger=document.createElement('button'); trigger.type='button'; trigger.className='dropdown-trigger'; trigger.id=id;
  trigger.setAttribute('aria-label',label); trigger.setAttribute('aria-haspopup','menu'); trigger.setAttribute('aria-expanded','false');
  const value=document.createElement('span'); value.textContent=options.find(option=>option.value===selected)?.label??selected;
  const chevron=document.createElement('span'); chevron.className='preference-chevron'; chevron.setAttribute('aria-hidden','true');
  trigger.append(value,chevron); wrap.append(trigger);
  let current: HTMLDivElement | null=null;
  let openAtPointerDown=false;
  // Auto popovers light-dismiss before click. Capture the state before dismissal
  // so clicking the same trigger closes it instead of creating a fresh menu.
  trigger.addEventListener('pointerdown',() => {openAtPointerDown=!!current?.matches(':popover-open');},{capture:true});
  trigger.addEventListener('pointercancel',() => {openAtPointerDown=false;});
  const open = (event?: Event) => {
    if(event instanceof MouseEvent && event.detail>0 && openAtPointerDown) {
      openAtPointerDown=false; current?.hidePopover(); trigger.setAttribute('aria-expanded','false'); return;
    }
    openAtPointerDown=false;
    if (current?.matches(':popover-open')) { current.hidePopover(); trigger.setAttribute('aria-expanded','false'); return; }
    const menu=document.createElement('div'); menu.className='preference-menu'; menu.popover='auto'; menu.role='menu';
    menu.id=`${id}-menu`; menu.setAttribute('aria-label',label); trigger.setAttribute('aria-controls',menu.id);
    const viewport=document.createElement('div'); viewport.className='menu-viewport';
    for (const option of options) {
      const item=document.createElement('button'); item.type='button'; item.className=`dropdown-item${option.value===selected?' is-selected':''}`;
      item.role='menuitemradio'; item.setAttribute('aria-checked',String(option.value===selected)); item.dataset.value=option.value;
      const text=document.createElement('span'); text.textContent=option.label; if(option.lang) text.lang=option.lang;
      const check=document.createElement('span'); check.className='preference-check'; check.setAttribute('aria-hidden','true');
      item.append(text,check); viewport.append(item);
      item.addEventListener('click',() => { menu.hidePopover(); trigger.focus(); if (option.value!==selected) change(option.value); });
    }
    menu.append(viewport); document.querySelector('#app')!.append(menu); current=menu;
    const disposeScrollbar=mountScrollbar(viewport,menu,'menu');
    const appRect=document.querySelector('.window')!.getBoundingClientRect();
    const triggerRect=trigger.getBoundingClientRect();
    const padding=parseFloat(getComputedStyle(document.querySelector('#panel')!).paddingLeft);
    const minTop=document.querySelector('.header')!.getBoundingClientRect().bottom+padding;
    const maxBottom=appRect.bottom-padding;
    const belowTop=Math.max(triggerRect.bottom,minTop)+8;
    const aboveBottom=Math.min(triggerRect.top,maxBottom)-8;
    const spaceBelow=maxBottom-belowTop; const spaceAbove=aboveBottom-minTop;
    const opensUp=spaceBelow<options.length*42+8 && spaceAbove>spaceBelow;
    menu.classList.toggle('opens-up',opensUp);
    menu.style.right=`${innerWidth-appRect.right+20}px`;
    menu.style.top=opensUp?'auto':`${belowTop}px`;
    menu.style.bottom=opensUp?`${innerHeight-aboveBottom}px`:'auto';
    viewport.style.maxHeight=`${Math.max(0,(opensUp?spaceAbove:spaceBelow)-6)}px`;
    const events=new AbortController();
    document.querySelector('#panel')!.addEventListener('scroll',()=>menu.hidePopover(),{signal:events.signal,passive:true});
    window.addEventListener('resize',()=>menu.hidePopover(),{signal:events.signal});
    menu.addEventListener('toggle',() => {
      if (menu.matches(':popover-open')) return;
      if(current===menu) trigger.setAttribute('aria-expanded','false'); events.abort();
      setTimeout(()=>{disposeScrollbar();menu.remove();if(current===menu)current=null;},matchMedia('(prefers-reduced-motion: reduce)').matches?0:250);
    });
    menu.addEventListener('keydown',event => {
      if(!['ArrowDown','ArrowUp','Home','End'].includes(event.key))return;
      event.preventDefault(); const items=[...viewport.querySelectorAll<HTMLButtonElement>('button')];
      const index=items.indexOf(document.activeElement as HTMLButtonElement);
      const next=event.key==='Home'?0:event.key==='End'?items.length-1:(index+(event.key==='ArrowDown'?1:items.length-1))%items.length;
      items[next].focus();
    });
    menu.showPopover(); trigger.setAttribute('aria-expanded','true');
    (viewport.querySelector<HTMLButtonElement>('.is-selected')??viewport.querySelector('button'))?.focus({preventScroll:true});
  };
  trigger.addEventListener('click',open);
  trigger.addEventListener('keydown',event=>{if(event.key==='ArrowDown'||event.key==='ArrowUp'){event.preventDefault();open();}});
  return {wrap,trigger};
}
