/** A source uploads one file at a time, retaining only unfinished upload tasks for retry or cancellation. */
import type { UploadedBlob } from '@/generated/api-types';
import { UploadTask } from '@/lib/uploads';
import type { ImportSource, ImportSourceFile } from './importSource';

interface Task {
  run: (signal: AbortSignal, progress: (received: number) => void) => Promise<UploadedBlob>;
  cancel: (options?: { keepCompleted?: boolean }) => Promise<void>;
}

/** A completed upload and the source path the import will inspect. */
export interface ImportUploadedFile extends ImportSourceFile { blob: UploadedBlob }
/** Measured progress of the whole selection. */
export interface ImportUploadProgress { received: number; bytes: number; completed: number; files: number }
/** How many selected files no longer have an unfinished upload to cancel. */
export interface ImportCleanupProgress { completed: number; files: number }

export class ImportUploads {
  private readonly files: readonly ImportSourceFile[];
  private readonly tasks = new Map<number, Task>();
  // The present API needs one receipt per selected file. File bytes are never held here.
  private readonly results: ImportUploadedFile[] = [];
  private readonly bytes: number;
  private totalReceived = 0;
  private received = 0;
  private next = 0;
  private controller: AbortController | null = null;
  private pending: Promise<ImportUploadedFile[]> | null = null;
  private cancelled = false;
  private cancellation: Promise<void> | null = null;

  constructor(source: Extract<ImportSource, { fault: null }>, private readonly makeTask: (file: File) => Task = (file) => new UploadTask(file)) {
    this.files = source.files;
    this.bytes = source.bytes;
  }

  /** A failed attempt keeps its active session. Completed files and untouched files allocate no tasks. */
  run(signal: AbortSignal, progress: (state: ImportUploadProgress) => void): Promise<ImportUploadedFile[]> {
    if (this.cancelled) return Promise.reject(new DOMException('Upload cancelled', 'AbortError'));
    if (this.pending !== null) return Promise.reject(new Error('Upload already running'));
    const controller = new AbortController();
    const forwardAbort = () => controller.abort(signal.reason);
    signal.addEventListener('abort', forwardAbort, { once: true });
    if (signal.aborted) forwardAbort();
    this.controller = controller;
    const pending = this.upload(controller.signal, progress).finally(() => {
      signal.removeEventListener('abort', forwardAbort);
      this.controller = null;
      this.pending = null;
    });
    this.pending = pending;
    return pending;
  }

  /** Untouched files need no cleanup, and completed grants may be shared by another upload. */
  cancel(progress: (state: ImportCleanupProgress) => void = () => undefined): Promise<void> {
    if (this.cancellation !== null) return this.cancellation;
    this.cancelled = true;
    this.controller?.abort();
    this.cancellation = this.release(progress).catch((error: unknown) => { this.cancellation = null; throw error; });
    return this.cancellation;
  }

  private async release(progress: (state: ImportCleanupProgress) => void): Promise<void> {
    await this.pending?.catch(() => undefined);
    progress({ completed: this.files.length - this.tasks.size, files: this.files.length });
    for (const [index, task] of this.tasks) {
      await task.cancel({ keepCompleted: true });
      this.tasks.delete(index);
      progress({ completed: this.files.length - this.tasks.size, files: this.files.length });
    }
  }

  private async upload(signal: AbortSignal, progress: (state: ImportUploadProgress) => void): Promise<ImportUploadedFile[]> {
    const report = () => progress({ received: this.totalReceived, bytes: this.bytes, completed: this.next, files: this.files.length });
    signal.throwIfAborted();
    report();
    while (this.next < this.files.length) {
      signal.throwIfAborted();
      const entry = this.files[this.next];
      if (entry === undefined) throw new Error('Import source entry missing');
      let task = this.tasks.get(this.next);
      if (task === undefined) { task = this.makeTask(entry.file); this.tasks.set(this.next, task); }
      const count = (received: number) => {
        this.totalReceived += received - this.received;
        this.received = received;
      };
      const blob = await task.run(signal, (received) => { count(received); report(); });
      signal.throwIfAborted();
      count(entry.file.size);
      this.results.push({ ...entry, blob });
      this.tasks.delete(this.next);
      this.next += 1;
      this.received = 0;
      report();
    }
    return this.results;
  }
}
