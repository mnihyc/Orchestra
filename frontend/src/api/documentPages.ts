/** Concurrent first pages of visible navigation places share transport and keep each native refusal. */
import { ApiError, api, client, ok } from '@/api/client';
import type { NodeDocumentPage } from '@/generated/api-types';

/** The native maximum page size bounds one batch as well as an individual list. */
const BATCH_SIZE = 200;
type NativePage = NonNullable<NodeDocumentPage['page']>;
interface Waiting {
  id: string;
  limit: number;
  reader: number | undefined;
  signal: AbortSignal;
  resolve: (page: NativePage) => void;
  reject: (error: unknown) => void;
}
let queued: Waiting[] = [];
let draining = false;

async function send(batch: Waiting[]): Promise<void> {
  const first = batch[0];
  if (first === undefined) return;
  const controller = new AbortController();
  const cancel = () => { if (batch.every((entry) => entry.signal.aborted)) controller.abort(); };
  const signals = new Set(batch.map((entry) => entry.signal));
  for (const signal of signals) signal.addEventListener('abort', cancel, { once: true });
  cancel();
  try {
    const pages = await ok(api.GET('/nodes/{id}/document-pages', {
      params: { path: { id: first.id }, query: { node_ids: batch.map((entry) => entry.id).join(','), limit: first.limit } }, signal: controller.signal,
    }));
    for (const entry of batch) {
      if (entry.signal.aborted || client.session.getState().readerVersion !== entry.reader) {
        entry.reject(new DOMException('The document reader changed', 'AbortError'));
        continue;
      }
      const result = pages.find((page) => page.node_id === entry.id);
      if (result?.page !== null && result?.page !== undefined) entry.resolve(result.page);
      else entry.reject(result?.error === null || result?.error === undefined ? new Error('The document page was omitted') : new ApiError(result.error, null));
    }
  } catch (error) { for (const entry of batch) entry.reject(error); }
  finally { for (const signal of signals) signal.removeEventListener('abort', cancel); }
}

/** One request already fans out to a page of places. Drain further pages without a request burst. */
async function drain(): Promise<void> {
  try {
    while (queued.length > 0) {
      const ready = queued;
      queued = [];
      const groups = new Map<string, Waiting[]>();
      for (const entry of ready) {
        if (entry.signal.aborted || client.session.getState().readerVersion !== entry.reader) { entry.reject(new DOMException('The document reader changed', 'AbortError')); continue; }
        const key = `${entry.reader ?? ''}:${entry.limit}`;
        const group = groups.get(key);
        if (group === undefined) groups.set(key, [entry]);
        else group.push(entry);
      }
      for (const entries of groups.values()) for (let offset = 0; offset < entries.length; offset += BATCH_SIZE) {
        const batch = entries.slice(offset, offset + BATCH_SIZE).filter((entry) => {
          if (!entry.signal.aborted && client.session.getState().readerVersion === entry.reader) return true;
          entry.reject(new DOMException('The document reader changed', 'AbortError'));
          return false;
        });
        if (batch.length > 0) await send(batch);
      }
    }
  } finally { draining = false; }
}

/** Only simultaneous, identical first-page shapes are combined; older/filtered pages stay native. */
export function firstDocumentPage(id: string, limit: number, signal: AbortSignal): Promise<NativePage> {
  const reader = client.session.getState().readerVersion;
  return new Promise((resolve, reject) => {
    queued.push({ id, limit, reader, signal, resolve, reject });
    if (!draining) {
      draining = true;
      queueMicrotask(() => { void drain(); });
    }
  });
}
