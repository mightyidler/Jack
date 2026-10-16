import { cssTimeMs } from './motion-time';
import './output-group-motion.css';

interface GroupPaint {
  key:string; node:HTMLElement; height:number; margin:number;
  following:string[];
  opacity:string; transform:string; filter:string; exiting:boolean; moving:boolean;
}
export interface OutputGroupPaint { items:GroupPaint[]; scrollTop:number; focused:string }
const selector='.sonar-channel[data-channel],.sonar-group-add';
const keyOf=(node:HTMLElement)=>node.dataset.channel??'add';
const motionId='output-group-presence';

// Own presence across the app's periodic DOM replacements. A render snapshots
// the painted frame, including a half-collapsed exit, before replacing anything.
export class OutputGroupMotion {
  private deadline=0;
  private stopScroll:(()=>void)|null=null;
  private scrolling=false;
  private targetKey='';

  capture(panel:HTMLElement|null):OutputGroupPaint {
    const items:GroupPaint[]=[];
    if(panel)for(const node of panel.querySelectorAll<HTMLElement>(selector)) {
      const slot=node.parentElement?.classList.contains('output-group-slot')?node.parentElement:null;
      const paint=getComputedStyle(node);
      const following:string[]=[];
      for(let next=(slot??node).nextElementSibling;next;next=next.nextElementSibling) {
        const sibling=next.classList.contains('output-group-slot')?next.firstElementChild:next;
        if(sibling instanceof HTMLElement)following.push(sibling.dataset.channel??(sibling.classList.contains('sonar-group-add')?'add':'other'));
      }
      items.push({key:keyOf(node),node:node.cloneNode(true) as HTMLElement,
        following,
        height:(slot??node).getBoundingClientRect().height,
        margin:slot?Number(getComputedStyle(slot).marginBottom.replace('px','')):0,
        opacity:paint.opacity,transform:paint.transform,filter:paint.filter,
        exiting:slot?.hasAttribute('data-output-group-exit')??false,moving:!!slot});
    }
    return {items,scrollTop:panel?.scrollTop??0,focused:(document.activeElement as HTMLElement)?.id??''};
  }

  animate(panel:HTMLElement,previous:OutputGroupPaint) {
    const wasScrolling=this.scrolling;
    this.stopScroll?.();this.stopScroll=null;
    this.scrolling=false;
    const section=panel.querySelector<HTMLElement>('.sonar-channels');
    if(!section || !previous.items.length)return;
    const nodes=[...section.querySelectorAll<HTMLElement>(selector)];
    const old=new Map(previous.items.map(item=>[item.key,item]));
    const current=new Map(nodes.map(node=>[keyOf(node),node]));
    const added=nodes.filter(node=>!old.has(keyOf(node)) || old.get(keyOf(node))!.exiting);
    const removed=previous.items.filter(item=>!item.exiting && !current.has(item.key));
    const changed=added.length>0 || removed.length>0;
    if(!changed && !previous.items.some(item=>item.moving))return;

    const style=getComputedStyle(section);
    const opening=cssTimeMs(style.getPropertyValue('--output-group-open-duration'),400);
    const closing=cssTimeMs(style.getPropertyValue('--output-group-close-duration'),350);
    const fade=cssTimeMs(style.getPropertyValue('--output-group-fade-duration'),150);
    const easing=style.getPropertyValue('--output-group-ease').trim()||'cubic-bezier(0.22,1,0.36,1)';
    const gap=Number(style.rowGap.replace('px',''))||0;
    const now=performance.now();
    const duration=changed?(added.some(node=>keyOf(node)!=='add')?opening:closing):Math.max(0,this.deadline-now);
    this.deadline=now+duration;
    if(changed)this.targetKey=added.find(node=>keyOf(node)!=='add')?.dataset.channel??'add';
    const shouldScroll=changed||wasScrolling;
    const reduced=matchMedia('(prefers-reduced-motion: reduce)').matches;
    const target=()=>current.get(this.targetKey);
    const targetNode=target();
    const padding=Number(getComputedStyle(panel).paddingBottom.replace('px',''));
    // Measure the final endpoint before installing presence slots. Following a
    // partially expanded card's bottom would briefly scroll backwards on add.
    const bottom=targetNode?targetNode.getBoundingClientRect().bottom-panel.getBoundingClientRect().top+panel.scrollTop+padding-panel.clientHeight:previous.scrollTop;
    const destination=Math.max(0,Math.min(panel.scrollHeight-panel.clientHeight,bottom));
    if(reduced || duration===0) {
      panel.scrollTop=shouldScroll?destination:previous.scrollTop;
      this.restoreFocus(panel,previous.focused);
      return true;
    }

    const start=document.timeline.currentTime;
    const play=(node:HTMLElement,frames:Keyframe[],time:number)=>{
      const animation=node.animate(frames,{duration:time,easing,fill:'both'});
      animation.id=motionId;
      if(typeof start==='number')animation.startTime=start;
      return animation;
    };
    const wrap=(node:HTMLElement)=>{
      const slot=document.createElement('div');slot.className='output-group-slot';
      node.before(slot);slot.append(node);return slot;
    };
    const settle=(slot:HTMLElement,node:HTMLElement,exiting:boolean,height:number,margin:number,paint:GroupPaint|undefined)=>{
      const toHeight=exiting?0:node.getBoundingClientRect().height;
      const slotMotion=play(slot,[{height:`${height}px`,marginBottom:`${margin}px`},
        {height:`${toHeight}px`,marginBottom:`${exiting?-gap:0}px`}],duration);
      const visual=play(node,[
        {opacity:paint?.opacity??0,transform:paint?.transform??'translateY(12px)',filter:paint?.filter??'blur(2px)'},
        {opacity:exiting?0:1,transform:exiting?'translateY(-4px)':'translateY(0)',filter:exiting?'blur(2px)':'blur(0px)'},
      ],exiting?Math.min(fade,duration):duration);
      void slotMotion.finished.then(()=>{
        if(exiting)slot.remove();else slot.replaceWith(node);
        slotMotion.cancel();visual.cancel();
      }).catch(()=>{});
    };
    // Reinsert inert paint for removed nodes at their former position. Clones
    // have no IDs or live controls and never participate in app-route motion.
    for(const item of previous.items) {
      if(current.has(item.key))continue;
      const node=item.node;
      for(const child of [node,...node.querySelectorAll<HTMLElement>('[id],[data-control],[data-device-motion-key]')]) {
        child.removeAttribute('id');child.removeAttribute('data-control');child.removeAttribute('data-device-motion-key');
      }
      const slot=document.createElement('div');slot.className='output-group-slot';
      slot.dataset.outputGroupExit='';slot.inert=true;slot.setAttribute('aria-hidden','true');slot.append(node);
      const following=item.following.map(key=>key==='other'?section.querySelector<HTMLElement>('.sonar-channel:not([data-channel])'):current.get(key)).find(Boolean)
        ??(item.key==='add'?null:current.get('add'));
      section.insertBefore(slot,following??null);
      settle(slot,node,true,item.height,item.margin,item);
    }
    for(const node of nodes) {
      const item=old.get(keyOf(node));
      if(item && !item.moving && !item.exiting)continue;
      const slot=wrap(node);
      settle(slot,node,false,item?.height??0,item?.margin??-gap,item);
    }
    // Restore after installing the initial layout frame, so deleting a tall
    // card cannot clamp the saved scroll position before the collapse begins.
    panel.scrollTop=previous.scrollTop;
    this.restoreFocus(panel,previous.focused);
    if(shouldScroll)this.follow(panel,destination,duration);
    return true;
  }

