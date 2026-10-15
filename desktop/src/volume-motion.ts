// The hit area and track stay fixed. Only the clipped fill follows the volume;
// paired output controls own their separate, bounded press response.
import { cssTimeMs } from './motion-time';

export class VolumeMotion {
  private position:number;
  private target:number;
  private velocity=0;
  private held=false;
  private frame=0;
  private time=0;
  private releaseTimer:ReturnType<typeof setTimeout>|undefined;
  private released:(()=>void)|null=null;

  constructor(private fill:HTMLElement,level:number,private onLevel:(level:number)=>void=()=>{},initial?:{level:number;velocity:number}) {
    this.position=initial?.level??level;this.target=level;
    this.velocity=initial?.velocity??0;
    fill.style.width='100%';this.paint();
  }
  snapshot() { return {level:this.position,velocity:this.velocity}; }
  dispose() {
    cancelAnimationFrame(this.frame);this.frame=0;
    clearTimeout(this.releaseTimer);this.releaseTimer=undefined;
    const done=this.released;this.released=null;done?.();
  }
  grab() {
    this.dispose();this.held=true;
  }
  set(level:number) {
    this.target=Math.max(0,Math.min(1,level));
    if(this.held) {
      // Pointer coordinates already come from the resting row bounds. Following
      // them directly prevents a held, stationary pointer from driving a spring.
      this.position=this.target;this.velocity=0;this.paint();return;
    }
    this.start();
  }
  release(immediate=false) {
    this.held=false;
    if(immediate || Math.abs(this.position-this.target)<.0002 && Math.abs(this.velocity)<.002) {
      this.finish();return Promise.resolve();
    }
    return new Promise<void>(resolve=>{
      this.released=resolve;
      const duration=cssTimeMs(getComputedStyle(this.fill).getPropertyValue('--volume-settle-time'),380);
      this.releaseTimer=setTimeout(()=>this.finish(),duration);
      if(duration===0)this.finish();else this.start();
    });
  }
  private paint() {
    const level=Math.max(0,Math.min(1,this.position));
    this.fill.style.transform=`scaleX(${level})`;this.onLevel(level);
  }
  private start() {
    if(matchMedia('(prefers-reduced-motion: reduce)').matches){this.finish();return;}
    if(!this.frame){this.time=performance.now();this.frame=requestAnimationFrame(this.tick);}
  }
  private finish() {
    cancelAnimationFrame(this.frame);this.frame=0;
    clearTimeout(this.releaseTimer);this.releaseTimer=undefined;
    this.position=this.target;this.velocity=0;this.paint();
    const done=this.released;this.released=null;done?.();
  }
  private tick=(time:number)=>{
    this.frame=0;
    const elapsed=Math.max(0,Math.min((time-this.time)/1000,.064));this.time=time;
    // Exact critically damped integration also settles cleanly after a stalled
    // frame. It has no horizontal stretch or underdamped grip oscillation.
    const frequency=26,decay=Math.exp(-frequency*elapsed);
    const offset=this.position-this.target;
    const momentum=this.velocity+frequency*offset;
    this.position=this.target+(offset+momentum*elapsed)*decay;
    this.velocity=(this.velocity-frequency*momentum*elapsed)*decay;
    this.paint();
    if(Math.abs(this.position-this.target)<.0002 && Math.abs(this.velocity)<.002)this.finish();
    else this.frame=requestAnimationFrame(this.tick);
  };
}
