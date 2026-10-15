// GG121 throttles classic slider writes at 80ms. Keep one request in flight
// and only the latest unsent value; slow I/O never creates a pointer-event queue.
export class LiveVolume {
  private desired: number | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private flight: Promise<void> | null = null;
  private sentAt = -Infinity;
  private ending = false;
  private failed = false;
  private confirmed: number;
  private send: (level:number, expected:number) => Promise<number>;
  private onError: (error:unknown) => void;
  private interval: number;
  constructor(confirmed: number, send: (level:number, expected:number) => Promise<number>, onError:(error:unknown) => void, interval = 80) {
    this.confirmed=confirmed;this.send=send;this.onError=onError;this.interval=interval;
  }
  set(level:number) {
    if (this.ending || this.failed) return;
    this.desired = level;
    this.pump();
  }
  private pump() {
    if (this.flight || this.timer || this.failed || this.desired === null) return;
    if (this.desired === this.confirmed) { this.desired = null; return; }
    const delay = this.ending ? 0 : this.interval - (performance.now() - this.sentAt);
    if (delay > 0) { this.timer = setTimeout(() => {this.timer=null;this.pump();},delay);return; }
    const level = this.desired; this.desired = null; this.sentAt = performance.now();
    this.flight = this.send(level,this.confirmed).then(actual => { this.confirmed = actual; })
      .catch(error => {this.failed=true;this.desired=null;this.onError(error);})
      .finally(() => {this.flight=null;this.pump();});
  }
  async finish(flush = true) {
    this.ending = true;
    if (this.timer) {clearTimeout(this.timer);this.timer=null;}
    if (!flush) this.desired = null;
    this.pump();
    while (this.flight) await this.flight;
  }
}
