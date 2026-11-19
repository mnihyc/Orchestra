/** Uploads in chunks sized to the link, which go on where they stopped.
 * The server chooses the maximum piece and computes the finished hash. A lost piece's answer is
 * recovered by sending its offset again; the server's received count tells us where to continue.
 * A failed task keeps its session for Retry. Cancel stops requests and never publishes a partial file.
 */
import { ApiError, api, client, ok } from '@/api/client';
import type { ChatPart, ContentPart, DocumentFile, ImageMediaType, UploadedBlob, UploadSessionCreateReply } from '@/generated/api-types';
import { NetworkError, READ_RETRY_PAUSES_MS } from '@/lib/requests';

/** Start with half a mebibyte so a slow connection can report progress before sending the maximum. */
const FIRST_PIECE_BYTES = 512 * 1024;
/** Pieces aim for one second of transfer so slow connections remain responsive. */
const PIECE_TARGET_MS = 1_000;
/** Do not shrink below 64 KiB unless the server's maximum is smaller: avoid tiny HTTP requests. */
const PIECE_MIN_BYTES = 64 * 1024;

/** The three requests an upload needs; tests can replace the transport with a lossy line. */
export interface UploadTransport {
  begin: (file: File, signal: AbortSignal) => Promise<UploadSessionCreateReply>;
  put: (id: string, offset: number, bytes: Blob, signal: AbortSignal) => Promise<number>;
  complete: (id: string, signal: AbortSignal) => Promise<UploadedBlob>;
  cancel?: (id: string) => Promise<void>;
  release?: (blob: string) => Promise<void>;
  current?: () => boolean;
}

const transport: UploadTransport = {
  begin: (file, signal) => ok(api.POST('/uploads/sessions', { body: { name: file.name, size: file.size, ...(file.type === '' ? {} : { media_type: file.type }) }, signal })),
  put: async (id, offset, bytes, signal) => (await ok(api.PUT('/uploads/sessions/{upload_id}', {
    params: { path: { upload_id: id }, query: { offset } },
    // Binary bodies are strings in OpenAPI; the serializer sends this slice as bytes without buffering it.
    body: '', bodySerializer: () => bytes, headers: { 'Content-Type': 'application/octet-stream' }, signal,
  }))).received,
  complete: (id, signal) => ok(api.POST('/uploads/sessions/{upload_id}/complete', { params: { path: { upload_id: id } }, signal })),
  cancel: async (id) => { await ok(api.DELETE('/uploads/sessions/{upload_id}', { params: { path: { upload_id: id } } })); },
  release: releaseUpload,
};

function browserTransport(): UploadTransport {
  const reader = client.session.getState().readerVersion;
  return { ...transport, current: () => client.session.getState().readerVersion === reader };
}

function pause(ms: number, signal: AbortSignal): Promise<void> {
  signal.throwIfAborted();
  return new Promise((resolve, reject) => {
    const finish = () => { signal.removeEventListener('abort', cancel); resolve(); };
    const timer = setTimeout(finish, ms);
    const cancel = () => {
      clearTimeout(timer);
      const reason: unknown = signal.reason;
      reject(reason instanceof Error ? reason : new DOMException('Upload cancelled', 'AbortError'));
    };
    signal.addEventListener('abort', cancel, { once: true });
  });
}

/** One file's resumable upload; the same instance is used when the person presses Retry. */
export class UploadTask {
  private session: UploadSessionCreateReply | null = null;
  private received = 0;
  private completed: UploadedBlob | null = null;
  private completing = false;
  private pending: Promise<UploadedBlob> | null = null;

  constructor(readonly file: File, private readonly requests: UploadTransport = browserTransport()) {}

  /** The receipt lets a cancelled workspace batch report measured partial progress. */
  get uploadId(): string | null { return this.session?.upload_id ?? null; }

  run(signal: AbortSignal, progress: (received: number) => void = () => undefined): Promise<UploadedBlob> {
    this.pending = this.upload(signal, progress);
    return this.pending;
  }

  /** Waits for the in-flight piece to settle before removing its session and partial bytes. */
  async cancel(options: { keepCompleted?: boolean } = {}): Promise<void> {
    await this.pending?.catch(() => undefined);
    if (this.requests.current?.() === false) return;
    if (this.completed !== null) {
      // A shared grant may also be needed by another upload of the same bytes.
      if (options.keepCompleted !== true) await this.requests.release?.(this.completed.blob);
    } else if (this.session !== null) await this.requests.cancel?.(this.session.upload_id);
  }

  private async upload(signal: AbortSignal, progress: (received: number) => void): Promise<UploadedBlob> {
    signal.throwIfAborted();
    if (this.requests.current?.() === false) throw new DOMException('Upload reader changed', 'AbortError');
    if (this.completed !== null) return this.completed;
    // Completion closes its session. If its answer was lost, a fresh upload finds the same hash safely.
    if (this.completing || (this.session !== null && Date.parse(this.session.expires_at) <= Date.now())) {
      this.session = null;
      this.received = 0;
      this.completing = false;
    }
    this.session ??= await this.requests.begin(this.file, signal);
    const maximum = this.session.chunk_bytes;
    if (!Number.isSafeInteger(maximum) || maximum <= 0) throw new Error('Invalid upload piece size');
    let pieceSize = Math.min(maximum, FIRST_PIECE_BYTES);
    let failures = 0;
    progress(this.received);
    while (this.received < this.file.size) {
      signal.throwIfAborted();
      if (this.requests.current?.() === false) throw new DOMException('Upload reader changed', 'AbortError');
      const offset = this.received;
      const piece = this.file.slice(offset, Math.min(offset + pieceSize, this.file.size));
      const started = performance.now();
      try {
        const received = await this.requests.put(this.session.upload_id, offset, piece, signal);
        if (!Number.isSafeInteger(received) || received <= offset || received > this.file.size) throw new Error('Invalid upload offset');
        this.received = received;
        progress(received);
        failures = 0;
        const elapsed = Math.max(1, performance.now() - started);
        pieceSize = Math.min(maximum, Math.max(Math.min(maximum, PIECE_MIN_BYTES), Math.round(piece.size * PIECE_TARGET_MS / elapsed)));
      } catch (error) {
        signal.throwIfAborted();
        const received = error instanceof ApiError && error.is('upload.offset') ? error.problem.received : undefined;
        if (typeof received === 'number' && Number.isSafeInteger(received) && received >= 0 && received <= this.file.size && received !== offset) {
          this.received = received;
          progress(received);
          failures = 0;
          continue;
        }
        if (!(error instanceof NetworkError) || failures >= READ_RETRY_PAUSES_MS.length) throw error;
        await pause(READ_RETRY_PAUSES_MS[failures] ?? 0, signal);
        failures += 1;
      }
    }
    signal.throwIfAborted();
    if (this.requests.current?.() === false) throw new DOMException('Upload reader changed', 'AbortError');
    this.completing = true;
    this.completed = await this.requests.complete(this.session.upload_id, signal);
    signal.throwIfAborted();
    return this.completed;
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
