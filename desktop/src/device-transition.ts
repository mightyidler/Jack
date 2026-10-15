import { cssTimeMs } from './motion-time';

interface LabelLayer { text:string; device:string|null; opacity:number; filter:string; color:string }
interface IconLayer { node:HTMLElement; asset:string; device:string|null; opacity:number; filter:string }
interface DevicePaint {
  device:string|null;
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
    saved.set(row.dataset.deviceMotionKey!,{
      device:row.dataset.endpoint??null,
      selected:row.classList.contains('selected'),
      statusDelay:statusAnimation?Math.max(0,Number(statusAnimation.effect?.getTiming().delay??0)-Number(statusAnimation.currentTime??0)):0,
      statusFilter:status?getComputedStyle(status).filter:'none',
      label:label.dataset.deviceLabel??label.textContent??'',
      layers:(layers.length?layers:[label]).map(node=>{
        const style=getComputedStyle(node);
        return {text:node.textContent??'',device:node.dataset.deviceLayerId??row.dataset.endpoint??null,opacity:Number(style.opacity),filter:style.filter,color:style.color};
      }),
      color:getComputedStyle(row).color,
      icon:icon?.dataset.iconAsset??null,
      iconLayers:(iconLayers.length?iconLayers:icon?[icon]:[]).map(node=>{
        const style=getComputedStyle(node);
        return {node:node.cloneNode(false) as HTMLElement,asset:node.dataset.iconAsset??'',device:node.dataset.deviceLayerId??row.dataset.endpoint??null,opacity:Number(style.opacity),filter:style.filter};
      }),
      surfaceOpacity:surface?Number(getComputedStyle(surface).opacity):0,
      statusOpacity:status?Number(getComputedStyle(status).opacity):0,
      contentOpacity:content?Number(getComputedStyle(content).opacity):null,
      fill:fill?{width:fill.style.width,transform:fill.style.transform}:null,
    });
  });
  return saved;
}

export function animateDeviceChange(root:ParentNode, previous:ReadonlyMap<string,DevicePaint>) {
  if(!previous.size || matchMedia('(prefers-reduced-motion: reduce)').matches)return;
  root.querySelectorAll<HTMLElement>(selector).forEach(row=>{
    const before=previous.get(row.dataset.deviceMotionKey!);
    const label=row.querySelector<HTMLElement>('.device-name');
    if(!label)return;
    const style=getComputedStyle(row);
    const textDuration=cssTimeMs(style.getPropertyValue('--device-text-duration'),150);
    const surfaceDuration=cssTimeMs(style.getPropertyValue('--device-surface-duration'),250);
    const easing=style.getPropertyValue('--device-change-ease').trim() || 'ease-in-out';
    const surfaceEasing=style.getPropertyValue('--device-surface-ease').trim() || easing;
    const fade=(node:HTMLElement,from:number,to:number,duration:number,curve=easing)=>{
      if(duration===0 || Math.abs(from-to)<.001)return;
      node.animate([{opacity:from},{opacity:to}],{duration,easing:curve});
    };
    const swapLayers=(incoming:HTMLElement,same:{opacity:number;filter:string}|undefined,outgoing:{node:HTMLElement;layer:{opacity:number;filter:string}}[])=>{
      // Finish the exit before revealing the replacement. A shared midpoint
      // keeps text and icon silhouettes from being superimposed.
      const options={duration:textDuration*(outgoing.length?2:1),easing:'linear'};
      return [incoming.animate([
        {opacity:same?.opacity??0,filter:same?.filter??'blur(2px)',offset:0,easing},
        ...(outgoing.length?[{opacity:0,filter:'blur(2px)',offset:.5,easing}]:[]),
        {opacity:1,filter:'blur(0px)',offset:1},
      ],options),...outgoing.map(({node,layer})=>node.animate([
        {opacity:layer.opacity,filter:layer.filter,offset:0,easing},
        {opacity:0,filter:'blur(2px)',offset:.5},
        {opacity:0,filter:'blur(2px)',offset:1},
      ],{...options,fill:'forwards'}))];
    };
    const text=label.textContent??'';
    const device=row.dataset.endpoint??null;
    const deviceChanged=!!before && before.device!==device;
    const icon=row.querySelector<HTMLElement>('.icon[data-icon-asset]');
    if(before && icon && textDuration>0 && (deviceChanged || before.icon!==icon.dataset.iconAsset || before.iconLayers.length>1)) {
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
    if(before && textDuration>0 && (deviceChanged || before.label!==text || before.layers.length>1)) {
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
    if(surface)fade(surface,before?.surfaceOpacity??0,selected?1:0,surfaceDuration,surfaceEasing);
    if(status) {
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
