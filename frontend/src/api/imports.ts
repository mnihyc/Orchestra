/** Import plans, previews, jobs and the person's retained reports use the authenticated API. */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';

import { api, client, ok, repeatable } from '@/api/client';
import { fileNameOf, type Progress } from '@/api/exports';
import type {
  Import, ImportLine, ImportPlan, ImportPreview, ImportPreviewRequest, ImportReportFormat, ImportReportListQuery,
  ImportReportRow, ImportRequest, ImportUndoPlan, ImportView, Job,
} from '@/generated/api-types';

/** A page of report rows fits comfortably in the dialog while keeping later rows available. */
const IMPORT_PAGE = 50;
/** Ten progress updates per second keep fast transfers from driving a render for every chunk. */
const PROGRESS_INTERVAL_MS = 100;

/** The reader can change without a full page reload; no old reader's result is a new reader's cache hit. */
const readerScope = () => {
  const session = client.session.getState();
  return [session.userId, session.readerVersion ?? 0] as const;
};

const identities = new WeakMap<object, number>();
let nextIdentity = 0;
function identity(value: object | null): number | null {
  if (value === null) return null;
  let held = identities.get(value);
  if (held === undefined) { held = ++nextIdentity; identities.set(value, held); }
  return held;
}

export const importKeys = {
  all: ['imports'] as const,
  mine: () => ['imports', 'mine', ...readerScope()] as const,
  one: (id: string) => ['imports', 'one', ...readerScope(), id] as const,
  report: (id: string, filter: ImportReportListQuery) => ['imports', 'report', ...readerScope(), id, filter] as const,
  inspection: (node: string | null, request: ImportRequest | null) => ['imports', 'inspection', ...readerScope(), node, identity(request)] as const,
  plan: (node: string | null, request: ImportRequest | null, view: ImportView) => ['imports', 'plan', ...readerScope(), node, identity(request), view] as const,
  preview: (node: string, request: ImportPreviewRequest) => ['imports', 'preview', ...readerScope(), node, identity(request.source), request.object] as const,
  undo: (id: string) => ['imports', 'undo', ...readerScope(), id] as const,
  forJob: (id: string) => ['imports', 'for-job', ...readerScope(), id] as const,
};

/** Checks an import without writing anything. */
export function planImport(node: string, request: ImportRequest, view: ImportView = {}, signal?: AbortSignal): Promise<ImportPlan> {
  return ok(api.POST('/nodes/{id}/imports/preflight', {
    params: { path: { id: node } }, body: { request, view: { limit: IMPORT_PAGE, ...view } }, ...(signal === undefined ? {} : { signal }),
  }));
}

/** Reads one source item's text for a preview. */
export function previewImport(node: string, request: ImportPreviewRequest, signal?: AbortSignal): Promise<ImportPreview> {
  return ok(api.POST('/nodes/{id}/imports/preview', {
    params: { path: { id: node } }, body: request, ...(signal === undefined ? {} : { signal }),
  }));
}

/** Starts one import; the request identity makes a retry of a lost answer safe. */
export function startImport(node: string, request: ImportRequest): Promise<Job> {
  return ok(api.POST('/nodes/{id}/imports', { params: { path: { id: node } }, body: request, ...repeatable(request.client_id) }));
}

/** The caller's imports, newest first. */
export const myImportsQuery = () => infiniteQueryOptions({
  queryKey: importKeys.mine(),
  gcTime: 0,
  initialPageParam: null as string | null,
  queryFn: ({ pageParam, signal }): Promise<{ items: ImportLine[]; next_cursor: string | null }> => ok(api.GET('/me/imports', {
    params: { query: { limit: IMPORT_PAGE, ...(pageParam === null ? {} : { cursor: pageParam }) } }, signal,
  })),
  getNextPageParam: (last) => last.next_cursor,
  maxPages: 1,
});

