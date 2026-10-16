import { cssTimeMs } from './motion-time';

interface DrumItem { id:string; label:string; icon:string }
export interface DeviceDrumPaint {
  order:string[];
  position:number;
  velocity:number;
  to:number;
  duration:number;
}
interface DrumMotion { snapshot:()=>DeviceDrumPaint; stop:()=>void }
const motions=new WeakMap<HTMLElement,DrumMotion>();

export function captureDeviceDrum(row:HTMLElement):DeviceDrumPaint|null {
  return motions.get(row)?.snapshot()??null;
}

function rebase(paint:DeviceDrumPaint,order:string[]):number|null {
  if(paint.order.length===order.length && paint.order.every((id,index)=>id===order[index]))return paint.position;
  const lower=Math.floor(paint.position),upper=Math.ceil(paint.position);
  const a=order.indexOf(paint.order[lower]),b=order.indexOf(paint.order[upper]);
  if(a>=0 && b>=0)return a+(b-a)*(paint.position-lower);
  return a>=0?a:b>=0?b:null;
}

/** Visual inventory only. Intermediate faces never invoke audio actions. */
export function animateDeviceDrum(row:HTMLElement,previous:DeviceDrumPaint|null,previousDevice:string|null) {
  const slot=row.querySelector<HTMLElement>('.device-drum');
  const targetFace=slot?.querySelector<HTMLElement>('.device-drum-face');
  if(!slot || !targetFace)return;
  const items:DrumItem[]=JSON.parse(row.dataset.deviceDrumItems??'[]');
  const order=items.map(item=>item.id);
  const target=order.indexOf(row.dataset.endpoint??'');
  const previousIndex=previousDevice?order.indexOf(previousDevice):-1;
  const rebased=previous?rebase(previous,order):null;
  const position=rebased??previousIndex;
  if(target<0 || (rebased===null && previousIndex<0))return;
  const sameOrder=!!previous && previous.order.length===order.length && previous.order.every((id,index)=>id===order[index]);
  const initialVelocity=sameOrder?previous!.velocity:0;
  if(Math.abs(target-position)<.0001 && Math.abs(initialVelocity)<.00002)return;
  const style=getComputedStyle(row);
  const reduced=matchMedia('(prefers-reduced-motion: reduce)');
  // Explicit zero timing must also settle an interrupted rotation immediately.
  if(cssTimeMs(style.getPropertyValue('--device-text-duration'),250)===0
    || cssTimeMs(style.getPropertyValue('--device-drum-one-duration'),250)===0)return;
  const durationFor=(steps:number)=>{
    const one=cssTimeMs(style.getPropertyValue('--device-drum-one-duration'),250);
    if(one===0)return 0;
    if(steps<=1)return one;
    if(steps<=2)return cssTimeMs(style.getPropertyValue('--device-drum-two-duration'),360);
    const many=cssTimeMs(style.getPropertyValue('--device-drum-many-duration'),450);
    if(many===0)return 0;
    const extra=cssTimeMs(style.getPropertyValue('--device-drum-extra-duration'),40);
    const maximum=cssTimeMs(style.getPropertyValue('--device-drum-max-duration'),650);
    return Math.min(maximum,many+Math.max(0,steps-3)*extra);
  };
  const continuing=sameOrder && previous!.to===target;
  const duration=continuing?previous!.duration:durationFor(Math.ceil(Math.abs(target-position)));
  if(reduced.matches || duration===0)return;
  const response=Math.max(1,Number(style.getPropertyValue('--device-drum-response'))||10);
  const frequency=response/duration;
  const tilt=Math.min(75,Math.max(10,Number(style.getPropertyValue('--device-drum-angle'))||42));
  const maxBlur=Math.max(0,Number(style.getPropertyValue('--device-drum-blur'))||0);
  const height=slot.clientHeight;
  if(height<=0)return;
  // Tangent faces share an edge on one cylinder, instead of independently
  // translating/tilting flat labels through the viewport.
  const radius=(height/2)/Math.tan(tilt*Math.PI/360);
  const iconTemplate=targetFace.querySelector<HTMLElement>('.icon')!;
  // Keep the selected label first for accessible name/report lookups. CSS's
  // shared 3D context resolves the actual front/back order of the faces.
  targetFace.dataset.deviceDrumId=items[target].id;
  const faces=items.map((item,index)=>{
    if(index===target)return targetFace;
    const face=document.createElement('span');face.className='device-drum-face';
    face.dataset.deviceDrumId=item.id;face.setAttribute('aria-hidden','true');
    const icon=iconTemplate.cloneNode(false) as HTMLElement;
    icon.dataset.iconAsset=item.icon;icon.style.maskImage=`url("${encodeURI(item.icon)}")`;
    const label=document.createElement('span');label.className='device-name';label.textContent=item.label;
    face.append(icon,label);return face;
  });
  const rotor=document.createElement('span');rotor.className='device-drum-rotor';
  faces.forEach((face,index)=>{
    face.style.transform=`rotateX(${-index*tilt}deg) translateZ(${radius}px)`;
  });
  rotor.append(targetFace,...faces.filter(face=>face!==targetFace));
  slot.replaceChildren(rotor);
  slot.dataset.spinning='true';
  let frame=0,paintedPosition=position,velocity=initialVelocity;
  let lastFrame=performance.now();
  const paint=(current:number)=>{
    paintedPosition=current;
    // One shared horizontal axis drives every face. Cancelling the cylinder's
    // radius places the selected face exactly back on the normal text plane.
    rotor.style.transform=`translateZ(${-radius}px) rotateX(${(current*tilt).toFixed(5)}deg)`;
    faces.forEach((face,index)=>{
      const distance=index-current;
      const visible=Math.abs(distance)<1.2;
      if(face.dataset.visible!==String(visible)) {
        face.dataset.visible=String(visible);
        face.style.visibility=visible?'visible':'hidden';
      }
      if(!visible)return;
      const edge=Math.min(1,Math.abs(distance));
      const facing=Math.max(0,Math.cos(distance*tilt*Math.PI/180));
      const focus=edge*edge*(3-2*edge);
      // A clear front face with smooth optical falloff into the cylinder. The
      // squared blur preserves legibility near center, while staying within 2px.
      const opacity=(1-focus).toFixed(4);
      const blur=maxBlur*edge*edge;
      const filter=`blur(${blur<.02?'0':blur.toFixed(3)}px) brightness(${(.9+.1*facing).toFixed(3)})`;
      if(face.style.opacity!==opacity)face.style.opacity=opacity;
      if(face.style.filter!==filter)face.style.filter=filter;
    });
  };
  const motion:DrumMotion={
    snapshot:()=>({order,position:paintedPosition,velocity,to:target,duration}),
    stop:()=>cancelAnimationFrame(frame),
  };
  motions.get(row)?.stop();motions.set(row,motion);
  const finish=()=>{
    cancelAnimationFrame(frame);
    if(motions.get(row)!==motion)return;
    motions.delete(row);
    targetFace.removeAttribute('style');
    delete targetFace.dataset.visible;
    slot.replaceChildren(targetFace);delete slot.dataset.spinning;
  };
  const tick=(now:number)=>{
    if(!row.isConnected){cancelAnimationFrame(frame);motions.delete(row);return;}
    if(reduced.matches){finish();return;}
    const elapsed=Math.max(0,now-lastFrame);lastFrame=now;
    // Exact critical damping: independent of frame rate, with position AND
    // velocity retained when the target changes or a render replaces the DOM.
    const displacement=paintedPosition-target;
    const momentum=velocity+frequency*displacement;
    const decay=Math.exp(-frequency*elapsed);
    const next=target+(displacement+momentum*elapsed)*decay;
    velocity=(velocity-frequency*momentum*elapsed)*decay;
    paint(next);
    if(Math.abs(next-target)<.0006 && Math.abs(velocity)<.00002) {
      velocity=0;paint(target);
      // Present the exact front plane once before retiring the rotating scene.
      frame=requestAnimationFrame(finish);
    } else frame=requestAnimationFrame(tick);
  };
  paint(position);
  frame=requestAnimationFrame(tick);
}
