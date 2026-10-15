import { cssTimeMs } from './motion-time';
import { captureDividerOpacity, animateDividerOpacity } from './divider-motion';
import { captureDevicePaint, animateDeviceChange } from './device-transition';

// One mounted surface and one geometry timeline, in a stable native viewport.
// Native calls change only its input/paint region, never WebView2's buffer size.
export class QuickSurface {
  private page: HTMLElement | null = null;
  private pending: (() => Promise<void>) | null = null;
  private running = false;
  private animations: Animation[] = [];
  private presented: (() => void) | null = null;
  private geometry: ResizeObserver;
  private prepared=new Map<string,HTMLElement>();
  constructor(private stage: HTMLElement, private resize: (height:number) => Promise<void>,private idle:()=>void=()=>{}) {
    this.geometry=new ResizeObserver(entries=>{
      const entry=entries[0];
      if(entry)this.publishHeight(entry.contentRect.height);
    });
    this.geometry.observe(stage);
  }
  get moving() { return this.running; }

  hasPrepared(key:string) { return this.prepared.has(key); }
  prepare(page:HTMLElement) {
    const key=page.dataset.viewKey!;
    if(this.prepared.has(key) || this.page?.dataset.viewKey===key)return;
    page.inert=true;page.setAttribute('aria-hidden','true');
    page.style.opacity='0';
    // Cached pages need DOM preparation, not four permanently blurred GPU layers.
    // Several prepared details share a back-button id. Keep those ids dormant
    // until activation so focus/ARIA lookups still find only the current page.
    page.querySelectorAll<HTMLElement>('[id]').forEach(node=>{node.dataset.preparedId=node.id;node.removeAttribute('id');});
    this.prepared.set(key,page);this.stage.append(page);
  }
  takePrepared(key:string) {
    const page=this.prepared.get(key);
    if(!page)return null;
    this.prepared.delete(key);
    page.querySelectorAll<HTMLElement>('[data-prepared-id]').forEach(node=>{node.id=node.dataset.preparedId!;delete node.dataset.preparedId;});
    return page;
  }
  clearPrepared() {
    this.prepared.forEach(page=>page.remove());this.prepared.clear();
  }

  show(page: HTMLElement, animate=true) {
    this.enqueue(()=>this.showPage(page,animate));
    // A hidden popup can settle without playing an invisible transition. Never
    // cancel back to the first keyframe, even when a close interrupts a page.
    if(!animate) {
      this.presented?.();
      this.animations.forEach(animation=>animation.finish());
    }
  }

  resizeContent(body: HTMLElement, update: () => void) {
    const owner=this.page;
    this.enqueue(async()=>{
      if(owner!==this.page || !owner?.isConnected)return;
      const from=this.stage.offsetHeight;
      const bodyFrom=body.offsetHeight;
      const inner=body.firstElementChild as HTMLElement;
      const wasOpen=body.closest('.t-acc')?.getAttribute('data-open')==='true';
      // Freeze the existing appearance while measuring the FINAL disclosure
      // layout. CSS grid and a second height transition must not race this one.
      inner.style.opacity=wasOpen?'1':'0';
      inner.style.filter=wasOpen?'blur(0px)':'blur(2px)';
      update();
      const bodyTo=body.offsetHeight;
      const to=owner.offsetHeight;
      const open=body.closest('.t-acc')?.getAttribute('data-open')==='true';
      body.style.height=`${bodyFrom}px`;
      try {
        await this.reserve(from,to);
        if(!this.reduced()) {
          // Assign exactly the same start time to all lanes, including height.
          const options=this.options;
          await this.play([
            this.stage.animate([{height:`${from}px`},{height:`${to}px`}],options),
            body.animate([{height:`${bodyFrom}px`},{height:`${bodyTo}px`}],options),
            inner.animate([{opacity:wasOpen?1:0,filter:wasOpen?'blur(0px)':'blur(2px)'},{opacity:open?1:0,filter:open?'blur(0px)':'blur(2px)'}],options),
          ]);
        }
      } finally {
        body.style.height=`${bodyTo}px`;
        inner.style.opacity=open?'1':'0';
        inner.style.filter=open?'blur(0px)':'blur(2px)';
        await this.settle(to);
        body.style.removeProperty('height');
        inner.style.removeProperty('opacity');
        inner.style.removeProperty('filter');
      }
    });
  }

  private enqueue(operation: () => Promise<void>) {
    this.pending=operation;
    if(!this.running)void this.drain();
  }

  private async drain() {
    this.running=true;
    this.stage.dataset.moving='true';
    try {
      while(this.pending) {
        const operation=this.pending;
        this.pending=null;
        try { await operation(); }
        catch(reason) { console.error(reason); }
      }
    } finally {
      this.stage.dataset.moving='false';
      this.running=false;
      this.idle();
    }
  }

  private reduced(animate=true) {
    return !animate || document.documentElement.dataset.popupOpen==='false' || matchMedia('(prefers-reduced-motion: reduce)').matches;
  }

  private async present() {
    if(document.hidden || document.documentElement.dataset.popupOpen==='false')return;
    // Let the prepared layer and its native input region reach presentation.
    await new Promise<void>(resolve=>{
      let frame=0;
      const finish=()=>{cancelAnimationFrame(frame);this.presented=null;resolve();};
      this.presented=finish;
      frame=requestAnimationFrame(finish);
    });
  }

  private async reserve(from:number,to:number) {
    this.stage.style.height=`${from}px`;
    this.publishHeight(from);
    await this.resize(Math.max(from,to));
    // The native viewport is stable; no extra two-frame resize wait is needed.
  }

