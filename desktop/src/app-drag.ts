import { cssTimeMs } from './motion-time';
import { captureAppGroups, animateAppGroups, stopAppGroupMotion } from './app-group-motion';
import { bottomEdgeGroup, edgeScrollDelta } from './app-drag-geometry';

interface DragOptions {
  canStart: () => boolean;
  onStart: (cancel: () => void) => void;
  onFinish: () => void;
  onDrop: (channel: string, before: string|null) => Promise<void>;
}

// Pointer capture keeps the lifted row attached to the finger/mouse rather
// than relying on the browser's native drag image or a handle-only hit area.
export function bindAppDrag(row: HTMLElement, options: DragOptions) {
  row.addEventListener('pointerdown', event => {
    if (event.button !== 0 || !event.isPrimary || !options.canStart()) return;
    const origin = row.getBoundingClientRect();
    const panel = row.closest<HTMLElement>('#panel');
    if (!panel) return;
    const header=panel.closest('.window')?.querySelector<HTMLElement>('.header');
    const pointer = event.pointerId;
    let lifted = false;
    let ended = false;
    let ghost: HTMLElement | null = null;
    let liftMotion:Animation|null=null;
    let target: HTMLElement | null = null;
    let beforeKey:string|null=null;
    const sourceList=row.closest<HTMLElement>('.sonar-app-list');
    const source=row.closest<HTMLElement>('.sonar-channel');
    const touchedLists=new Set<HTMLElement>();
    // The floating row owns the item during a drag. Rebuild separators around
    // visible rows and the single slot, never around the hidden capture node.
    const syncDividers=(list:HTMLElement)=>{
      touchedLists.add(list);
      list.querySelectorAll('.sonar-app-divider').forEach(divider=>divider.remove());
      const visible=[...list.children].filter(node=>node===slot || node.classList.contains('sonar-app') && !node.classList.contains('app-drag-source'));
      visible.slice(1).forEach(node=>{
        const divider=document.createElement('div');divider.className='sonar-app-divider';
        list.insertBefore(divider,node);
      });
    };
    let frame = 0;
    let scrollTime=0;
    let x = event.clientX;
    let y = event.clientY;
    const offsetX = x - origin.left;
    const offsetY = y - origin.top;
    const startX = x;
    const startY = y;
    const slot = document.createElement('div');
    slot.className = 'app-drop-slot t-resize';
    slot.setAttribute('aria-hidden','true');
    const listeners = new AbortController();
    const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
    const motionStyle=getComputedStyle(row);
    const duration=cssTimeMs(motionStyle.getPropertyValue('--app-motion-fast'),250);
    const easing=motionStyle.getPropertyValue('--app-motion-ease').trim() || 'cubic-bezier(0.22,1,0.36,1)';
    const finalRect=(node:HTMLElement,scrollTop:number)=>{
      const current=panel.scrollTop;
      // Let the browser clamp to its exact (possibly fractional) final range.
      panel.scrollTop=scrollTop;
      const rect=node.getBoundingClientRect();
      panel.scrollTop=current;
      return rect;
    };
    const releaseInput = () => {
      listeners.abort(); cancelAnimationFrame(frame);
      if (row.hasPointerCapture(pointer)) row.releasePointerCapture(pointer);
    };
    const cleanup = (returning=false) => {
      releaseInput();
      const paint=captureAppGroups(panel);
      stopAppGroupMotion(panel);
      target?.classList.remove('app-drop-target'); slot.remove();
      row.classList.remove('app-drag-source','app-pressed'); row.removeAttribute('aria-grabbed');
      // Restore layout now, but keep its paint owned by the floating row until
      // it arrives. Revealing this row early paints a duplicate during return.
      row.classList.toggle('app-drag-returning',returning);
      for(const list of touchedLists)syncDividers(list);
      document.body.classList.remove('app-dragging');
      const destination=finalRect(row,paint.scrollTop);
      if(row.isConnected)animateAppGroups(panel,paint);
      return destination;
    };
    const settle = async (destination: DOMRect | null, commit: boolean) => {
      if (ended) return;
      ended = true;
      const channel = target?.dataset.channel;
      releaseInput();
      if (!commit) destination=cleanup(!!ghost);
      else {
        const paint=captureAppGroups(panel);
        stopAppGroupMotion(panel);
        destination=finalRect(slot,paint.scrollTop);
        animateAppGroups(panel,paint);
      }
      if (ghost && destination) {
        const paint=getComputedStyle(ghost);
        const from={transform:paint.transform,scale:paint.scale,boxShadow:paint.boxShadow,opacity:1};
        liftMotion?.cancel();
        const motion = ghost.animate([
          from,
          {transform:`translate3d(${destination.left}px,${destination.top}px,0)`,scale:1,opacity:1,boxShadow:'0 0 0 #0000',backgroundColor:'transparent'},
        ],{duration:reduced?0:commit?cssTimeMs(motionStyle.getPropertyValue('--app-group-duration'),350):duration,easing,fill:'forwards'});
        await motion.finished.catch(() => {});
      }
      try { if (commit && channel) await options.onDrop(channel,beforeKey); }
      finally {
        // Keep the single painted slot until render captures it and hands the
        // landed ghost to its real row without changing the reserved height.
        if(ghost)ghost.dataset.landing='true';
        options.onFinish();cleanup();ghost?.remove();
      }
    };
    const cancel = () => void settle(row.getBoundingClientRect(),false);
    const place = () => {
      if (!ghost) return;
      ghost.style.transform = `translate3d(${x-offsetX}px,${y-offsetY}px,0)`;
      let hit = document.elementFromPoint(x,y)?.closest<HTMLElement>('.sonar-channel[data-channel]');
      if(!hit) {
        const cards=[...panel.querySelectorAll<HTMLElement>('.sonar-channel[data-channel]')]
          .filter(card=>!card.closest('[data-output-group-exit]'));
        const index=bottomEdgeGroup(x,y,panel.getBoundingClientRect(),cards.map(card=>card.getBoundingClientRect()));
        hit=cards[index];
      }
      const next = hit && hit.dataset.dropDisabled !== 'true' ? hit : null;
      if (!next) {
        if(slot.parentElement) {
          const paint=captureAppGroups(panel);
          const oldList=slot.parentElement;
          target?.classList.remove('app-drop-target');target=null;slot.remove();syncDividers(oldList);
          animateAppGroups(panel,paint);
        }
        return;
      }
      const list = next.querySelector<HTMLElement>('.sonar-app-list');
      if (!list) return;
      if(target===next && slot.parentElement===list) {
        const gap=slot.getBoundingClientRect();
        if(y>=gap.top && y<=gap.bottom)return;
      }
      // Hit-test the list without the reserved gap or its FLIP transforms.
      // Otherwise opening a slot pushes row centers away from the pointer and
      // can force a bottom/middle drop back toward the first position.
      const divider=list.querySelector<HTMLElement>('.sonar-app-divider');
      const slotExtent=slot.parentElement===list?slot.offsetHeight+(divider?divider.offsetHeight+2*Number(getComputedStyle(list).rowGap.replace('px','')):0):0;
      const before = [...list.querySelectorAll<HTMLElement>('.sonar-app:not(.app-drag-source)')]
        .find(item => {
          const bounds=item.getBoundingClientRect();
          const shift=new DOMMatrixReadOnly(getComputedStyle(item).transform).m42;
          const reserved=slotExtent && !!(slot.compareDocumentPosition(item)&Node.DOCUMENT_POSITION_FOLLOWING)?slotExtent:0;
          return y<bounds.top-shift-reserved+bounds.height/2;
        }) ?? null;
      if (target === next && slot.parentElement === list && beforeKey===(before?.dataset.applicationKey??null)) return;
      const paint=captureAppGroups(panel);
      const oldList=slot.parentElement;
      target?.classList.remove('app-drop-target'); target=next; target.classList.add('app-drop-target');
      beforeKey=before?.dataset.applicationKey??null;
      list.insertBefore(slot,before);
      if(oldList && oldList!==list)syncDividers(oldList);
      syncDividers(list);
      animateAppGroups(panel,paint);
    };
    const scroll = (now:number) => {
      if (ended || !lifted) return;
      const bounds = panel.getBoundingClientRect();
      const contentTop=header?.getBoundingClientRect().bottom??bounds.top;
      const dy=edgeScrollDelta(x,y,bounds,now-scrollTime,contentTop);scrollTime=now;
      if(dy) {
        const previous=panel.scrollTop;
        panel.scrollTop+=dy;
        // At a scroll limit nothing moved. Re-running hit testing there used
        // to remove/reinsert the slot and restart the same layout every frame.
        if(Math.abs(panel.scrollTop-previous)>.001)place();
      }
      frame = requestAnimationFrame(scroll);
    };
    row.setPointerCapture(pointer); event.preventDefault();
    row.style.setProperty('--app-lift-origin',`${offsetX}px ${offsetY}px`);
    row.classList.add('app-pressed');
    const move = (next: PointerEvent) => {
      if (next.pointerId !== pointer || ended) return;
      x=next.clientX;y=next.clientY;
      if (!lifted && Math.hypot(x-startX,y-startY) < 6) return;
      if (!lifted) {
        const paint=captureAppGroups(panel);
        const pressed=getComputedStyle(row);
        const pickup={scale:pressed.scale,boxShadow:pressed.boxShadow};
        lifted=true;row.setAttribute('aria-grabbed','true');document.body.classList.add('app-dragging');
        ghost=row.cloneNode(true) as HTMLElement;
        ghost.removeAttribute('id');ghost.removeAttribute('aria-grabbed');ghost.setAttribute('aria-hidden','true');ghost.classList.remove('app-drag-source','app-pressed');ghost.classList.add('app-drag-ghost');
        ghost.querySelectorAll('[id]').forEach(element => element.removeAttribute('id'));
        ghost.style.width=`${origin.width}px`;ghost.style.height=`${origin.height}px`;
        ghost.style.transform=`translate3d(${x-offsetX}px,${y-offsetY}px,0)`;
        document.body.append(ghost);
        const held=getComputedStyle(ghost);
        liftMotion=ghost.animate([pickup,{scale:held.scale,boxShadow:held.boxShadow}],{duration:reduced?0:duration,easing});
        row.classList.add('app-drag-source');
        if(sourceList) {
          const nextRow=[...sourceList.querySelectorAll<HTMLElement>('.sonar-app')].find(item=>item!==row && !!(row.compareDocumentPosition(item)&Node.DOCUMENT_POSITION_FOLLOWING));
          beforeKey=nextRow?.dataset.applicationKey??null;
          sourceList.insertBefore(slot,row);syncDividers(sourceList);
          if(source?.dataset.channel && source.dataset.dropDisabled!=='true') {
            target=source;target.classList.add('app-drop-target');
          }
        }
        animateAppGroups(panel,paint);
        scrollTime=performance.now();
        frame=requestAnimationFrame(scroll);
      }
      place();
    };
    row.addEventListener('pointermove',move,{signal:listeners.signal});
    row.addEventListener('pointerup',next => {
      if (next.pointerId !== pointer) return;
      if (!lifted) {
        ended=true;releaseInput();row.classList.remove('app-pressed');
        // Keep a short click's painted row alive through its scale return.
        const returning=row.getAnimations().filter(animation=>animation instanceof CSSTransition && ['scale','box-shadow'].includes(animation.transitionProperty));
        void Promise.allSettled(returning.map(animation=>animation.finished)).then(()=>{cleanup();options.onFinish();});
        return;
      }
      const commit=!!target;
      void settle(commit ? slot.getBoundingClientRect() : row.getBoundingClientRect(),commit);
    },{signal:listeners.signal});
    row.addEventListener('pointercancel',cancel,{signal:listeners.signal});
    window.addEventListener('keydown',event=>{if(event.key==='Escape'){event.preventDefault();cancel();}},{signal:listeners.signal});
    row.addEventListener('lostpointercapture',() => { if (!ended) cancel(); },{signal:listeners.signal});
    window.addEventListener('blur',cancel,{signal:listeners.signal});
    // Reserve the row immediately, including the distance before lift-off.
    // A background refresh must not replace it and lose pointer capture while
    // the user is still beginning their very first drag.
    options.onStart(cancel);
  });
}
