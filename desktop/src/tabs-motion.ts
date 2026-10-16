// A critically damped indicator follows new selections from its current
// position and velocity, without overshoot or frame-rate-dependent integration.
export class SlidingTabs {
  private pill: HTMLElement | null = null;
  private observer: ResizeObserver | null = null;
  private frame = 0;
  private measureFrame = 0;
  private last = 0;
  private initialized = false;
  private position = [0, 0];
  private velocity = [0, 0];
  private target = [0, 0];

  mount(rail: HTMLElement, pill: HTMLElement) {
    this.observer?.disconnect();
    cancelAnimationFrame(this.measureFrame);
    this.pill = pill;
    const measure = () => {
      const selected = rail.querySelector<HTMLElement>('[aria-selected=true]')!;
      const railRect = rail.getBoundingClientRect();
      const selectedRect = selected.getBoundingClientRect();
      const scale = railRect.width / parseFloat(getComputedStyle(rail).width);
      this.target[0] = (selectedRect.left - railRect.left) / scale;
      this.target[1] = selectedRect.width / scale;
      if (!this.initialized) {
        this.position = [...this.target]; this.initialized = true;
      }
      this.paint(); this.start();
    };
    this.observer = new ResizeObserver(() => {
      cancelAnimationFrame(this.measureFrame);
      this.measureFrame = requestAnimationFrame(measure);
    });
    this.observer.observe(rail);
    measure();
  }

  private paint() {
    if (!this.pill) return;
    this.pill.style.left = '0';
    this.pill.style.transform = `translate3d(${this.position[0]}px,0,0)`;
    this.pill.style.width = `${this.position[1]}px`;
  }

  private start() {
    if (matchMedia('(prefers-reduced-motion: reduce)').matches) {
      cancelAnimationFrame(this.frame); this.frame = 0;
      this.position = [...this.target]; this.velocity.fill(0); this.paint(); return;
    }
    if (!this.frame) { this.last = performance.now(); this.frame = requestAnimationFrame(this.tick); }
  }

  private tick = (time: number) => {
    this.frame = 0;
    const elapsed = Math.max(0,Math.min((time - this.last) / 1000,.064)); this.last = time;
    const frequency=24,decay=Math.exp(-frequency*elapsed);
    for (let i = 0; i < 2; i++) {
      const offset=this.position[i]-this.target[i];
      const momentum=this.velocity[i]+frequency*offset;
      this.position[i]=this.target[i]+(offset+momentum*elapsed)*decay;
      this.velocity[i]=(this.velocity[i]-frequency*momentum*elapsed)*decay;
    }
    const settled = this.position.every((value,i) => Math.abs(value-this.target[i]) < .0005 && Math.abs(this.velocity[i]) < .005);
    if (settled) { this.position = [...this.target]; this.velocity.fill(0); }
    this.paint();
    if (!settled) this.frame = requestAnimationFrame(this.tick);
  };
}
