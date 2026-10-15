// Bald's 7px rail / 3px thumb, adapted as an overlay so its reserved gutter
// cannot add extra padding to the right of a JSON-defined card or popup.
export function mountScrollbar(scroller: HTMLElement, host: HTMLElement, kind: 'main' | 'menu' | 'modal') {
  const track = document.createElement('div'); track.className = `bald-scrollbar scrollbar-${kind}`;
  if (kind==='main' && scroller.classList.contains('render')) track.classList.add('scrollbar-output');
  const thumb = document.createElement('div'); thumb.className = 'bald-scrollbar-thumb';
  thumb.role = 'scrollbar'; thumb.setAttribute('aria-orientation','vertical');
  if (scroller.id) thumb.setAttribute('aria-controls',scroller.id);
  track.append(thumb); host.append(track);
  let frame = 0;
  let thumbHeight = 0;
  const paint = () => {
    if (kind === 'main') host.querySelector('.header')?.classList.toggle('is-scrolled',scroller.scrollTop > 0);
    const range = scroller.scrollHeight-scroller.clientHeight;
    track.hidden = range <= 1;
    if (range <= 1) return;
    const height = track.clientHeight;
    const minimum = kind === 'menu' ? 24 : kind === 'main' ? 184 : 78;
    thumbHeight = Math.min(height,Math.max(minimum,height*scroller.clientHeight/scroller.scrollHeight));
    thumb.style.height = `${thumbHeight}px`;
    thumb.style.transform = `translateY(${scroller.scrollTop/range*(height-thumbHeight)}px)`;
    thumb.setAttribute('aria-valuemin','0'); thumb.setAttribute('aria-valuemax',String(range));
    thumb.setAttribute('aria-valuenow',String(Math.round(scroller.scrollTop)));
  };
  const schedule = () => { if (!frame) frame = requestAnimationFrame(() => { frame=0; paint(); }); };
  const observer = new ResizeObserver(() => {
    if(kind==='main') {
      const header=host.querySelector<HTMLElement>('.header');
      if(header)track.style.top=`${header.offsetTop+header.offsetHeight}px`;
    }
    schedule();
  }); observer.observe(scroller);
  if(kind==='main') {
    const header=host.querySelector<HTMLElement>('.header');
    if(header) { track.style.top=`${header.offsetTop+header.offsetHeight}px`; observer.observe(header); }
  }
  // Disclosures can now live in any settings section, not just the first card.
  for (const child of scroller.children) observer.observe(child);
  const mutations = new MutationObserver(schedule); mutations.observe(scroller,{childList:true,subtree:true});
  scroller.addEventListener('scroll',schedule,{passive:true});
  let pointer: number | null = null; let startY = 0; let startScroll = 0;
  thumb.addEventListener('pointerdown',event => {
    event.preventDefault(); pointer=event.pointerId; startY=event.clientY; startScroll=scroller.scrollTop;
    thumb.setPointerCapture(pointer);
  });
  thumb.addEventListener('pointermove',event => {
    if (event.pointerId!==pointer) return;
    const travel=track.clientHeight-thumbHeight;
    if (travel>0) scroller.scrollTop=startScroll+(event.clientY-startY)*(scroller.scrollHeight-scroller.clientHeight)/travel;
  });
  const release = () => { pointer=null; };
  thumb.addEventListener('pointerup',release); thumb.addEventListener('pointercancel',release);
  paint();
  return () => {
    observer.disconnect(); mutations.disconnect(); cancelAnimationFrame(frame);
    scroller.removeEventListener('scroll',schedule); track.remove();
  };
}
