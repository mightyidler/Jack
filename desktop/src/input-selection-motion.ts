import { cssTimeMs } from './motion-time';

interface StatusPaint { opacity:number; transform:string; stroke:string; remaining:number|null }
interface SelectionPaint {
  device:string;
  left:number; top:number; width:number; height:number; level:number;
  remaining:number|null;
  statuses:Map<string,StatusPaint>;
}
const lists='[data-input-list]';

// Coordinates belong to the list, so scrolling or the tray's moving anchor
// cannot turn a selection change into a jump in screen coordinates.
export function captureInputSelection(root:ParentNode|null) {
  const saved=new Map<string,SelectionPaint>();
  root?.querySelectorAll<HTMLElement>(lists).forEach(list=>{
    const selected=list.querySelector<HTMLElement>('.device-row.selected');
    if(!selected)return;
    const layer=list.querySelector<HTMLElement>('.input-selection-surface');
    const surface=layer??selected;
    const bounds=surface.getBoundingClientRect(),origin=list.getBoundingClientRect();
    const scale=origin.width/parseFloat(getComputedStyle(list).width);
    const fill=surface.querySelector<HTMLElement>('.volume-fill');
    const motion=layer?.getAnimations().find(animation=>animation.id==='input-selection-move');
    const statuses=new Map<string,StatusPaint>();
    list.querySelectorAll<HTMLElement>('.device-row').forEach(row=>{
      const status=row.querySelector<HTMLElement>('.input-volume-status');
      const check=row.querySelector<SVGSVGElement>('.input-selection-check');
      const path=check?.querySelector('path');
      const draw=path?.getAnimations().find(animation=>animation.id==='input-selection-stroke');
      if(status && check && path)statuses.set(row.dataset.endpoint!,{
        opacity:Number(getComputedStyle(status).opacity),
        transform:getComputedStyle(check).transform,
        stroke:getComputedStyle(path).strokeDashoffset,
        remaining:draw?Math.max(0,Number(draw.effect?.getComputedTiming().endTime??0)-Number(draw.currentTime??0)):null,
      });
    });
    saved.set(list.dataset.inputList!,{
      device:selected.dataset.endpoint!,
      left:(bounds.left-origin.left)/scale,top:(bounds.top-origin.top)/scale,
      width:bounds.width/scale,height:bounds.height/scale,
      level:fill?new DOMMatrix(getComputedStyle(fill).transform).a:0,
      remaining:motion?Math.max(0,Number(motion.effect?.getComputedTiming().endTime??0)-Number(motion.currentTime??0)):null,
      statuses,
    });
  });
  return saved;
}

export function animateInputSelection(root:ParentNode,previous:ReadonlyMap<string,SelectionPaint>) {
  if(!previous.size || matchMedia('(prefers-reduced-motion: reduce)').matches)return;
  root.querySelectorAll<HTMLElement>(lists).forEach(list=>{
    const before=previous.get(list.dataset.inputList!);
    const selected=list.querySelector<HTMLElement>('.device-row.selected');
    if(!before || !selected)return;
    const tokens=getComputedStyle(list);
    const duration=cssTimeMs(tokens.getPropertyValue('--input-selection-duration'),250);
    const checkDuration=cssTimeMs(tokens.getPropertyValue('--input-check-duration'),250);
    const fadeDuration=cssTimeMs(tokens.getPropertyValue('--input-status-duration'),150);
    const easing=tokens.getPropertyValue('--ease-smooth-out').trim() || 'cubic-bezier(.22,1,.36,1)';
    const changed=before.device!==selected.dataset.endpoint;
    const start=document.timeline.currentTime;
    const animate=(node:Element,frames:Keyframe[],time:number,id:string)=>{
      if(time===0)return null;
      const motion=node.animate(frames,{duration:time,easing});
      motion.id=id;
      if(typeof start==='number')motion.startTime=start;
      return motion;
    };
    list.querySelectorAll<HTMLElement>('.device-row').forEach(row=>{
      const paint=before.statuses.get(row.dataset.endpoint!);
      const status=row.querySelector<HTMLElement>('.input-volume-status');
      const check=row.querySelector<SVGSVGElement>('.input-selection-check');
      const path=check?.querySelector('path');
      if(!paint || !status || !check || !path)return;
      const active=row===selected;
      const opacity=active?1:0;
      if(Math.abs(paint.opacity-opacity)>.001)animate(status,[{opacity:paint.opacity},{opacity}],fadeDuration,'input-selection-status');
      // Draw only a newly selected check. Interrupted renders carry its stroke
      // and scale forward instead of starting the confirmation again.
      if(active && (paint.opacity<.999 || parseFloat(paint.stroke)>.001)) {
        const remaining=changed?checkDuration:Math.min(checkDuration,paint.remaining??checkDuration);
        animate(check,[{transform:paint.opacity<.001?'scale(.8)':paint.transform},{transform:'scale(1)'}],remaining,'input-selection-check');
        animate(path,[{strokeDashoffset:paint.opacity<.001?'1':paint.stroke},{strokeDashoffset:'0'}],remaining,'input-selection-stroke');
      }
    });
    if(duration===0 || (!changed && !before.remaining))return;
    const rowBounds=selected.getBoundingClientRect(),listBounds=list.getBoundingClientRect();
    const scale=listBounds.width/parseFloat(tokens.width);
    const left=(rowBounds.left-listBounds.left)/scale,top=(rowBounds.top-listBounds.top)/scale;
    const width=rowBounds.width/scale,height=rowBounds.height/scale;
    const layer=document.createElement('span');layer.className='input-selection-surface';
    layer.setAttribute('aria-hidden','true');
    layer.style.left=`${left}px`;layer.style.top=`${top}px`;
    layer.style.width=`${width}px`;layer.style.height=`${height}px`;
    const fill=document.createElement('span');fill.className='volume-fill';
    const liveFill=selected.querySelector<HTMLElement>('.volume-fill');
    const level=liveFill?new DOMMatrix(getComputedStyle(liveFill).transform).a:0;
    fill.style.width='100%';fill.style.transform=`scaleX(${level})`;layer.append(fill);
    list.dataset.selectionMoving='true';list.append(layer);
    const remaining=changed?duration:Math.min(duration,before.remaining??duration);
    const motion=animate(layer,[
      {transform:`translate3d(${before.left-left}px,${before.top-top}px,0) scale(${before.width/width},${before.height/height})`},
      {transform:'translate3d(0,0,0) scale(1,1)'},
    ],remaining,'input-selection-move');
    const fillMotion=animate(fill,[{transform:`scaleX(${before.level})`},{transform:`scaleX(${level})`}],remaining,'input-selection-level');
    const events=new AbortController();
    const cleanup=()=>{
      events.abort();motion?.cancel();fillMotion?.cancel();layer.remove();
      delete list.dataset.selectionMoving;
    };
    // Direct volume manipulation takes ownership immediately.
    selected.addEventListener('pointerdown',cleanup,{capture:true,once:true,signal:events.signal});
    selected.addEventListener('keydown',event=>{
      if(['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End'].includes(event.key))cleanup();
    },{capture:true,signal:events.signal});
    void motion?.finished.then(cleanup,cleanup);
  });
}
