import { cssTimeMs } from './motion-time';
import { captureDividerOpacity, animateDividerOpacity } from './divider-motion';
import { captureDevicePaint, animateDeviceChange } from './device-transition';
import { captureInputSelection, animateInputSelection } from './input-selection-motion';

// One mounted surface and one geometry timeline, in a stable native viewport.
// Native calls change only its input/paint region, never WebView2's buffer size.
export class QuickSurface {
  private page: HTMLElement | null = null;
  private pending: (() => Promise<void>) | null = null;
  private pendingPage: HTMLElement | null = null;
  private running = false;
  private animations: Animation[] = [];
  private outgoing: HTMLElement[] = [];
  private interrupted = false;
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
    this.enqueue(()=>this.showPage(page,animate),page);
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
      const from=parseFloat(getComputedStyle(this.stage).height);
      const bodyFrom=parseFloat(getComputedStyle(body).height);
      const inner=body.firstElementChild as HTMLElement;
      const opacity=Number(getComputedStyle(inner).opacity);
      const arrow=body.closest('.t-acc')?.querySelector<HTMLElement>('.t-acc-chevron');
      const arrowFrom=arrow?getComputedStyle(arrow).transform:'none';
      if(arrow)arrow.style.transform=arrowFrom;
      // Freeze the existing appearance while measuring the FINAL disclosure
      // layout. CSS grid and a second height transition must not race this one.
      inner.style.opacity=String(opacity);
      body.style.removeProperty('height');
      update();
      const bodyTo=body.offsetHeight;
      const to=owner.offsetHeight;
      const open=body.closest('.t-acc')?.getAttribute('data-open')==='true';
      body.style.height=`${bodyFrom}px`;
      try {
        await this.reserve(from,to);
        if(!this.reduced() && !this.interrupted) {
          // Assign exactly the same start time to all lanes, including height.
          const options=this.options;
          const animations=[
            this.stage.animate([{height:`${from}px`},{height:`${to}px`}],options),
            body.animate([{height:`${bodyFrom}px`},{height:`${bodyTo}px`}],options),
            inner.animate([{opacity},{opacity:open?1:0}],{...options,duration:cssTimeMs(getComputedStyle(inner).getPropertyValue('--quick-content-duration'),150)}),
          ];
          animations.push(...this.dissolve(owner,Number(getComputedStyle(owner).opacity)));
          if(arrow) {
            const tokens=getComputedStyle(arrow);
            animations.push(arrow.animate([
              {transform:arrowFrom},
              {transform:`rotateZ(${open?180:0}deg)`},
            ],{
              duration:cssTimeMs(tokens.getPropertyValue('--duration-fast'),250),
              easing:tokens.getPropertyValue('--ease-smooth-out').trim() || 'cubic-bezier(.22,1,.36,1)',
              fill:'forwards',
            }));
          }
          await this.play(animations);
        }
      } finally {
        if(this.interrupted) {
          // Keep the painted disclosure for the next gesture to measure.
          await this.settle(parseFloat(getComputedStyle(this.stage).height));
        } else {
          body.style.height=`${bodyTo}px`;
          inner.style.opacity=open?'1':'0';
          owner.style.opacity='1';
          this.retireOutgoing();
          if(arrow)arrow.style.transform=`rotateZ(${open?180:0}deg)`;
          await this.settle(to);
          arrow?.style.removeProperty('transform');
          body.style.removeProperty('height');
          inner.style.removeProperty('opacity');
        }
      }
    });
  }

  private enqueue(operation: () => Promise<void>,page?:HTMLElement) {
    this.pendingPage?.remove();
    this.pendingPage=page??null;
    this.pending=operation;
    if(this.running) {
      this.interrupted=true;
      // A new gesture starts from the frame on screen, without waiting for the
      // old destination or jumping to it. Commit before cancelling the effects.
      this.animations.forEach(animation=>{animation.commitStyles();animation.cancel();});
      this.animations=[];
      this.presented?.();
    }
    if(!this.running)void this.drain();
  }

  private async drain() {
    this.running=true;
    this.stage.dataset.moving='true';
    try {
      while(this.pending) {
        const operation=this.pending;
        this.pending=null;
        this.pendingPage=null;
        this.interrupted=false;
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
    const finished=Promise.allSettled(animations.map(animation=>animation.finished));
    await Promise.allSettled(animations.map(animation=>animation.ready));
    if(this.interrupted)return;
    const start=document.timeline.currentTime;
    animations.forEach(animation=>{
      if(this.reduced())animation.finish();
      else {animation.play();if(typeof start==='number')animation.startTime=start;}
    });
    await finished;
  }

  private dissolve(page:HTMLElement,opacity:number) {
    const tokens=getComputedStyle(this.stage);
    const options:KeyframeAnimationOptions={
      duration:cssTimeMs(tokens.getPropertyValue('--quick-content-duration'),150),
      easing:tokens.getPropertyValue('--quick-content-ease').trim() || 'ease-in-out',
      fill:'forwards',
    };
    return [
      ...(opacity<1?[page.animate([{opacity},{opacity:1}],options)]:[]),
      ...this.outgoing.map(layer=>layer.animate([{opacity:Number(getComputedStyle(layer).opacity)},{opacity:0}],options)),
    ];
  }

  private retireOutgoing() {
    this.outgoing.forEach(layer=>layer.remove());
    this.outgoing=[];
  }

  private get options(): KeyframeAnimationOptions {
    const tokens=getComputedStyle(document.documentElement);
    // Production CSS minification rewrites 250ms as .25s. WAAPI always takes
    // milliseconds: parsing only the number makes a visible tween last .25ms.
    const duration=cssTimeMs(tokens.getPropertyValue('--quick-resize-duration'),250);
    return {duration,easing:tokens.getPropertyValue('--ease-smooth-out').trim() || 'cubic-bezier(.22,1,.36,1)',fill:'forwards'};
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
    if(!this.pending)await this.present();
  }

  private async showPage(page:HTMLElement,animate:boolean) {
    const previous=this.page;
    const dividerOpacity=captureDividerOpacity(previous);
    const devicePaint=captureDevicePaint(previous?.dataset.viewKey===page.dataset.viewKey?previous:null);
    const inputSelection=captureInputSelection(previous?.dataset.viewKey===page.dataset.viewKey?previous:null);
    const focused=document.activeElement as HTMLElement | null;
    const focusId=previous?.contains(focused) ? focused?.id : undefined;
    const changed=!!previous && previous.dataset.viewKey!==page.dataset.viewKey;
    const transition=changed && !this.reduced(animate);
    const previousOpacity=previous?Number(getComputedStyle(previous).opacity):1;
    const from=parseFloat(getComputedStyle(this.stage).height);
    // Keep prepared layers mounted and transparent through measurement.
    page.style.opacity='0';
    page.inert=true;
    if(page.parentElement!==this.stage)this.stage.append(page);
    const to=page.offsetHeight;
    let activated=false;
    try {
      await this.reserve(from || to,to);
      if(this.interrupted)return;
      if(previous) {
        previous.inert=true;
        previous.setAttribute('aria-hidden','true');
        previous.removeAttribute('id');
        previous.querySelectorAll('[id]').forEach(node=>node.removeAttribute('id'));
      }
      this.page=page;
      activated=true;
      page.style.opacity=transition?'0':String(previousOpacity);
      page.style.removeProperty('visibility');
      page.removeAttribute('aria-hidden');
      page.inert=false;
      if(!this.reduced(animate))animateDividerOpacity(page,dividerOpacity);
      // Navigation is a stationary dissolve. In-place device changes use the
      // same fixed slots, without the main window's vertical push.
      if(!this.reduced(animate))animateDeviceChange(page,devicePaint,{slide:false});
      if(!this.reduced(animate))animateInputSelection(page,inputSelection);
      if(transition)this.outgoing.push(previous!);
      else previous?.remove();
      if(!this.reduced(animate)) {
        page.dataset.animating='true';
        this.outgoing.forEach(layer=>layer.dataset.animating='true');
        const animations: Animation[]=[];
        const options=this.options;
        if(Math.abs(to-from)>.5)animations.push(this.stage.animate([{height:`${from}px`},{height:`${to}px`}],options));
        animations.push(...this.dissolve(page,transition?0:previousOpacity));
        if(animations.length)await this.play(animations);
      }
    } finally {
      if(!activated)page.remove();
      else {
        // Interrupted dissolves retain every painted layer for the next gesture.
        // Removing the dominant outgoing layer here would cause a one-frame flash.
        if(!this.interrupted) {
          this.retireOutgoing();
          page.style.opacity='1';
        }
        delete page.dataset.animating;
        page.style.visibility='visible';
        page.inert=false;
        this.page=page;
        await this.settle(this.interrupted?parseFloat(getComputedStyle(this.stage).height):to);
        if(focusId && document.activeElement===document.body)page.querySelector<HTMLElement>(`#${CSS.escape(focusId)}`)?.focus({preventScroll:true});
      }
    }
  }
}