/** Checks the recent page only; older results remain reachable through the paged personal history. */
export async function importForJob(job: string, signal?: AbortSignal): Promise<Import | null> {
  const page: { items: ImportLine[]; next_cursor: string | null } = await ok(api.GET('/me/imports', {
    params: { query: { limit: IMPORT_PAGE } }, ...(signal === undefined ? {} : { signal }),
  }));
  const line = page.items.find((item) => item.job_id === job || item.undo_job_id === job);
  if (line === undefined) return null;
  return ok(api.GET('/imports/{id}', { params: { path: { id: line.id } }, ...(signal === undefined ? {} : { signal }) }));
}

/** The names of the files an import was made from, a page at a time and only when they are looked at. */
export const importFileNamesQuery = (id: string, pageSize: number) => infiniteQueryOptions({
  queryKey: [...importKeys.one(id), 'files', pageSize] as const,
  initialPageParam: null as string | null,
  queryFn: ({ pageParam, signal }): Promise<{ items: string[]; next_cursor: string | null }> => ok(api.GET('/imports/{id}/files', {
    params: { path: { id }, query: { limit: pageSize, ...(pageParam === null ? {} : { cursor: pageParam }) } }, signal,
  })),
  getNextPageParam: (last) => last.next_cursor,
});

/** One retained import. */
export const importQuery = (id: string) => queryOptions({
  queryKey: importKeys.one(id),
  gcTime: 0,
  queryFn: ({ signal }): Promise<Import> => ok(api.GET('/imports/{id}', { params: { path: { id } }, signal })),
});

/** A report, with the server's filter and cursor. */
export const importReportQuery = (id: string, filter: ImportReportListQuery) => infiniteQueryOptions({
  queryKey: importKeys.report(id, filter),
  gcTime: 0,
  initialPageParam: null as string | null,
  queryFn: ({ pageParam, signal }): Promise<{ items: ImportReportRow[]; next_cursor: string | null }> => ok(api.GET('/imports/{id}/report', {
    params: { path: { id }, query: { ...filter, limit: IMPORT_PAGE, ...(pageParam === null ? {} : { cursor: pageParam }) } }, signal,
  })),
  getNextPageParam: (last) => last.next_cursor,
  maxPages: 1,
});

/** Reads the undo plan without changing the imported objects. */
export function planImportUndo(id: string, signal?: AbortSignal): Promise<ImportUndoPlan> {
  return ok(api.POST('/imports/{id}/undo/preflight', { params: { path: { id } }, ...(signal === undefined ? {} : { signal }) }));
}

/** Starts undo with one client identity, so a lost answer can be retried safely. */
export function startImportUndo(id: string, key: string): Promise<Job> {
  return ok(api.POST('/imports/{id}/undo', { params: { path: { id } }, body: { client_id: key }, ...repeatable(key) }));
}

/** Writes one response chunk at a time, waiting for the destination before reading another. */
export async function downloadImportReport(id: string, format: ImportReportFormat, progress: (value: Progress) => void, write: (bytes: Uint8Array<ArrayBuffer>) => Promise<void>, signal?: AbortSignal): Promise<{ bytes: number; name: string }> {
  const reply = await api.GET('/imports/{id}/report/download', {
    params: { path: { id }, query: { format } }, parseAs: 'stream', ...(signal === undefined ? {} : { signal }),
  });
  const body = (await ok(Promise.resolve(reply))) as ReadableStream<Uint8Array> | null;
  const length = Number(reply.response.headers.get('Content-Length'));
  const total = Number.isFinite(length) && length > 0 ? length : null;
  const name = fileNameOf(reply.response, `import-report.${format}`);
  progress({ received: 0, total });
  let received = 0, reported = 0, lastReport = performance.now();
  if (body !== null) {
    const reader = body.getReader();
    try {
      for (;;) {
        signal?.throwIfAborted();
        const piece = await reader.read();
        if (piece.done) break;
        signal?.throwIfAborted();
        await write(piece.value as Uint8Array<ArrayBuffer>);
        received += piece.value.byteLength;
        const now = performance.now();
        if (now - lastReport >= PROGRESS_INTERVAL_MS) { progress({ received, total }); reported = received; lastReport = now; }
      }
    } catch (error) {
      await reader.cancel(error).catch(() => undefined);
      throw error;
    } finally {
      reader.releaseLock();
    }
  }
  if (received !== reported) progress({ received, total });
  return { bytes: received, name };
}
