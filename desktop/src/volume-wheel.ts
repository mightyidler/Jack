// Accumulate fine trackpad deltas; a conventional wheel notch is one step.
export class VolumeWheel {
  private remainder=0;
  private lastTime=-Infinity;
  private fine=false;
  step(deltaY:number,deltaMode:number,shift:boolean,time:number):number {
    if(!Number.isFinite(deltaY) || deltaY===0)return 0;
    if(time-this.lastTime>250 || shift!==this.fine || Math.sign(deltaY)!==Math.sign(this.remainder))this.remainder=0;
    this.lastTime=time;this.fine=shift;
    this.remainder+=deltaY*(deltaMode===1?40:deltaMode===2?100:1);
    const steps=Math.trunc(this.remainder/100);
    this.remainder-=steps*100;
    return steps===0?0:-steps*(shift?.01:.05);
  }
  reset() {this.remainder=0;this.lastTime=-Infinity;}
}
