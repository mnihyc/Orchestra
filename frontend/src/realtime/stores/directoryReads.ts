/** Four actual fetches, including cancelled fetches that have not settled; a bounded caller owns each queued read. */
export const DIRECTORY_READ_CONCURRENCY = 4;
interface Queued { signal: AbortSignal; start: () => void; cancel: () => void }
export class DirectoryReads {
  private active = 0;
  private readonly queue: Queued[] = [];
  usage(): { active: number; queued: number } { return { active: this.active, queued: this.queue.length }; }
  run<T>(signal: AbortSignal, fetch: () => Promise<T>): Promise<T> {
    return new Promise((resolve, reject) => {
      const abort = () => {
        const index = this.queue.indexOf(job);
        if (index >= 0) this.queue.splice(index, 1);
        reject(new DOMException('Aborted', 'AbortError'));
      };
      const job: Queued = { signal, cancel: abort, start: () => {
        signal.removeEventListener('abort', abort);
        this.active += 1;
        // An active abort signals fetch but does not resolve the joined promise or credit a slot.
        // Only the actual dependency's settlement does that, even if a test/native transport ignores abort.
        let request: Promise<T>;
        try { request = fetch(); } catch (error: unknown) {
          request = Promise.reject(error instanceof Error ? error : new Error(typeof error === 'string' ? error : 'Directory request failed'));
        }
        void request.then(resolve, reject).finally(() => { this.active -= 1; this.drain(); });
      } };
      if (signal.aborted) { abort(); return; }
      signal.addEventListener('abort', abort, { once: true });
      this.queue.push(job); this.drain();
    });
  }
  private drain(): void {
    while (this.active < DIRECTORY_READ_CONCURRENCY && this.queue.length > 0) {
      const job = this.queue.shift();
      if (job === undefined) return;
      if (job.signal.aborted) job.cancel(); else job.start();
    }
  }
}
