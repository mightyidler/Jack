// ello-reboot AppShell's measured indicator and butterySpring (170 / 26),
// adapted to Jack's DOM renderer without adding React or Framer Motion.
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
    this.pill.style.left = `${this.position[0]}px`;
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
    const elapsed = Math.min((time - this.last) / 1000,.032); this.last = time;
    const steps = Math.max(1,Math.ceil(elapsed / (1 / 120))); const dt = elapsed / steps;
    for (let step = 0; step < steps; step++) {
      for (let i = 0; i < 2; i++) {
        this.velocity[i] += (170 * (this.target[i] - this.position[i]) - 26 * this.velocity[i]) * dt;
        this.position[i] += this.velocity[i] * dt;
      }
    }
    const settled = this.position.every((value,i) => Math.abs(value-this.target[i]) < .0005 && Math.abs(this.velocity[i]) < .005);
    if (settled) { this.position = [...this.target]; this.velocity.fill(0); }
    this.paint();
    if (!settled) this.frame = requestAnimationFrame(this.tick);
  };
}
