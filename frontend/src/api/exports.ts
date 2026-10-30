/** API client for Exports: the plan of an archive, asking for it, the list of one's own and their removal, the archive fetched, the job that writes it, and the single files of a thread, a chat and a document.
 *
 * What taking something away asks of the API. One thing is one request that answers with the file; a place is
 * planned first (nothing is made), then asked for, which starts a job; the job is followed until it ends, and the
 * archive it made is fetched, and fetched again, until it is removed. A file is read into the page and saved from
 * there, because the access token is kept in memory and a plain link would carry none; an archive is read in pieces
 * so that the page can say how far it has come.
 *
 * The request that asks for an archive carries the key of the act, so that a second attempt after a lost answer
 * makes nothing twice. The plan, the list and the job are reads.
 */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';
import { api, done, ok, repeatable } from '@/api/client';
import { liveValue } from '@/api/liveValue';
import { documentPage, type DocumentPage } from '@/api/documents';
import type { ChatExportFiles, Document, DocumentBlock, DocumentExport, DocumentExportChoice, DocumentRef, ExportDetail, ExportFormat, ExportPlan, ExportRequest, Job, Thread, Export as ExportRow } from '@/generated/api-types';

export const exportKeys = {
  all: ['exports'] as const,
  mine: ['exports', 'mine'] as const,
  one: (id: string) => ['exports', 'one', id] as const,
  job: (id: string) => ['job', id] as const,
  plan: (place: string, request: ExportRequest) => ['exports', 'plan', place, request] as const,
  documents: (place: string) => ['exports', 'documents', place] as const,
  document: (id: string) => ['exports', 'document', id] as const,
  blocks: (id: string) => ['exports', 'blocks', id] as const,
  threads: (topic: string) => ['exports', 'threads', topic] as const,
};

/** How many archives a page of the list holds: what a table shows without scrolling far, and no more. */
export const EXPORTS_PAGE = 20;

/** How many documents a page of a place's documents holds: the most a list may ask for at once. */
export const DOCUMENTS_PAGE = 200;

// ---------------------------------------------------------------- an archive of a place

/** What an archive of a place would hold for the caller, and what stands in its way. Nothing is made. */
export function planExport(place: string, request: ExportRequest, signal?: AbortSignal): Promise<ExportPlan> {
  return ok(
    api.POST('/nodes/{id}/exports/preflight', {
      params: { path: { id: place } },
      body: request,
      ...(signal === undefined ? {} : { signal }),
    }),
  );
}

/** Asks for an archive of a place: the job that writes it. `key` is made once, when the person acts. */
export function askForExport(place: string, request: ExportRequest, key: string): Promise<Job> {
  return ok(api.POST('/nodes/{id}/exports', { params: { path: { id: place } }, body: request, ...repeatable(key) }));
}

/** The caller's own archives, newest first, a page at a time. */
export const myExportsQuery = infiniteQueryOptions({
  queryKey: exportKeys.mine,
  initialPageParam: null as string | null,
  queryFn: async ({ pageParam, signal }): Promise<{ items: ExportRow[]; next_cursor: string | null }> =>
    ok(
      api.GET('/me/exports', {
        params: { query: { limit: EXPORTS_PAGE, ...(pageParam === null ? {} : { cursor: pageParam }) } },
        signal,
      }),
    ),
  getNextPageParam: (last) => last.next_cursor,
});

/** One of the caller's archives. */
export const exportQuery = (id: string) =>
  queryOptions({
    queryKey: exportKeys.one(id),
    queryFn: ({ signal }): Promise<ExportRow> => ok(api.GET('/exports/{id}', { params: { path: { id } }, signal })),
  });

/** Removes an archive now; a job still writing it is cancelled. */
export function removeExport(id: string): Promise<void> {
  return done(api.DELETE('/exports/{id}', { params: { path: { id } } }));
}

/** A job, as it stands. */
export const jobQuery = (id: string) =>
  queryOptions({
    queryKey: exportKeys.job(id),
    queryFn: ({ signal }): Promise<Job> => liveValue(exportKeys.job(id), signal),
  });

/** Reads a job once, outside a component. */
export function readJob(id: string, signal?: AbortSignal): Promise<Job> {
  return ok(api.GET('/jobs/{id}', { params: { path: { id } }, ...(signal === undefined ? {} : { signal }) }));
}

