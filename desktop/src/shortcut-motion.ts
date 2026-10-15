// transitions-dev text-swap-soft: labels always share one normal-flow grid cell.
import { cssTimeMs } from './motion-time';
interface Swap { label:string; frame:number }
const running=new WeakMap<HTMLElement,Swap>();
const reduced=()=>matchMedia('(prefers-reduced-motion: reduce)').matches;
// Evaluate the transitions-polish CSS curves on the existing shared RAF clock.
function bezier(progress:number,x1:number,y1:number,x2:number,y2:number) {
  if(progress<=0||progress>=1)return progress;
  const sample=(t:number,a:number,b:number)=>3*(1-t)*(1-t)*t*a+3*(1-t)*t*t*b+t*t*t;
  let low=0,high=1;
  for(let step=0;step<16;step++){
    const middle=(low+high)/2;
    if(sample(middle,x1,x2)<progress)low=middle;else high=middle;
  }
  return sample((low+high)/2,y1,y2);
}
function text(label:string) {
  const node=document.createElement('span');node.className='shortcut-value';node.textContent=label;return node;
}
function naturalSize(button:HTMLElement,label:string) {
  const row=button.parentElement!;
  const gap=parseFloat(getComputedStyle(row).columnGap)||0;
  const left=row.querySelector<HTMLElement>('.shortcut-label')?.offsetWidth??0;
  const available=Math.max(1,row.clientWidth-left-gap);
  // Measure offscreen, never temporarily empty or resize the visible button.
  const probe=button.cloneNode(false) as HTMLElement;
  probe.removeAttribute('id');probe.setAttribute('aria-hidden','true');probe.inert=true;
  probe.style.cssText=`position:fixed;visibility:hidden;pointer-events:none;left:0;top:0;width:max-content;max-width:${available}px;height:auto;transform:none;transition:none;animation:none;flex:none;`;
  probe.append(text(label));document.body.append(probe);
  const style=getComputedStyle(probe);
  // Preserve fractional glyph advances. offsetWidth rounds them down and can
  // push the final glyph onto another line when the measured width is reused.
  const paddingX=(parseFloat(style.paddingLeft)||0)+(parseFloat(style.paddingRight)||0);
  const bounds=probe.getBoundingClientRect();
  const width=Math.min(available,Math.ceil(bounds.width));
  probe.style.width=`${width}px`;
  const size={width,height:probe.getBoundingClientRect().height,paddingX};
  probe.remove();return size;
}
export function setShortcutLabel(button:HTMLElement,label:string,recording:boolean,animate=true) {
  button.classList.toggle('recording',recording);
  const previous=running.get(button);
  if(previous?.label===label||(!previous&&button.textContent===label))return;
  const next=text(label);
  if(!animate||!button.isConnected||!button.children.length) {
    if(previous)cancelAnimationFrame(previous.frame);
    running.delete(button);button.replaceChildren(next);
    button.style.removeProperty('width');button.style.removeProperty('height');button.title=label;return;
  }
  const style=getComputedStyle(button);
  const current={width:parseFloat(style.width)||button.offsetWidth,height:parseFloat(style.height)||button.offsetHeight};
  const layers=[...button.querySelectorAll<HTMLElement>('.shortcut-value')].map(node=>{
    const paint=getComputedStyle(node);
    return {node,opacity:parseFloat(paint.opacity),filter:paint.filter,width:node.getBoundingClientRect().width};
  });
  if(previous)cancelAnimationFrame(previous.frame);
  const target=naturalSize(button,label);
  const old=layers.filter(layer=>layer.opacity>.001);
  // Freeze wrapping and paint, while keeping every layer in the SAME grid row.
  layers.forEach(layer=>{
    if(!old.includes(layer)){layer.node.remove();return;}
    layer.node.style.width=`${layer.width}px`;layer.node.style.maxWidth='none';
    layer.node.style.opacity=String(layer.opacity);layer.node.style.filter=layer.filter;
    layer.node.setAttribute('aria-hidden','true');
  });
  next.style.width=`${Math.max(0,target.width-target.paddingX)}px`;
  next.style.maxWidth='none';next.style.opacity='0';next.style.filter='blur(2px)';
  button.style.width=`${current.width}px`;button.style.height=`${current.height}px`;
  button.append(next);button.title=label;
  const state:Swap={label,frame:0};running.set(button,state);
  const tokens=getComputedStyle(button);
  const lessMotion=reduced();
  // Reduced motion keeps a brief fade instead of bypassing the entire swap.
  const textDuration=lessMotion?120:cssTimeMs(tokens.getPropertyValue('--shortcut-text-duration'),120);
  const resizeDuration=lessMotion?0:cssTimeMs(tokens.getPropertyValue('--shortcut-resize-duration'),160);
  const blur=tokens.getPropertyValue('--shortcut-text-blur').trim()||'2px';
  const blurAmount=lessMotion?0:parseFloat(blur)||2;
  const outgoing=old.map(layer=>({...layer,blur:parseFloat(layer.filter.match(/blur\(([\d.]+)px\)/)?.[1]??'0')}));
  const growing=target.width>=current.width&&target.height>=current.height;
  const resizeStart=growing?0:textDuration;
  const revealStart=growing?resizeDuration:textDuration;
  const end=Math.max(resizeStart+resizeDuration,revealStart+textDuration);
  const progress=(time:number,start:number,duration:number)=>duration===0?Number(time>=start):Math.min(1,Math.max(0,(time-start)/duration));
  const smooth=(value:number)=>bezier(value,.42,0,.58,1);
  const resizeEase=(value:number)=>bezier(value,.22,1,.36,1);
  let started:number|undefined;
  const paint=(timestamp:number)=>{
    if(running.get(button)!==state)return;
    if(!button.isConnected){running.delete(button);return;}
    // Start on the first presented frame, not before native IPC/layout work.
    started??=timestamp;
    const elapsed=timestamp-started;
    const exit=smooth(progress(elapsed,0,textDuration));
    const size=resizeEase(progress(elapsed,resizeStart,resizeDuration));
    const enter=smooth(progress(elapsed,revealStart,textDuration));
    // Both paint and geometry use this ONE clock. No finished/cancel promise
    // can skip a phase or restore the final text before it has been presented.
    for(const layer of outgoing){
      layer.node.style.opacity=String(layer.opacity*(1-exit));
      layer.node.style.filter=`blur(${layer.blur+(blurAmount-layer.blur)*exit}px)`;
      if(exit===1)layer.node.remove();
    }
    button.style.width=`${current.width+(target.width-current.width)*size}px`;
    button.style.height=`${current.height+(target.height-current.height)*size}px`;
    next.style.opacity=String(enter);next.style.filter=`blur(${blurAmount*(1-enter)}px)`;
    if(elapsed<end){state.frame=requestAnimationFrame(paint);return;}
    next.removeAttribute('style');
    button.style.removeProperty('width');button.style.removeProperty('height');running.delete(button);
  };
  state.frame=requestAnimationFrame(paint);
}
