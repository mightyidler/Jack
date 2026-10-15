import { cssTimeMs } from './motion-time';

interface DragOptions {
  canStart: () => boolean;
  onStart: (cancel: () => void) => void;
  onFinish: () => void;
  onDrop: (channel: string) => Promise<void>;
}

// Pointer capture keeps the lifted row attached to the finger/mouse rather
// than relying on the browser's native drag image or a handle-only hit area.
export function bindAppDrag(row: HTMLElement, options: DragOptions) {
  row.addEventListener('pointerdown', event => {
    if (event.button !== 0 || !event.isPrimary || !options.canStart()) return;
    const origin = row.getBoundingClientRect();
    const panel = row.closest<HTMLElement>('#panel');
    if (!panel) return;
    const pointer = event.pointerId;
    let lifted = false;
    let ended = false;
    let ghost: HTMLElement | null = null;
    let target: HTMLElement | null = null;
    let frame = 0;
    let x = event.clientX;
    let y = event.clientY;
    const offsetX = x - origin.left;
    const offsetY = y - origin.top;
    const startX = x;
    const startY = y;
    const source = row.closest<HTMLElement>('.sonar-channel');
    const slot = document.createElement('div');
    slot.className = 'app-drop-slot t-resize';
    slot.setAttribute('aria-hidden','true');
    const listeners = new AbortController();
    const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
    const motionStyle=getComputedStyle(row);
    const duration=cssTimeMs(motionStyle.getPropertyValue('--app-motion-fast'),250);
    const easing=motionStyle.getPropertyValue('--app-motion-ease').trim() || 'cubic-bezier(0.22,1,0.36,1)';
    const releaseInput = () => {
      listeners.abort(); cancelAnimationFrame(frame);
      if (row.hasPointerCapture(pointer)) row.releasePointerCapture(pointer);
    };
    const cleanup = () => {
      releaseInput();
      target?.classList.remove('app-drop-target'); slot.remove();
      row.classList.remove('app-drag-source'); row.removeAttribute('aria-grabbed');
      document.body.classList.remove('app-dragging');
    };
    const settle = async (destination: DOMRect | null, commit: boolean) => {
      if (ended) return;
      ended = true;
      const channel = target?.dataset.channel;
      releaseInput();
      if (!commit) cleanup();
      if (ghost && destination) {
        const rect = ghost.getBoundingClientRect();
        const motion = ghost.animate([
          {transform:`translate3d(${rect.left}px,${rect.top}px,0) scale(1.025)`,opacity:1},
          {transform:`translate3d(${destination.left}px,${destination.top}px,0) scale(1)`,opacity:commit?1:0},
        ],{duration:reduced?0:duration,easing,fill:'forwards'});
        await motion.finished.catch(() => {});
      }
      try { if (commit && channel) await options.onDrop(channel); }
      finally { cleanup();ghost?.remove();options.onFinish(); }
    };
    const cancel = () => void settle(row.getBoundingClientRect(),false);
    const place = () => {
      if (!ghost) return;
      ghost.style.transform = `translate3d(${x-offsetX}px,${y-offsetY}px,0) scale(1.025)`;
      const hit = document.elementFromPoint(x,y)?.closest<HTMLElement>('.sonar-channel[data-channel]');
      const next = hit && hit !== source && hit.dataset.dropDisabled !== 'true' ? hit : null;
      if (!next) { target?.classList.remove('app-drop-target');target=null;slot.remove();return; }
      const list = next.querySelector<HTMLElement>('.sonar-app-list');
      if (!list) return;
      const before = [...list.querySelectorAll<HTMLElement>('.sonar-app:not(.app-drag-source)')].find(item => y < item.getBoundingClientRect().top + item.getBoundingClientRect().height/2) ?? null;
      if (target === next && slot.parentElement === list && slot.nextElementSibling === before) return;
      const moving = [...panel.querySelectorAll<HTMLElement>('.sonar-channel,.sonar-app:not(.app-drag-source)')];
      const positions = new Map(moving.map(item => [item,item.getBoundingClientRect()]));
      target?.classList.remove('app-drop-target'); target=next; target.classList.add('app-drop-target');
      list.insertBefore(slot,before);
      const nextPositions = new Map(moving.map(item => [item,item.getBoundingClientRect()]));
      if (!reduced) for (const item of moving) {
        const old = positions.get(item)!; const current = nextPositions.get(item)!;
        const parent = item.classList.contains('sonar-app') ? item.closest<HTMLElement>('.sonar-channel') : null;
        const parentDy = parent && positions.has(parent) ? positions.get(parent)!.top - nextPositions.get(parent)!.top : 0;
        const dy = old.top - current.top - parentDy;
        if (Math.abs(dy)>0.5) item.animate([{transform:`translateY(${dy}px)`},{transform:'translateY(0)'}],{duration,easing});
      }
    };
    const scroll = () => {
      if (ended || !lifted) return;
      const bounds = panel.getBoundingClientRect();
      if (x >= bounds.left && x <= bounds.right && y >= bounds.top && y <= bounds.bottom) {
        const dy = y < bounds.top+48 ? -Math.min(10,(bounds.top+48-y)/4) : y > bounds.bottom-48 ? Math.min(10,(y-bounds.bottom+48)/4) : 0;
        if (dy) { panel.scrollTop += dy; place(); }
      }
      frame = requestAnimationFrame(scroll);
    };
    row.setPointerCapture(pointer); event.preventDefault();
    const move = (next: PointerEvent) => {
      if (next.pointerId !== pointer || ended) return;
      x=next.clientX;y=next.clientY;
      if (!lifted && Math.hypot(x-startX,y-startY) < 6) return;
      if (!lifted) {
        lifted=true;row.classList.add('app-drag-source');row.setAttribute('aria-grabbed','true');document.body.classList.add('app-dragging');
        ghost=row.cloneNode(true) as HTMLElement;
        ghost.removeAttribute('id');ghost.removeAttribute('aria-grabbed');ghost.setAttribute('aria-hidden','true');ghost.classList.remove('app-drag-source');ghost.classList.add('app-drag-ghost');
        ghost.querySelectorAll('[id]').forEach(element => element.removeAttribute('id'));
        ghost.style.width=`${origin.width}px`;ghost.style.height=`${origin.height}px`;document.body.append(ghost);
        frame=requestAnimationFrame(scroll);
      }
      place();
    };
    row.addEventListener('pointermove',move,{signal:listeners.signal});
    row.addEventListener('pointerup',next => {
      if (next.pointerId !== pointer) return;
      if (!lifted) { ended=true;cleanup();options.onFinish();return; }
      const source = row.closest<HTMLElement>('.sonar-channel')?.dataset.channel;
      const commit=!!target && target.dataset.channel !== source;
      void settle(commit ? slot.getBoundingClientRect() : row.getBoundingClientRect(),commit);
    },{signal:listeners.signal});
    row.addEventListener('pointercancel',cancel,{signal:listeners.signal});
    row.addEventListener('lostpointercapture',() => { if (!ended) cancel(); },{signal:listeners.signal});
    window.addEventListener('blur',cancel,{signal:listeners.signal});
    // Reserve the row immediately, including the distance before lift-off.
    // A background refresh must not replace it and lose pointer capture while
    // the user is still beginning their very first drag.
    options.onStart(cancel);
  });
}