  private async play(animations: Animation[]) {
    this.animations=animations;
    // Prepare the first filtered/layered frame before starting the 250ms clock.
    // Cold layer setup must not consume the beginning of the visible motion.
    animations.forEach(animation=>{animation.pause();animation.currentTime=0;});
    await Promise.all(animations.map(animation=>animation.ready));
    const start=document.timeline.currentTime;
    animations.forEach(animation=>{
      if(this.reduced())animation.finish();
      else {animation.play();if(typeof start==='number')animation.startTime=start;}
    });
    await Promise.all(animations.map(animation=>animation.finished));
  }

  private async animateHeight(from:number,to:number) {
    if(this.reduced() || Math.abs(to-from)<=.5)return;
    await this.play([this.stage.animate([{height:`${from}px`},{height:`${to}px`}],this.options)]);
  }

  private get options(): KeyframeAnimationOptions {
    const tokens=getComputedStyle(document.documentElement);
    // Production CSS minification rewrites 250ms as .25s. WAAPI always takes
    // milliseconds: parsing only the number makes a visible tween last .25ms.
    const duration=cssTimeMs(tokens.getPropertyValue('--page-slide-dur'),250);
    return {duration,easing:tokens.getPropertyValue('--page-slide-ease').trim() || 'cubic-bezier(.22,1,.36,1)',fill:'forwards'};
  }

  private publishHeight(height:number) {
    // This changes CSS positioning only. It never calls native resize or changes
    // the observed stage's dimensions, so it cannot form a ResizeObserver loop.
    document.documentElement.style.setProperty('--quick-surface-height',`${height+58}px`);
  }

  private async settle(height:number) {
    // Commit the final underlying styles before removing animation effects.
    // Hold the moving flag through the final input-region update as well.
    this.stage.style.height=`${height}px`;
    this.publishHeight(height);
    this.animations.forEach(animation=>animation.cancel());
    this.animations=[];
    await this.resize(height);
    await this.present();
  }

  private async showPage(page:HTMLElement,animate:boolean) {
    const previous=this.page;
    const dividerOpacity=captureDividerOpacity(previous);
    const devicePaint=captureDevicePaint(previous?.dataset.viewKey===page.dataset.viewKey?previous:null);
    const focused=document.activeElement as HTMLElement | null;
    const focusId=previous?.contains(focused) ? focused?.id : undefined;
    const changed=!!previous && previous.dataset.viewKey!==page.dataset.viewKey;
    const transition=changed && !this.reduced(animate);
    const direction=page.dataset.pageId==='2'?1:-1;
    const tokens=getComputedStyle(this.stage);
    const distance=parseFloat(tokens.getPropertyValue('--page-slide-distance')) || 8;
    const blur=parseFloat(tokens.getPropertyValue('--page-blur')) || 3;
    const from=this.stage.offsetHeight;
    // Keep prepared layers mounted and transparent through measurement.
    page.style.opacity='0';
    page.inert=true;
    if(page.parentElement!==this.stage)this.stage.append(page);
    const to=page.offsetHeight;
    try {
      await this.reserve(from || to,to);
      if(previous) {
        previous.inert=true;
        previous.setAttribute('aria-hidden','true');
        previous.removeAttribute('id');
        previous.querySelectorAll('[id]').forEach(node=>node.removeAttribute('id'));
      }
      this.page=page;
      page.style.opacity=transition?'0':'1';
      page.style.transform=transition?`translate3d(${direction*distance}px,0,0)`:'translate3d(0,0,0)';
      page.style.filter=transition?`blur(${blur}px)`:'blur(0px)';
      page.style.removeProperty('visibility');
      page.removeAttribute('aria-hidden');
      page.inert=false;
      if(!this.reduced(animate))animateDividerOpacity(page,dividerOpacity);
      if(!this.reduced(animate))animateDeviceChange(page,devicePaint);
      if(transition) {
        page.dataset.animating='true';previous!.dataset.animating='true';
        const animations: Animation[]=[];
        const options=this.options;
        if(Math.abs(to-from)>.5)animations.push(this.stage.animate([{height:`${from}px`},{height:`${to}px`}],options));
        // Transitions.dev page slide: the incoming page resolves from 3px blur
        // as the outgoing page softens away. All lanes share the geometry clock.
        animations.push(page.animate([{opacity:0,transform:`translate3d(${direction*distance}px,0,0)`,filter:`blur(${blur}px)`},{opacity:1,transform:'translate3d(0,0,0)',filter:'blur(0px)'}],options));
        animations.push(previous!.animate([{opacity:1,transform:'translate3d(0,0,0)',filter:'blur(0px)'},{opacity:0,transform:`translate3d(${-direction*distance}px,0,0)`,filter:`blur(${blur}px)`}],options));
        await this.play(animations);
      } else {
        previous?.remove();
        if(!this.reduced(animate))await this.animateHeight(from || to,to);
      }
    } finally {
      // Retire the outgoing layer before cancelling its fill-forwards effect.
      // It must never return to opacity:1 for a compositor frame.
      previous?.remove();
      delete page.dataset.animating;
      page.style.visibility='visible';
      page.style.opacity='1';
      page.style.transform='translate3d(0,0,0)';
      page.style.filter='blur(0px)';
      page.inert=false;
      this.page=page;
      await this.settle(to);
      if(focusId && document.activeElement===document.body)page.querySelector<HTMLElement>(`#${CSS.escape(focusId)}`)?.focus({preventScroll:true});
    }
  }
}
