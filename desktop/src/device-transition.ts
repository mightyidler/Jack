import { cssTimeMs } from './motion-time';
import { captureDeviceDrum, animateDeviceDrum, type DeviceDrumPaint } from './device-drum';

interface LayerPaint { opacity:number; filter:string; transform:string }
interface LabelLayer extends LayerPaint { text:string; device:string|null; color:string }
interface IconLayer extends LayerPaint { node:HTMLElement; asset:string; device:string|null }
interface DevicePaint {
  device:string|null;
  direction:number;
  swapRemaining:number|null;
  drum:DeviceDrumPaint|null;
  selected:boolean; statusDelay:number; statusFilter:string;
  label:string; layers:LabelLayer[]; color:string;
  icon:string|null; iconLayers:IconLayer[];
  surfaceOpacity:number; statusOpacity:number; contentOpacity:number|null;
  fill:{width:string;transform:string}|null;
}
const selector='.device-row[data-device-motion-key]';

export function captureDevicePaint(root:ParentNode|null):Map<string,DevicePaint> {
  const saved=new Map<string,DevicePaint>();
  root?.querySelectorAll<HTMLElement>(selector).forEach(row=>{
    const label=row.querySelector<HTMLElement>('.device-name');
    if(!label)return;
    const layers=[...label.querySelectorAll<HTMLElement>('.device-label-layer')];
    const icon=row.querySelector<HTMLElement>('.icon[data-icon-asset]');
    const iconLayers=icon?[...icon.querySelectorAll<HTMLElement>('.device-icon-layer')]:[];
    const content=row.querySelector<HTMLElement>('.volume-content');
    const surface=row.querySelector<HTMLElement>('.volume-surface-clip');
    const status=row.querySelector<HTMLElement>('.input-volume-status');
    const statusAnimation=status?.getAnimations().find(animation=>animation.id==='input-selection-status');
    const fill=row.querySelector<HTMLElement>('.volume-fill');
    const swap=row.getAnimations({subtree:true}).find(animation=>animation.id==='device-content-swap' && animation.playState!=='finished');
    saved.set(row.dataset.deviceMotionKey!,{
      device:row.dataset.endpoint??null,
      direction:row.dataset.deviceMotionDirection==='1'?1:-1,
      swapRemaining:swap?Math.max(0,Number(swap.effect?.getComputedTiming().endTime??0)-Number(swap.currentTime??0)):null,
      drum:captureDeviceDrum(row),
      selected:row.classList.contains('selected'),
      statusDelay:statusAnimation?Math.max(0,Number(statusAnimation.effect?.getTiming().delay??0)-Number(statusAnimation.currentTime??0)):0,
      statusFilter:status?getComputedStyle(status).filter:'none',
      label:label.dataset.deviceLabel??label.textContent??'',
      layers:(layers.length?layers:[label]).map(node=>{
        const style=getComputedStyle(node);
        return {text:node.textContent??'',device:node.dataset.deviceLayerId??row.dataset.endpoint??null,opacity:Number(style.opacity),filter:style.filter,transform:style.transform,color:style.color};
      }),
      color:getComputedStyle(row).color,
      icon:icon?.dataset.iconAsset??null,
      iconLayers:(iconLayers.length?iconLayers:icon?[icon]:[]).map(node=>{
        const style=getComputedStyle(node);
        return {node:node.cloneNode(false) as HTMLElement,asset:node.dataset.iconAsset??'',device:node.dataset.deviceLayerId??row.dataset.endpoint??null,opacity:Number(style.opacity),filter:style.filter,transform:style.transform};
      }),
      surfaceOpacity:surface?Number(getComputedStyle(surface).opacity):0,
      statusOpacity:status?Number(getComputedStyle(status).opacity):0,
      contentOpacity:content?Number(getComputedStyle(content).opacity):null,
      fill:fill?{width:fill.style.width,transform:fill.style.transform}:null,
    });
  });
  return saved;
}

