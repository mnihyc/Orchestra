/** Durable ranges and a measured request window shared by simultaneous file uploads. */
import { ApiError, api, client, ok, repeatable } from '@/api/client';
import type { ChatPart, ContentPart, DocumentFile, ImageMediaType, UploadedBlob, UploadDestination, UploadSessionPutReply, UploadRange, UploadSessionCreateReply, UploadSessionStatus, TransferPolicy } from '@/generated/api-types';
import { NetworkError, newActKey, READ_RETRY_PAUSES_MS } from '@/lib/requests';

/** The ordinary authenticated requests, also replaceable by a transport double. */
export interface UploadTransport {
  begin: (file: File, signal: AbortSignal, key: string) => Promise<UploadSessionCreateReply>;
  status: (id: string, signal: AbortSignal) => Promise<UploadSessionStatus>;
  put: (id: string, offset: number, bytes: Blob, signal: AbortSignal, sent?: (started: number) => void) => Promise<UploadSessionPutReply>;
  complete: (id: string, signal: AbortSignal) => Promise<UploadedBlob>;
  cancel?: (id: string) => Promise<void>;
  release?: (blob: string) => Promise<void>;
  current?: () => boolean;
  /** Browser instances of the same reader share admission even across destinations. */
  window?: TransferWindow;
}

const transport: UploadTransport = {
  begin: beginUpload,
  status: (id, signal) => ok(api.GET('/uploads/sessions/{upload_id}', { params: { path: { upload_id: id } }, signal })),
  put: (id, offset, bytes, signal, sent) => client.putFilePiece(id, offset, bytes, signal, sent),
  complete: async (id, signal) => {
    for (;;) {
      signal.throwIfAborted();
      const result = await ok(api.POST('/uploads/sessions/{upload_id}/complete', { params: { path: { upload_id: id } }, signal }));
      if (result.state === 'ready') return result.file;
      await pause(result.retry_after_ms, signal);
    }
  },
  cancel: async (id) => { await ok(api.DELETE('/uploads/sessions/{upload_id}', { params: { path: { upload_id: id } } })); },
  release: releaseUpload,
};

function beginUpload(file: File, signal: AbortSignal, key: string, to?: UploadDestination): Promise<UploadSessionCreateReply> {
  return ok(api.POST('/uploads/sessions', { body: { name: file.name, size: file.size,
    ...(file.type === '' ? {} : { media_type: file.type }), ...(to === undefined ? {} : { to }) }, signal, ...repeatable(key) }));
}

let browserWindow: { reader: number | undefined; value: TransferWindow } | undefined;
function browserTransport(to?: UploadDestination): UploadTransport {
  const reader = client.session.getState().readerVersion;
  if (browserWindow?.reader !== reader) browserWindow = { reader, value: new TransferWindow() };
  browserWindow ??= { reader, value: new TransferWindow() };
  return { ...transport, begin: (file,signal,key)=>beginUpload(file,signal,key,to), window: browserWindow.value, current: () => client.session.getState().readerVersion === reader };
}

