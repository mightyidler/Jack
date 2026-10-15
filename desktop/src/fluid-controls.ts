// One spring moves the painted boxes. Layout and pointer coordinates stay fixed;
// only the backgrounds scale, with inverse-scaled radii and unscaled content.
import { rubberBand } from './rubber-band';
import { cssTimeMs } from './motion-time';

const pairs=new WeakMap<HTMLElement,FluidControls>();
class FluidControls {
  // Volume grip, picker grip, edge pressure, volume follow.
  private position=[0,0,0,0];
  private velocity=[0,0,0,0];
  private target=[0,0,0,0];
  private edge=0;
  private follow=0;
  private frame=0;
  private time=0;
  private frequency=24;
  private immediate=false;
  private pressing=false;
  private canceled=false;
  private held=false;
  private width=300;
  private gap=4;
  private outer=12;
  private surface:HTMLElement;
  private pickerSurface:HTMLElement;
  private pickerHost:HTMLElement;
  private geometry:ResizeObserver;
  private waiting:(()=>void)[]=[];

  constructor(private host:HTMLElement,private button:HTMLButtonElement) {
    this.surface=host.querySelector<HTMLElement>('.volume-surface')!;
    this.pickerHost=button.parentElement!;
    this.pickerSurface=document.createElement('span');
    this.pickerSurface.className='sonar-picker-surface';
    this.pickerSurface.setAttribute('aria-hidden','true');
    this.pickerHost.prepend(this.pickerSurface);
    this.geometry=new ResizeObserver(entries=>{
      const width=entries[0]?.contentRect.width;
      if(width>48){
        const style=getComputedStyle(host);
        const gap=Number.parseFloat(style.columnGap);
        if(Number.isFinite(gap))this.gap=gap;
        this.width=width-this.gap-44;
        this.outer=Number.parseFloat(style.getPropertyValue('--pair-outer'))||12;
        this.paint();
      }
      if(!host.isConnected)this.geometry.disconnect();
    });
    this.geometry.observe(host);
    button.addEventListener('pointerdown',event=>{
      if(button.disabled||event.button!==0)return;
      this.canceled=false;this.pressing=true;button.setPointerCapture(event.pointerId);this.update();
    });
    button.addEventListener('pointerup',event=>{
      const bounds=this.pickerSurface.getBoundingClientRect();
      const inside=event.clientX>=bounds.left&&event.clientX<=bounds.right&&event.clientY>=bounds.top&&event.clientY<=bounds.bottom;
      this.canceled=!inside;
      this.pressing=false;this.update();
    });
    const cancel=()=>{this.canceled=true;this.pressing=false;this.update();};
    button.addEventListener('pointercancel',cancel);
    button.addEventListener('lostpointercapture',()=>{if(this.pressing)cancel();});
    button.addEventListener('keydown',event=>{
      if(!button.disabled&&['Enter',' '].includes(event.key)&&!event.repeat){this.canceled=false;this.pressing=true;this.update();}
    });
    button.addEventListener('keyup',event=>{
      if(['Enter',' '].includes(event.key)){this.pressing=false;this.update();}
    });
    button.addEventListener('blur',cancel);
    // Pointer capture keeps the spring responsive outside the painted box,
    // but a release outside must still cancel the button's ordinary click.
    button.addEventListener('click',event=>{
      if(this.canceled&&event.detail>0){event.preventDefault();event.stopImmediatePropagation();this.canceled=false;}
    },{capture:true});
    // Menu state affects the chevron, not the pressed shape of its button.
    new MutationObserver(()=>this.update()).observe(button,{attributes:true,attributeFilter:['disabled']});
  }

  volume(held:boolean) {
    this.held=held;
    if(!held){this.edge=0;this.follow=0;}
    this.update();
    if(held||!this.frame)return Promise.resolve();
    return new Promise<void>(resolve=>this.waiting.push(resolve));
  }

  overrun(distance:number,level:number) {
    this.edge=Math.sign(distance)*rubberBand(distance/this.width,.06);
    this.follow=(level-.5)*2;
    this.update(false);
  }