export function animateDeviceChange(root:ParentNode, previous:ReadonlyMap<string,DevicePaint>, {slide=true}:{slide?:boolean}={}) {
  if(!previous.size || matchMedia('(prefers-reduced-motion: reduce)').matches)return;
  root.querySelectorAll<HTMLElement>(selector).forEach(row=>{
    const before=previous.get(row.dataset.deviceMotionKey!);
    const label=row.querySelector<HTMLElement>('.device-name');
    if(!label)return;
    const style=getComputedStyle(row);
    const textDuration=cssTimeMs(style.getPropertyValue('--device-text-duration'),250);
    const surfaceDuration=cssTimeMs(style.getPropertyValue('--device-surface-duration'),250);
    const easing=style.getPropertyValue('--device-change-ease').trim() || 'ease-in-out';
    const slideEasing=style.getPropertyValue('--device-slide-ease').trim() || easing;
    const surfaceEasing=style.getPropertyValue('--device-surface-ease').trim() || easing;
    const fade=(node:HTMLElement,from:number,to:number,duration:number,curve=easing)=>{
      if(duration===0 || Math.abs(from-to)<.001)return;
      node.animate([{opacity:from},{opacity:to}],{duration,easing:curve});
    };
    const distance=style.getPropertyValue('--device-change-distance').trim() || '100%';
    const blur=style.getPropertyValue('--device-change-blur').trim() || '2px';
    const text=label.textContent??'';
    const device=row.dataset.endpoint??null;
    const deviceChanged=!!before && before.device!==device;
    const icon=row.querySelector<HTMLElement>('.icon[data-icon-asset]');
    const continuing=!!before && !deviceChanged && before.label===text && before.icon===(icon?.dataset.iconAsset??null);
    const swapDuration=continuing && before.swapRemaining!==null?Math.min(textDuration,before.swapRemaining):textDuration;
    const swapStart=document.timeline.currentTime;
    const animateLayer=(node:HTMLElement,from:LayerPaint,to:LayerPaint,leaving:boolean)=>{
      const options:KeyframeAnimationOptions={duration:swapDuration,fill:leaving?'forwards':'none'};
      // Movement settles quickly; paint stays on an even cross-blur curve.
      // Both lanes, the icon, and the label share exactly the same start time.
      const animations=[
        node.animate([{transform:from.transform},{transform:to.transform}],{...options,easing:slide?slideEasing:easing}),
        node.animate([{opacity:from.opacity,filter:from.filter},{opacity:to.opacity,filter:to.filter}],{...options,easing}),
      ];
      animations.forEach(animation=>{
        animation.id='device-content-swap';
        if(typeof swapStart==='number')animation.startTime=swapStart;
      });
      return animations;
    };
    // Use the same ordered inventory as the picker, never endpoint IDs/names.
    // Moving to a lower item rolls up; moving to a higher item rolls down.
    const order:string[]=JSON.parse(row.dataset.deviceMotionOrder??'[]');
    const fromIndex=order.indexOf(before?.device??'');
    const toIndex=order.indexOf(device??'');
    const direction=deviceChanged && fromIndex>=0 && toIndex>=0 && fromIndex!==toIndex
      ? Math.sign(fromIndex-toIndex) : before?.direction??-1;
    row.dataset.deviceMotionDirection=String(direction);
    // Clip moving content at the full volume box, leaving its elastic painted
    // surface alone. The text/icon slots themselves must not crop the motion.
    row.style.setProperty('--device-motion-box-height',`${row.clientHeight}px`);
    row.style.setProperty('--device-motion-padding-left',style.paddingLeft);
    row.style.setProperty('--device-motion-padding-right',style.paddingRight);
    const drum=slide && !!row.querySelector('.device-drum');
    if(drum)animateDeviceDrum(row,before?.drum??null,before?.device??null);
    const swapLayers=(incoming:HTMLElement,same:LayerPaint|undefined,outgoing:{node:HTMLElement;layer:LayerPaint}[])=>{
      // Adjacent layers move together while opacity and blur interpolate across
      // the full duration. Blur ends at the configured value, never accumulates.
      // An interrupted render resumes from each layer's painted position.
      return [
        ...animateLayer(incoming,
          {opacity:same?.opacity??0,filter:same?.filter??`blur(${blur})`,transform:slide?(same?.transform??`translateY(calc(${distance} * ${-direction}))`):'none'},
          {opacity:1,filter:'blur(0px)',transform:'translateY(0)'},false),
        ...outgoing.flatMap(({node,layer})=>animateLayer(node,
          {...layer,transform:slide?layer.transform:'none'},
          {opacity:0,filter:`blur(${blur})`,transform:slide?`translateY(calc(${distance} * ${direction}))`:'none'},true)),
      ];
    };
    if(!drum && before && icon && textDuration>0 && (deviceChanged || before.icon!==icon.dataset.iconAsset || before.iconLayers.length>1)) {
      // Keep the slot fixed and use the label's exit/enter sequence, including the
      // currently painted layers when a render interrupts a device change.
      const incoming=icon.cloneNode(false) as HTMLElement;incoming.classList.add('device-icon-layer');
      incoming.dataset.deviceLayerId=device??'';
      const matches=(layer:IconLayer)=>layer.asset===icon.dataset.iconAsset && layer.device===device;
      const same=before.iconLayers.find(matches);
      const outgoing=before.iconLayers.filter(layer=>!matches(layer) && layer.opacity>.001);
      outgoing.forEach(layer=>{layer.node.classList.add('device-icon-layer');layer.node.dataset.deviceLayerId=layer.device??'';});
      icon.style.maskImage='none';icon.style.backgroundColor='transparent';
      icon.replaceChildren(incoming,...outgoing.map(layer=>layer.node));
      const animations=swapLayers(incoming,same,outgoing.map(layer=>({node:layer.node,layer})));
      void Promise.allSettled(animations.map(animation=>animation.finished)).then(()=>{
        if(icon.contains(incoming)){incoming.classList.remove('device-icon-layer');delete incoming.dataset.deviceLayerId;icon.replaceWith(incoming);}
      });
    }
    if(!drum && before && textDuration>0 && (deviceChanged || before.label!==text || before.layers.length>1)) {
      label.dataset.deviceLabel=text;
      const incoming=document.createElement('span');incoming.className='device-label-layer';incoming.textContent=text;
      incoming.dataset.deviceLayerId=device??'';
      const matches=(layer:LabelLayer)=>layer.text===text && layer.device===device;
      const same=before.layers.find(matches);
      const outgoing=before.layers.filter(layer=>!matches(layer) && layer.opacity>.001).map(layer=>{
        const node=document.createElement('span');node.className='device-label-layer device-label-outgoing';node.textContent=layer.text;
        node.dataset.deviceLayerId=layer.device??'';
        node.setAttribute('aria-hidden','true');node.style.color=layer.color;
        return {node,layer};
      });
      label.replaceChildren(incoming,...outgoing.map(item=>item.node));
      const animations=swapLayers(incoming,same,outgoing);
      void Promise.allSettled(animations.map(animation=>animation.finished)).then(()=>{
        if(label.contains(incoming))label.replaceChildren(document.createTextNode(text));
      });
    }
    if(before && before.color!==style.color && textDuration>0)row.animate([{color:before.color},{color:style.color}],{duration:textDuration,easing});
    const content=row.querySelector<HTMLElement>('.volume-content');
    if(content && before?.contentOpacity!==null && before?.contentOpacity!==undefined) {
      const opacity=Number(getComputedStyle(content).opacity);
      if(Math.abs(before.contentOpacity-opacity)>.001)fade(content,before.contentOpacity,opacity,textDuration);
    }
    const selected=row.classList.contains('selected');
    const surface=row.querySelector<HTMLElement>('.volume-surface-clip');
    const status=row.querySelector<HTMLElement>('.input-volume-status');
    if(!selected && before?.fill && before.surfaceOpacity>.001) {
      const fill=row.querySelector<HTMLElement>('.volume-fill');
      if(fill){fill.style.width=before.fill.width;fill.style.transform=before.fill.transform;}
    }
    const inputSelection=!!row.closest('[data-input-list]');
    if(surface && !inputSelection)fade(surface,before?.surfaceOpacity??0,selected?1:0,surfaceDuration,surfaceEasing);
    if(status && !inputSelection) {
      const duration=cssTimeMs(getComputedStyle(status).getPropertyValue('--volume-status-duration'),150);
      const from=before?.statusOpacity??0,to=selected?1:0;
      if(duration>0 && Math.abs(from-to)>.001) {
        const delay=selected && before && from<=.001?(!before.selected?duration:before.statusDelay):0;
        const blur=style.getPropertyValue('--device-selection-blur').trim() || '2px';
        const animation=status.animate([
          {opacity:from,filter:from>.001?before?.statusFilter??'none':`blur(${blur})`},
          {opacity:to,filter:selected?'blur(0px)':`blur(${blur})`},
        ],{duration,delay,easing,fill:'backwards'});
        animation.id='input-selection-status';
      }
    }
  });
}