/** Cancels the job that writes an archive. */
export function cancelJob(id: string): Promise<Job> {
  return ok(api.POST('/jobs/{id}/cancel', { params: { path: { id } } }));
}

// ---------------------------------------------------------------- what the dialog of a place reads

/** The documents written beside a place and beside everything below it, the most recently changed first, a page at a time. */
export const documentsUnderQuery = (place: string) =>
  infiniteQueryOptions({
    queryKey: exportKeys.documents(place),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<DocumentPage> =>
      ok(
        api.GET('/nodes/{id}/documents', {
          params: { path: { id: place }, query: { under: true, limit: DOCUMENTS_PAGE, ...(pageParam === null ? {} : { cursor: pageParam }) } },
          signal,
        }),
      ).then(documentPage),
    getNextPageParam: (last) => last.items.privateRows.length > 0 ? undefined : last.next_cursor,
  });

/** The most pages of one place's own documents that are read to name them one by one: a place with more than this is not picked round. */
export const OWN_DOCUMENT_PAGES_MAX = 25;

/** Every document written beside one place and no other, read page by page, or `null` when there are more than a request may name. */
export async function readOwnDocuments(place: string, signal?: AbortSignal): Promise<DocumentRef[] | null> {
  const found: DocumentRef[] = [];
  let cursor: string | null = null;
  for (let page = 0; page < OWN_DOCUMENT_PAGES_MAX; page += 1) {
    const here = documentPage(await ok(
      api.GET('/nodes/{id}/documents', {
        params: { path: { id: place }, query: { limit: DOCUMENTS_PAGE, ...(cursor === null ? {} : { cursor }) } },
        ...(signal === undefined ? {} : { signal }),
      }),
    ));
    found.push(...here.items);
    if (here.next_cursor === null || here.items.privateRows.length > 0) return found;
    cursor = here.next_cursor;
  }
  return null;
}

/** The threads of a topic with what the caller holds on each, which the topic's state carries. */
export const topicThreadsQuery = (topic: string) =>
  queryOptions({
    queryKey: exportKeys.threads(topic),
    queryFn: async ({ signal }): Promise<Thread[]> => {
      const state = await ok(api.GET('/topics/{id}/sync', { params: { path: { id: topic } }, signal }));
      return state.threads;
    },
  });

// ---------------------------------------------------------------- files

/** A file as the server wrote it: its bytes, the name to save it under, and what its format could not keep. */
export interface SavedFile {
  blob: Blob;
  name: string;
  /** What the format could not say, as the server lists it; empty for a format that loses nothing. */
  losses: string[];
}

/** The name a download says it should be saved under: the form that carries any character first. */
export function fileNameOf(response: Response, fallback: string): string {
  const header = response.headers.get('Content-Disposition') ?? '';
  const encoded = /filename\*=UTF-8''([^;]+)/i.exec(header)?.[1];
  if (encoded !== undefined) {
    try {
      return decodeURIComponent(encoded);
    } catch {
      // A name that is not valid in that form falls back on the plain one.
    }
  }
  return /filename="?([^";]+)"?/i.exec(header)?.[1] ?? fallback;
}

/** The header in which the server lists what a document's format could not keep. */
const LOSSES_HEADER = 'X-Orch-Export-Losses';

async function fileFrom(answer: Promise<{ data?: unknown; error?: unknown; response: Response }>, fallback: string): Promise<SavedFile> {
  const reply = await answer;
  const blob = (await ok(Promise.resolve(reply))) as Blob;
  const losses = (reply.response.headers.get(LOSSES_HEADER) ?? '')
    .split(',')
    .map((loss) => loss.trim())
    .filter((loss) => loss !== '');
  return { blob, name: fileNameOf(reply.response, fallback), losses };
}

/** What a thread is exported with. */
export interface ThreadFileQuery {
  format: ExportFormat;
  detail: ExportDetail;
  /** The turns to hold, counted in; absent for the first and the last. */
  from?: number;
  to?: number;
  /** What a person's export adds to the written form: the words of `include`, in the order the server names them. */
  include: readonly ('reasoning' | 'sub_streams' | 'chat')[];
}