  private update(readTiming=true) {
    const picker=!this.button.disabled&&this.pressing;
    this.target=[this.held?1:0,picker?1:0,this.held?this.edge:0,this.held?this.follow:0];
    if(readTiming) {
      const style=getComputedStyle(this.host);
      this.outer=Number.parseFloat(style.getPropertyValue('--pair-outer'))||12;
      const duration=cssTimeMs(style.getPropertyValue(this.held||picker?'--pair-press-duration':'--pair-release-duration'),this.held||picker?150:250);
      this.immediate=duration===0;
      if(duration>0)this.frequency=8/(duration/1000);
    }
    if(this.immediate||matchMedia('(prefers-reduced-motion: reduce)').matches){this.finish();return;}
    if(!this.frame){this.host.dataset.pairMoving='true';this.time=performance.now();this.frame=requestAnimationFrame(this.tick);}
  }

  private paint() {
    const [volume,picker,edge,follow]=this.position;
    const v=Math.max(0,Math.min(1.025,volume)),p=Math.max(0,Math.min(1.025,picker));
    const pressure=edge*this.width;
    const travel=Math.max(-1,Math.min(1,pressure*.06+follow*.1));
    // A restrained 2px height lift with just 1.5% neighbor compression.
    const sx=1+3*v/this.width-.015*p+pressure*.03/this.width,sy=1+2*v/44-.015*p;
    const px=1+2*p/44-.015*v,py=px;
    const rowWidth=this.width*sx,pickerWidth=44*px;
    // Solve the two painted bounds as one centered assembly. The picker starts
    // exactly one resting gap after the row's visible right edge, EVERY frame.
    // No independent translation coefficients can open or collapse that gap.
    const left=(this.width+44-rowWidth-pickerWidth)/2+travel;
    const pickerShift=left+rowWidth-this.width+(pickerWidth-44)/2;
    this.surface.style.transform=`translate3d(${left}px,0,0) scale(${sx},${sy})`;
    this.surface.style.borderRadius=this.fixedRadii(sx,sy,false);
    this.pickerHost.style.transform=`translate3d(${pickerShift}px,0,0)`;
    this.pickerSurface.style.transform=`scale(${px},${py})`;
    this.pickerSurface.style.borderRadius=this.fixedRadii(px,py,true);
  }

  private fixedRadii(sx:number,sy:number,picker:boolean) {
    const corners=picker?[4,this.outer,this.outer,4]:[this.outer,4,4,this.outer];
    // Cancel the transform's effect on the resting corner values. There is no
    // radius morph: visible app/tray corners stay 12/4px and 16/4px throughout.
    return `${corners.map(r=>`${r/sx}px`).join(' ')} / ${corners.map(r=>`${r/sy}px`).join(' ')}`;
  }

  private finish() {
    cancelAnimationFrame(this.frame);this.frame=0;
    delete this.host.dataset.pairMoving;
    this.position=[...this.target];this.velocity.fill(0);
    if(this.host.isConnected)this.paint();
    this.waiting.splice(0).forEach(resolve=>resolve());
  }

  private tick=(time:number)=>{
    this.frame=0;
    if(!this.host.isConnected){this.geometry.disconnect();this.finish();return;}
    if(matchMedia('(prefers-reduced-motion: reduce)').matches){this.finish();return;}
    const elapsed=Math.max(0,Math.min((time-this.time)/1000,.064));this.time=time;
    // Exact critically damped integration gives the same response at 60/120Hz
    // without a rubbery rebound. Re-grabs retain position AND velocity.
    const decay=Math.exp(-this.frequency*elapsed);
    for(let axis=0;axis<4;axis++){
      const offset=this.position[axis]-this.target[axis];
      const momentum=this.velocity[axis]+this.frequency*offset;
      this.position[axis]=this.target[axis]+(offset+momentum*elapsed)*decay;
      this.velocity[axis]=(this.velocity[axis]-this.frequency*momentum*elapsed)*decay;
    }
    this.paint();
    if(this.position.every((value,axis)=>Math.abs(value-this.target[axis])<.0001&&Math.abs(this.velocity[axis])<.001))this.finish();
    else this.frame=requestAnimationFrame(this.tick);
  };
}

export function bindFluidControls(host:HTMLElement) {
  const button=host.querySelector<HTMLButtonElement>('.sonar-output-open');
  if(button)pairs.set(host,new FluidControls(host,button));
}
export function holdFluidVolume(row:HTMLElement,held:boolean) {
  const host=row.closest<HTMLElement>('.sonar-device-control');
  return (host&&pairs.get(host)?.volume(held))||Promise.resolve();
}
export function updateFluidVolume(row:HTMLElement,overrun:number,level:number) {
  const host=row.closest<HTMLElement>('.sonar-device-control');
  const pair=host&&pairs.get(host);
  if(!pair)return false;
  pair.overrun(overrun,level);return true;
}