function abortReason(signal: AbortSignal): Error {
  const reason: unknown = signal.reason;
  return reason instanceof Error ? reason : new DOMException('Upload cancelled', 'AbortError');
}
function pause(ms: number, signal: AbortSignal): Promise<void> {
  signal.throwIfAborted();
  return new Promise((resolve, reject) => {
    const finish = () => { signal.removeEventListener('abort', cancel); resolve(); };
    const timer = setTimeout(finish, ms);
    const cancel = () => { clearTimeout(timer); reject(abortReason(signal)); };
    signal.addEventListener('abort', cancel, { once: true });
  });
}
function checkedPolicy(policy: TransferPolicy): TransferPolicy {
  if (Object.values(policy).some((value) => !Number.isSafeInteger(value) || value <= 0)
    || policy.min_piece_bytes > policy.initial_piece_bytes || policy.initial_piece_bytes > policy.max_piece_bytes
    || policy.max_piece_bytes > policy.max_window_bytes) throw new Error('Invalid upload transfer policy');
  return policy;
}
function merged(ranges: UploadRange[]): UploadRange[] {
  const result: UploadRange[] = [];
  for (const range of ranges.toSorted((a, b) => a.start - b.start)) {
    const last = result.at(-1);
    if (last !== undefined && last.start + last.len >= range.start) last.len = Math.max(last.len, range.start + range.len - last.start);
    else result.push({ ...range });
  }
  return result;
}
function missing(ranges: UploadRange[], size: number, maximum: number): UploadRange | null {
  let start = 0;
  for (const range of merged(ranges)) {
    if (start < range.start) return { start, len: Math.min(maximum, range.start - start) };
    start = Math.max(start, range.start + range.len);
  }
  return start < size ? { start, len: Math.min(maximum, size - start) } : null;
}
interface Lease { range: UploadRange; sent: (started: number) => void; release: (received: boolean) => void }
interface Waiting { policy: TransferPolicy; signal: AbortSignal; take: (maximum: number) => UploadRange | null; resolve: (lease: Lease | null) => void; reject: (reason: unknown) => void; cancel: () => void }
/** FIFO byte and request admission; one sample first, then the measured bandwidth-delay window. */
export class TransferWindow {
  private readonly waiting: Waiting[] = [];
  private readonly active = new Map<Lease, TransferPolicy>();
  private bytes = 0;
  private rate = 0;
  private sends: { start: number; firstEnd: number; end: number; bytes: number } | undefined;
  private rtt = Number.POSITIVE_INFINITY;
  private epoch = 0;
  private lastAck: number | undefined;
  private burstBase = 0;
  private burstBytes = 0;
  noteRoundTrip(ms: number): void { this.rtt = Math.max(0, ms); }
  stoodStill(): void { this.rate = 0; this.sends = undefined; }
  acquire(policy: TransferPolicy, signal: AbortSignal, take: Waiting['take']): Promise<Lease | null> {
    signal.throwIfAborted();
    return new Promise((resolve, reject) => {
      const entry: Waiting = { policy, signal, take, resolve, reject, cancel: () => {
        const index = this.waiting.indexOf(entry);
        if (index >= 0) this.waiting.splice(index, 1);
        reject(abortReason(signal)); this.pump();
      } };
      signal.addEventListener('abort', entry.cancel, { once: true });
      this.waiting.push(entry); this.pump();
    });
  }
  private pump(): void {
    while (this.waiting.length > 0) {
      const entry = this.waiting[0];
      if (entry === undefined) return;
      const { policy } = entry;
      const sends = this.sends;
      const sendRate = sends === undefined ? 0 : sends.bytes * 1000 / Math.max(1, sends.end - sends.start);
      const rate = Math.max(sendRate, this.rate);
      const requests = rate > 0 ? Math.min(policy.requests, ...[...this.active.values()].map((held) => held.requests)) : Math.min(policy.initial_requests, policy.requests, ...[...this.active.values()].map((held) => Math.min(held.initial_requests, held.requests)));
      const ceiling = Math.min(policy.max_window_bytes, ...[...this.active.values()].map((held) => held.max_window_bytes));
      const window = rate > 0 ? Math.min(ceiling, Math.max(policy.min_piece_bytes, Math.ceil(rate * (policy.target_window_ms + this.rtt * policy.window_rtts) / 1000))) : Math.min(ceiling, policy.initial_piece_bytes * Math.min(policy.initial_requests, policy.requests));
      const freeSlots = requests - this.active.size;
      const freeBytes = window - this.bytes;
      if (freeSlots <= 0 || freeBytes < policy.min_piece_bytes) return;
      // Existing requests keep their original sizes when the measured window grows.
      // Divide the remaining bytes among the slots that can still use them.
      const piece = rate > 0
        ? Math.min(policy.max_piece_bytes, Math.max(policy.min_piece_bytes, Math.floor(freeBytes / freeSlots)))
        : policy.initial_piece_bytes;
      const range = entry.take(Math.min(piece, freeBytes));
      this.waiting.shift(); entry.signal.removeEventListener('abort', entry.cancel);
      if (range === null) { entry.resolve(null); continue; }
      if (this.active.size === 0) { this.epoch = performance.now(); this.lastAck = undefined; this.burstBase = this.epoch; this.burstBytes = 0; }
      const lease: Lease = { range, sent: (started) => {
        if (!this.active.has(lease) || entry.signal.aborted) return;
        const end = performance.now();
        // Sends overlapping the first completion share one link. Their combined bytes
        // crossed it during one interval, before the receiver finished its durable writes.
        if (this.sends === undefined || started >= this.sends.firstEnd) {
          this.sends = { start: started, firstEnd: end, end, bytes: range.len };
        } else {
          this.sends.start = Math.min(this.sends.start, started);
          this.sends.end = end;
          this.sends.bytes += range.len;
        }
        this.pump();
      }, release: (received) => {
        if (!this.active.delete(lease)) return;
        this.bytes -= range.len;
        if (this.active.size === 0) this.sends = undefined;
        if (received) {
          // Batched ACKs count together over a real interval, rather than inventing zero-time bandwidth.
          const now = performance.now();
          if (this.lastAck !== undefined && now - this.lastAck < 1) this.burstBytes += range.len;
          else { this.burstBase = this.lastAck ?? this.epoch; this.burstBytes = range.len; this.lastAck = now; }
          this.rate = this.burstBytes * 1000 / Math.max(1, now - this.burstBase);
        }
        this.pump();
      } };
      this.active.set(lease, policy); this.bytes += range.len; entry.resolve(lease);
    }
  }
}
const windows = new WeakMap<UploadTransport, TransferWindow>();