/** The conversation of a thread as a file. */
export function exportThreadFile(id: string, query: ThreadFileQuery, signal?: AbortSignal): Promise<SavedFile> {
  return fileFrom(
    api.GET('/threads/{id}/export', {
      params: {
        path: { id },
        query: {
          format: query.format,
          detail: query.detail,
          // Always said: left out, the server holds the sub-agents in, and a person who unticked them has asked for none.
          include: query.include.join(','),
          ...(query.from === undefined ? {} : { from: query.from }),
          ...(query.to === undefined ? {} : { to: query.to }),
        },
      },
      parseAs: 'blob',
      ...(signal === undefined ? {} : { signal }),
    }),
    `thread.${query.format === 'json' ? 'json' : 'md'}`,
  );
}

/** What a chat is exported with. */
export interface ChatFileQuery {
  format: ExportFormat;
  files: ChatExportFiles;
  /** The messages to hold, by their numbers, counted in. */
  from?: number;
  to?: number;
}

/** A chat as a file; with its files, an archive of it and them. */
export function exportChatFile(id: string, query: ChatFileQuery, signal?: AbortSignal): Promise<SavedFile> {
  return fileFrom(
    api.GET('/chats/{id}/export', {
      params: {
        path: { id },
        query: {
          format: query.format,
          files: query.files,
          ...(query.from === undefined ? {} : { from: query.from }),
          ...(query.to === undefined ? {} : { to: query.to }),
        },
      },
      parseAs: 'blob',
      ...(signal === undefined ? {} : { signal }),
    }),
    query.files === 'include' ? 'chat.zip' : `chat.${query.format === 'json' ? 'json' : 'md'}`,
  );
}

/** What a document is exported with. */
export interface DocumentFileQuery {
  format: DocumentExport;
  attachments: DocumentExportChoice;
  comments: DocumentExportChoice;
}

/** A document as a file, written by the server. */
export function exportDocumentFile(id: string, query: DocumentFileQuery, signal?: AbortSignal): Promise<SavedFile> {
  return fileFrom(
    api.GET('/documents/{id}/export', {
      params: { path: { id }, query },
      parseAs: 'blob',
      ...(signal === undefined ? {} : { signal }),
    }),
    'document',
  );
}

/** A document with what the caller holds on it and what it holds. */
export const exportedDocumentQuery = (id: string) =>
  queryOptions({
    queryKey: exportKeys.document(id),
    queryFn: ({ signal }): Promise<Document> => ok(api.GET('/documents/{id}', { params: { path: { id } }, signal })),
  });

/** The blocks of a document as they stand, read once, for what a format would lose of them: a document that is not open in the page has none to hand. */
export const exportedBlocksQuery = (id: string) =>
  queryOptions({
    queryKey: exportKeys.blocks(id),
    queryFn: async ({ signal }): Promise<DocumentBlock[]> => (await ok(api.GET('/documents/{id}/sync', { params: { path: { id }, query: {} }, signal }))).blocks,
    // What the person sees of it is read afresh each time the dialog opens: the document may have changed since.
    gcTime: 0,
  });

// ---------------------------------------------------------------- an archive fetched

/** How far a download has come: the bytes read, and the bytes there will be where the server said. */
export interface Progress {
  received: number;
  total: number | null;
}

/**
 * Fetches an archive, in pieces, and tells how far it has come. A fetch that is called off through `signal`
 * ends with the error of a request that was.
 */
export async function downloadExport(id: string, onProgress: (progress: Progress) => void, signal?: AbortSignal): Promise<SavedFile> {
  const reply = await api.GET('/exports/{id}/download', {
    params: { path: { id } },
    parseAs: 'stream',
    ...(signal === undefined ? {} : { signal }),
  });
  const body = (await ok(Promise.resolve(reply))) as ReadableStream<Uint8Array> | null;
  const length = Number(reply.response.headers.get('Content-Length'));
  const total = Number.isFinite(length) && length > 0 ? length : null;
  const name = fileNameOf(reply.response, 'export.zip');
  onProgress({ received: 0, total });
  if (body === null) return { blob: new Blob([], { type: 'application/zip' }), name, losses: [] };
  const reader = body.getReader();
  const pieces: Uint8Array<ArrayBuffer>[] = [];
  let received = 0;
  for (;;) {
    const piece = await reader.read();
    if (piece.done) break;
    pieces.push(piece.value as Uint8Array<ArrayBuffer>);
    received += piece.value.byteLength;
    onProgress({ received, total });
  }
  return { blob: new Blob(pieces, { type: 'application/zip' }), name, losses: [] };
}
