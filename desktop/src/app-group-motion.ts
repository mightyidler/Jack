import { cssTimeMs } from './motion-time';

interface ItemPaint { rect:DOMRect; group:string; visible:boolean }
interface GroupPaint { rect:DOMRect; items:Map<string,ItemPaint> }
export interface AppGroupPaint extends Map<string,GroupPaint> {scrollTop:number}
const motionId='app-group-layout';
const groups=(root:ParentNode)=>[...root.querySelectorAll<HTMLElement>('.sonar-channel')]
  .filter(node=>!node.closest('[data-output-group-exit]'));
const groupKey=(node:HTMLElement)=>node.dataset.channel??'other';
const itemKey=(node:HTMLElement)=>node.classList.contains('sonar-app-divider')
  ? `divider:${node.nextElementSibling?.id??''}` : node.id || 'drop-slot';
const items=(node:HTMLElement)=>[...node.querySelectorAll<HTMLElement>('.sonar-app,.sonar-app-divider,.app-drop-slot')];

// Read painted geometry, including an interrupted animation. All reads happen
// before cancellation/writes so changing targets never snaps back to layout.
export function captureAppGroups(root:ParentNode|null):AppGroupPaint {
  const paint:AppGroupPaint=Object.assign(new Map<string,GroupPaint>(),{scrollTop:root instanceof HTMLElement?root.scrollTop:0});
  if(!root)return paint;
  const ghost=document.querySelector<HTMLElement>('.app-drag-ghost');
  for(const group of groups(root)) {
    const key=groupKey(group);
    paint.set(key,{rect:group.getBoundingClientRect(),items:new Map(items(group).map(node=>{
      const landing=(node.classList.contains('app-drag-source') || node.classList.contains('app-drag-returning')) && ghost?.dataset.landing==='true';
      return [itemKey(node),{
        rect:landing?ghost.getBoundingClientRect():node.getBoundingClientRect(),group:key,
        visible:landing || node.getClientRects().length>0 && getComputedStyle(node).visibility!=='hidden',
      }];
    }))});
  }
  return paint;
}

export function stopAppGroupMotion(root:ParentNode) {
  for(const card of groups(root))for(const animation of card.getAnimations({subtree:true})) {
    if(animation.id===motionId)animation.cancel();
  }
}

export function animateAppGroups(root:ParentNode,previous:AppGroupPaint) {
  const cards=groups(root);
  stopAppGroupMotion(root);
  if(!previous.size || matchMedia('(prefers-reduced-motion: reduce)').matches)return;
  const style=getComputedStyle(cards[0]??document.documentElement);
  const duration=cssTimeMs(style.getPropertyValue('--app-group-duration'),350);
  const fade=cssTimeMs(style.getPropertyValue('--app-motion-quick'),150);
  const easing=style.getPropertyValue('--app-motion-ease').trim()||'cubic-bezier(0.22,1,0.36,1)';
  if(duration===0)return;
  const previousItems=new Map([...previous.values()].flatMap(group=>[...group.items]));
  const measured=cards.map(card=>({card,key:groupKey(card),rect:card.getBoundingClientRect(),
    items:items(card).map(node=>({node,key:itemKey(node),rect:node.getBoundingClientRect()}))}));
  const start=document.timeline.currentTime;
  const animate=(node:HTMLElement,frames:Keyframe[],time=duration)=>{
    const animation=node.animate(frames,{duration:time,easing});
    animation.id=motionId;
    if(typeof start==='number')animation.startTime=start;
    return animation;
  };
  // Animate real height instead of scaling a card: text, icons, corners and the
  // gap between groups keep their shape while every following group flows.
  for(const group of measured) {
    const before=previous.get(group.key);
    if(!before)continue;
    const delta=before.rect.height-group.rect.height;
    if(Math.abs(delta)>.5)animate(group.card,[{height:`${before.rect.height}px`},{height:`${group.rect.height}px`}]);
    for(const item of group.items) {
      const old=previousItems.get(item.key);
      // A display:none row has a zero rect, not a painted origin at (0, 0).
      if(!old?.visible) {
        if(fade>0)animate(item.node,[{opacity:0},{opacity:1}],fade);
        continue;
      }
      // Group translation comes from the height lanes above. Animate only the
      // item's local displacement, otherwise children would travel twice.
      const dy=old.rect.top-before.rect.top-(item.rect.top-group.rect.top);
      const dx=old.rect.left-before.rect.left-(item.rect.left-group.rect.left);
      if(Math.abs(dy)>.5 || Math.abs(dx)>.5) {
        const moved=old.group!==group.key;
        if(moved)item.node.classList.add('app-transfer-arrival');
        const animation=animate(item.node,[{transform:`translate3d(${dx}px,${dy}px,0)`},{transform:'translate3d(0,0,0)'}]);
        void animation.finished.catch(()=>{}).then(()=>{
          if(!item.node.getAnimations().some(active=>active.id===motionId && active.playState!=='finished')) {
            item.node.classList.remove('app-transfer-arrival');
          }
        });
      }
    }
  }
  // Cancelling height effects briefly exposes the smaller final scroll range.
  // Restore after installing all initial heights so the browser cannot clamp
  // scrollTop upwards every time the bottom-edge slot is retargeted.
  if(root instanceof HTMLElement)root.scrollTop=previous.scrollTop;
}