/** One file keeps durable server coverage across Retry and joins every request before cleanup. */
export class UploadTask {
  private session: UploadSessionCreateReply | null = null;
  private ranges: UploadRange[] = [];
  private completed: UploadedBlob | null = null;
  private completing = false;
  private pending: Promise<UploadedBlob> | null = null;
  private running = false;
  private readonly window: TransferWindow;
  readonly file: File;
  private readonly requests: UploadTransport;
  private creationKey = newActKey();

  constructor(file: File, requests?: UploadTransport, to?: UploadDestination) {
    this.file = file; this.requests = requests ?? browserTransport(to);
    let window = this.requests.window ?? windows.get(this.requests);
    if (window === undefined) { window = new TransferWindow(); windows.set(this.requests, window); }
    this.window = window;
  }
  get uploadId(): string | null { return this.session?.upload_id ?? null; }

  run(signal: AbortSignal, progress: (received: number) => void = () => undefined, phase: (value: 'uploading' | 'verifying') => void = () => undefined): Promise<UploadedBlob> {
    if (this.running && this.pending !== null) return this.pending;
    this.running = true;
    this.pending = this.upload(signal, progress, phase).finally(() => { this.running = false; });
    return this.pending;
  }
  async cancel(options: { keepCompleted?: boolean } = {}): Promise<void> {
    await this.pending?.catch(() => undefined);
    if (this.requests.current?.() === false) return;
    if (this.completed === null && this.completing && this.session !== null) {
      try {
        const status = await this.requests.status(this.session.upload_id, new AbortController().signal);
        this.completed = status.completed ?? null;
      } catch { /* Cancellation still uses its own owner-checked writer when the receipt cannot be read. */ }
    }
    if (this.completed !== null) {
      if (options.keepCompleted !== true) {
        if (this.completed.relay) await this.requests.cancel?.(this.completed.relay);
        else await this.requests.release?.(this.completed.blob);
      }
    } else if (this.session !== null) {
      try { await this.requests.cancel?.(this.session.upload_id); }
      catch (error) { if (!(error instanceof ApiError) || !error.is('not_found')) throw error; }
    }
  }
  private current(signal: AbortSignal): void {
    signal.throwIfAborted();
    if (this.requests.current?.() === false) throw new DOMException('Upload reader changed', 'AbortError');
  }
  private async upload(external: AbortSignal, progress: (received: number) => void, phase: (value: 'uploading' | 'verifying') => void): Promise<UploadedBlob> {
    this.current(external);
    if (this.completed !== null) return this.completed;
    if (!this.completing && this.session !== null && Date.parse(this.session.expires_at) <= Date.now()) {
      this.session = null; this.ranges = []; this.completing = false; this.creationKey = newActKey();
    }
    const started = performance.now();
    if (this.session === null) this.session = await this.requests.begin(this.file, external, this.creationKey);
    else {
      let status: UploadSessionStatus;
      try { status = await this.requests.status(this.session.upload_id, external); }
      catch (error) {
        if (!(error instanceof ApiError) || !(error.is('gone') || error.is('not_found'))) throw error;
        this.session = null; this.ranges = []; this.completing = false; this.creationKey = newActKey();
        return this.upload(external, progress, phase);
      }
      this.accept(status.ranges); this.session.policy = status.policy;
      if (status.completed !== undefined && status.completed !== null) { this.completed = status.completed; progress(this.file.size); this.current(external); return status.completed; }
    }
    phase('uploading');
    this.window.noteRoundTrip(performance.now() - started);
    const policy = checkedPolicy(this.session.policy);
    const id = this.session.upload_id;
    const stop = new AbortController();
    const signal = AbortSignal.any([external, stop.signal]);
    const reserved: UploadRange[] = [];
    let failed: unknown;
    const report = () => progress(this.ranges.reduce((sum, range) => sum + range.len, 0));
    const worker = async () => {
      let failures = 0;
      for (;;) {
        this.current(signal);
        const lease = await this.window.acquire(policy, signal, (maximum) => {
          const range = missing([...this.ranges, ...reserved], this.file.size, maximum);
          if (range !== null) reserved.push(range);
          return range;
        });
        if (lease === null) return;
        let received = false;
        try {
          const range = lease.range;
          const {received: total, expires_at} = await this.requests.put(id, range.start, this.file.slice(range.start, range.start + range.len), signal, lease.sent);
          if (!Number.isSafeInteger(total) || total < range.len || total > this.file.size) throw new Error('Invalid upload progress');
          if (this.session && Date.parse(expires_at)>Date.parse(this.session.expires_at)) this.session.expires_at=expires_at;
          this.ranges = merged([...this.ranges, range]); report(); received = true; failures = 0;
        } catch (error) {
          this.current(signal);
          if (!(error instanceof NetworkError) || failures >= READ_RETRY_PAUSES_MS.length) throw error;
          if (error.failure === 'silent') this.window.stoodStill();
          await pause(READ_RETRY_PAUSES_MS[failures] ?? 0, signal); failures += 1;
          try {
            const status = await this.requests.status(id, signal);
            this.accept([...this.ranges, ...status.ranges]); report();
          } catch (statusError) { if (!(statusError instanceof NetworkError)) throw statusError; }
        } finally {
          const index = reserved.indexOf(lease.range);
          if (index >= 0) reserved.splice(index, 1);
          lease.release(received);
        }
      }
    };
    report();
    await Promise.allSettled(Array.from({ length: Math.min(policy.requests, Math.ceil(policy.max_window_bytes / policy.min_piece_bytes), Math.max(1, Math.ceil(this.file.size / policy.min_piece_bytes))) }, async () => {
      try { await worker(); } catch (error) { if (!stop.signal.aborted) { failed = error; stop.abort(error); } }
    }));
    if (failed !== undefined) throw failed instanceof Error ? failed : new Error("Upload failed", { cause: failed });
    this.current(external);
    if (missing(this.ranges, this.file.size, this.file.size) !== null) throw new Error('Upload coverage is incomplete');
    this.completing = true;
    phase('verifying');
    this.completed = await this.requests.complete(id, external);
    this.current(external);
    return this.completed;
  }
  private accept(ranges: UploadRange[]): void {
    if (ranges.some((range) => !Number.isSafeInteger(range.start) || !Number.isSafeInteger(range.len)
      || range.start < 0 || range.len <= 0 || range.start + range.len > this.file.size)) throw new Error('Invalid upload ranges');
    this.ranges = merged(ranges);
  }
}

