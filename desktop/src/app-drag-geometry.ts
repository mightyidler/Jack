interface Bounds {left:number;right:number;top:number;bottom:number}
const edge=48;

export function edgeScrollDelta(x:number,y:number,panel:Bounds,elapsedMs:number,contentTop=panel.top):number {
  if(x<panel.left || x>panel.right || y<panel.top || y>panel.bottom)return 0;
  // The panel extends behind the fixed header. Start the upper edge zone at
  // the visible content boundary, and keep scrolling if the pointer overshoots
  // into the header while remaining inside the panel.
  const top=Math.max(panel.top,Math.min(contentTop,panel.bottom));
  const speed=y<top+edge?-Math.min(600,(top+edge-y)*15)
    :y>panel.bottom-edge?Math.min(600,(y-panel.bottom+edge)*15):0;
  // RAF frequency varies (60/120/144 Hz). Speed is pixels/second, and a delayed
  // frame must not fling the list when the window becomes responsive again.
  return speed*Math.min(32,Math.max(0,elapsedMs))/1000;
}

// The last card ends above the panel's bottom padding. Treat that edge strip
// as its drop area, even while a slot/height animation changes the hit surface.
export function bottomEdgeGroup(x:number,y:number,panel:Bounds,cards:readonly Bounds[]):number {
  if(x<panel.left || x>panel.right || y<panel.bottom-edge || y>panel.bottom)return -1;
  let nearest=-1,distance=Infinity;
  cards.forEach((card,index)=>{
    if(x<card.left || x>card.right || card.top>=panel.bottom || card.bottom<=panel.top)return;
    const delta=Math.max(card.top-y,y-card.bottom,0);
    if(delta<distance){nearest=index;distance=delta;}
  });
  return nearest;
}
