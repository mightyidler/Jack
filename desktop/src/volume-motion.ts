// Visual response is independent of the serialized Sonar volume writes.
// Standalone tracks own their elasticity; paired tracks use FluidControls.
import { rubberBand } from './rubber-band';
export class VolumeMotion {
  private position:number[];
  private target:number[];
  private velocity=[0,0,0];
  private frame=0;
  private time=0;
  private releaseTimer:ReturnType<typeof setTimeout>|undefined;
  private released:(()=>void)|null=null;
  private radiiReady=false;
  constructor(private fill:HTMLElement,private surface:HTMLElement,level:number,private onLevel:(level:number)=>void=()=>{},initial?:{level:number;velocity:number}) {
    this.position=[initial?.level??level,0,0]; this.target=[level,0,0];
    this.velocity[0]=initial?.velocity??0;
    fill.style.width='100%'; this.paint();
  }
  // Carry the currently painted value and speed across a view rebuild. Device
  // changes and interrupted transitions must continue from what is on screen.
  snapshot() { return {level:this.position[0],velocity:this.velocity[0]}; }
  dispose() {
    cancelAnimationFrame(this.frame);this.frame=0;
    clearTimeout(this.releaseTimer);this.releaseTimer=undefined;
    const done=this.released;this.released=null;done?.();
  }
  grab() {
    clearTimeout(this.releaseTimer); this.releaseTimer=undefined;
    const previous=this.released; this.released=null; previous?.();
    this.target[2]=1; this.start();
  }
  set(level:number,overrun=0) {
    this.target[0]=Math.max(0,Math.min(1,level));
    // Paired tracks pass zero overrun; avoid a forced layout during their fill
    // animation. Standalone tracks only measure when actually beyond an edge.
    const width=overrun?this.surface.closest<HTMLElement>('.device-row')?.offsetWidth||1:1;
    this.target[1]=Math.sign(overrun)*rubberBand(overrun/width,.025);
    this.start();
  }
  release(immediate=false) {
    this.target[1]=0;this.target[2]=0;
    if(immediate){this.finish();return Promise.resolve();}
    return new Promise<void>(resolve=>{
      this.released=resolve;
      this.releaseTimer=setTimeout(()=>this.finish(),380);
      this.start();
    });
  }
  private paint() {
    this.fill.style.transform=`scaleX(${Math.max(0,Math.min(1.015,this.position[0]))})`;
    this.onLevel(Math.max(0,Math.min(1,this.position[0])));
    const row=this.surface.closest<HTMLElement>('.device-row');
    // The shared controller owns actual bounds and radii. A second paint-only
    // scale here would distort the corners and break the gap between buttons.
    if(row?.closest('.sonar-device-control')?.querySelector('.sonar-output-open'))return;
    const grip=Math.max(0,Math.min(1.06,this.position[2]));
    const clip=this.surface.parentElement!;
    const pull=Math.max(-.025,Math.min(.025,this.position[1]));
    const stretch=.012*grip+Math.abs(pull);
    clip.style.setProperty('--volume-x-stretch',String(stretch));
    clip.style.setProperty('--volume-scale-y',String(1+.04*grip));
    clip.style.setProperty('--volume-left-bleed',String(Math.max(0,-pull)+.012*grip));
    clip.style.setProperty('--volume-right-bleed',String(Math.max(0,pull)+.012*grip));
    clip.style.setProperty('--volume-origin',pull<0?'100%':pull>0?'0%':'50%');
    if(row?.isConnected&&!this.radiiReady) {
      const style=getComputedStyle(row);
      [style.borderTopLeftRadius,style.borderTopRightRadius,style.borderBottomRightRadius,style.borderBottomLeftRadius].forEach((radius,index)=>{
        this.surface.style.setProperty(`--volume-base-${index}`,`${parseFloat(radius)||0}px`);
      });
      this.radiiReady=true;this.surface.dataset.geometry='true';
    }
  }
  private start() {
    if(matchMedia('(prefers-reduced-motion: reduce)').matches) {this.position=[...this.target];this.velocity.fill(0);this.paint();this.finish();return;}
    if(!this.frame){this.time=performance.now();this.frame=requestAnimationFrame(this.tick);}
  }
  private finish() {
    cancelAnimationFrame(this.frame);this.frame=0;
    clearTimeout(this.releaseTimer);this.releaseTimer=undefined;
    this.position=[...this.target];this.velocity.fill(0);this.paint();
    const done=this.released;this.released=null;done?.();
  }
  private tick=(time:number)=>{
    this.frame=0;
    const elapsed=Math.min((time-this.time)/1000,.032);this.time=time;
    const steps=Math.max(1,Math.ceil(elapsed*120));const dt=elapsed/steps;
    for(let step=0;step<steps;step++)for(let axis=0;axis<3;axis++){
      const stiffness=axis===0?600:460;const damping=axis===0?44:28;
      this.velocity[axis]+=(stiffness*(this.target[axis]-this.position[axis])-damping*this.velocity[axis])*dt;
      this.position[axis]+=this.velocity[axis]*dt;
    }
    this.paint();
    const settled=this.position.every((value,axis)=>Math.abs(value-this.target[axis])<.0002&&Math.abs(this.velocity[axis])<.002);
    if(settled)this.finish();else this.frame=requestAnimationFrame(this.tick);
  };
}