/** Removing a private attachment releases its upload grant; saved messages keep their own pins. */
export async function releaseUpload(blob: string, attachment?: { thread: string; path: string }): Promise<void> {
  await ok(api.DELETE('/uploads/blobs/{hash}', { params: { path: { hash: blob }, ...(attachment === undefined ? {} : { query: attachment }) } }));
}

/** Image types both conversation contracts accept; other images travel as ordinary files. */
export function imageType(type: string): ImageMediaType | null {
  return type === 'image/png' || type === 'image/jpeg' || type === 'image/webp' || type === 'image/gif' ? type : null;
}

/** Publishes a finished upload as a thread part, which the engine delivers before the turn. */
export async function attachToThread(id: string, uploaded: UploadedBlob, signal?: AbortSignal, replace?: string): Promise<ContentPart> {
  const media = imageType(uploaded.media_type);
  const attached = await ok(api.POST('/threads/{id}/attachments', {
    params: { path: { id } }, body: '', bodySerializer: () => JSON.stringify({ blob: uploaded.blob, name: uploaded.name, ...(replace === undefined ? {} : { replace }) }), headers: { 'Content-Type': 'application/json' }, ...(signal === undefined ? {} : { signal }),
  }));
  return media === null ? { type: 'file', ...attached } : { type: 'image', blob: uploaded.blob, media_type: media, name: attached.name, path: attached.path };
}

/** Publishes a finished upload as a chat part. */
export async function attachToChat(id: string, uploaded: UploadedBlob, signal?: AbortSignal): Promise<ChatPart> {
  const attached = await ok(api.POST('/chats/{id}/attachments', {
    params: { path: { id } }, body: '', bodySerializer: () => JSON.stringify({ blob: uploaded.blob, name: uploaded.name }), headers: { 'Content-Type': 'application/json' }, ...(signal === undefined ? {} : { signal }),
  }));
  const media = imageType(attached.media_type);
  return media === null ? { type: 'attachment', ...attached } : { type: 'image', blob: attached.blob, name: attached.name, media_type: media };
}

/** Adds a finished upload to a document's files without silently overwriting an existing name. */
export function attachToDocument(id: string, uploaded: UploadedBlob, signal?: AbortSignal): Promise<DocumentFile> {
  return ok(api.POST('/documents/{id}/files', {
    params: { path: { id }, query: {} }, body: '', bodySerializer: () => JSON.stringify({ blob: uploaded.blob, name: uploaded.name }), headers: { 'Content-Type': 'application/json' }, ...(signal === undefined ? {} : { signal }),
  }));
}
