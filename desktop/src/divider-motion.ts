import { cssTimeMs } from './motion-time';

const selector='.device-divider[data-divider-key]';

// Device selection rebuilds the list. Carry its painted opacity across that
// replacement so a CSS state change can still fade, including rapid reversals.
export function captureDividerOpacity(root:ParentNode|null):Map<string,number> {
  const opacity=new Map<string,number>();
  root?.querySelectorAll<HTMLElement>(selector).forEach(divider=>{
    opacity.set(divider.dataset.dividerKey!,Number(getComputedStyle(divider).opacity));
  });
  return opacity;
}

export function animateDividerOpacity(root:ParentNode, previous:ReadonlyMap<string,number>) {
  if(!previous.size || matchMedia('(prefers-reduced-motion: reduce)').matches)return;
  root.querySelectorAll<HTMLElement>(selector).forEach(divider=>{
    const from=previous.get(divider.dataset.dividerKey!);
    const to=divider.classList.contains('hidden')?0:1;
    if(from===undefined || Math.abs(from-to)<.001)return;
    const style=getComputedStyle(divider);
    const duration=cssTimeMs(style.getPropertyValue('--divider-fade-duration'),250);
    if(duration===0)return;
    divider.animate([{opacity:from},{opacity:to}],{
      duration,easing:style.getPropertyValue('--divider-fade-ease').trim() || 'ease-in-out',
    });
  });
}