  private restoreFocus(panel:HTMLElement,focused:string) {
    const replacement=focused==='group-add'?panel.querySelector<HTMLElement>('.sonar-group-remove')
      :focused.startsWith('group-remove-')?panel.querySelector<HTMLElement>('#group-add'):null;
    if(!document.getElementById(focused))replacement?.focus({preventScroll:true});
  }

  private follow(panel:HTMLElement,destination:number,duration:number) {
    let frame=0;
    const start=performance.now(),from=panel.scrollTop;
    this.scrolling=true;
    const stop=()=>{
      cancelAnimationFrame(frame);this.scrolling=false;
      panel.removeEventListener('wheel',interrupt);panel.removeEventListener('touchstart',interrupt);
      panel.removeEventListener('pointerdown',interrupt);panel.removeEventListener('keydown',onKey);
    };
    const interrupt=()=>{stop();this.stopScroll=null;};
    const onKey=(event:KeyboardEvent)=>{
      if(['ArrowUp','ArrowDown','PageUp','PageDown','Home','End',' '].includes(event.key))interrupt();
    };
    const tick=(now:number)=>{
      if(!panel.isConnected){interrupt();return;}
      const progress=Math.min(1,(now-start)/duration);
      // Evaluate the same .22,1,.36,1 curve as the layout lanes. Clamp against
      // their live range while moving toward the measured, settled endpoint.
      const ease=smoothOut(progress);
      panel.scrollTop=from+(destination-from)*ease;
      if(progress<1)frame=requestAnimationFrame(tick);else interrupt();
    };
    panel.addEventListener('wheel',interrupt,{passive:true});panel.addEventListener('touchstart',interrupt,{passive:true});
    panel.addEventListener('pointerdown',interrupt,{passive:true});panel.addEventListener('keydown',onKey);
    this.stopScroll=stop;frame=requestAnimationFrame(tick);
  }
}

function smoothOut(progress:number) {
  if(progress<=0)return 0;
  if(progress>=1)return 1;
  let low=0,high=1,t=progress;
  for(let i=0;i<12;i++) {
    const x=3*(1-t)*(1-t)*t*.22+3*(1-t)*t*t*.36+t*t*t;
    if(x<progress)low=t;else high=t;
    t=(low+high)/2;
  }
  return 1-(1-t)**3;
}
