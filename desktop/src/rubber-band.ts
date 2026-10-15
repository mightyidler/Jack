// Bounded travel in pixels; d is the available elastic envelope, not volume.
export const rubberBand=(x:number,d:number,c=.55)=>(1-1/(Math.abs(x)*c/d+1))*d;
