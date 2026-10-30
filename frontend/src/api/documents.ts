/** API client for Documents: blocks and their operations, comments, versions and what changed between them, files, exports and moving.
 *
 * What the screens of documents ask of the API: the documents written beside a node, one document
 * whole (the state behind its live feed), the batches of operations a person's writing is sent as,
 * its versions and what changed between two, its discussions, its export, and the acts that rename,
 * move, delete and bring back a document.
 *
 * A batch carries the `client_id` its sender made once, so that a second attempt after a lost answer
 * changes nothing twice. A batch that is refused because a block changed meanwhile comes back with
 * those blocks as they stand (`StaleBlocks`), and is never sent again as it was.
 */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';

import { ApiError, api, done, ok, repeatable } from '@/api/client';
import { resourceRows, type ResourceRows } from '@/api/listed';
import { firstDocumentPage } from '@/api/documentPages';
import type {
  Document,
  DocumentBlock,
  DocumentChanges,
  DocumentComment,
  DocumentConvertReply,
  DocumentDiscussion,
  DocumentExport,
  DocumentExportChoice,
  DocumentOp,
  DocumentOpsCreateReply,
  DocumentRef,
  HiddenItem,
  DocumentSync,
  DocumentVersion,
  DocumentVersionGetReply,
  Inline,
  NewDocumentAccess,
  NewBlock,
  PagePreview,
} from '@/generated/api-types';

export type {
  Document,
  DocumentBlock,
  DocumentChanges,
  DocumentComment,
  DocumentDiscussion,
  DocumentOp,
  DocumentRef,
  DocumentSync,
  DocumentVersion,
};

export const documentKey = (id: string) => ['document', id] as const;
export const documentsKey = (nodeId: string) => ['documents', nodeId] as const;
export const versionsKey = (id: string) => ['document-versions', id] as const;
export const changesKey = (id: string, from: string, to: number | null) => ['document-changes', id, from, to] as const;

/** How many documents a page of a list asks for. */
export const DOCUMENTS_PAGE = 30;

/** Readable document references with minimal administrative names kept apart for their list frame. */
export interface DocumentPage {
  items: ResourceRows<DocumentRef>;
  next_cursor: string | null;
}

/** Splits the wire union before an editor, tree or export chooser reads a document's content fields. */
export function documentPage(page: { items: readonly unknown[]; next_cursor: string | null }): DocumentPage {
  return { items: resourceRows<DocumentRef>(page.items as readonly (DocumentRef | HiddenItem)[]), next_cursor: page.next_cursor };
}

// ---------------------------------------------------------------- reading

/** The documents written beside a node, the most recently changed first; with `under`, those beside the nodes below it too. */
export const documentsOfNodeQuery = (nodeId: string, options: { under?: boolean; deleted?: boolean; participating?: boolean; batch?: boolean } = {}) =>
  infiniteQueryOptions({
    queryKey: [...documentsKey(nodeId), options.under === true, options.deleted === true, options.participating === true] as const,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) =>
      (options.batch === true && pageParam === null && options.under !== true && options.deleted !== true && options.participating !== true
        ? firstDocumentPage(nodeId, DOCUMENTS_PAGE, signal) : ok(
        api.GET('/nodes/{id}/documents', {
          params: {
            path: { id: nodeId },
            query: {
              limit: DOCUMENTS_PAGE,
              ...(pageParam === null ? {} : { cursor: pageParam }),
              ...(options.under === true ? { under: true } : {}),
              ...(options.participating === true ? { participating: true } : {}),
              ...(options.deleted === true ? { deleted: 'only' as const } : {}),
            },
          },
          signal,
        }),
      )).then(documentPage),
    getNextPageParam: (page) => page.next_cursor,
  });

/** One document with what the caller holds on it, which is what the page needs to decide what it may do. */
export const documentQuery = (id: string) =>
  queryOptions({
    queryKey: documentKey(id),
    queryFn: ({ signal }): Promise<Document> => ok(api.GET('/documents/{id}', { params: { path: { id } }, signal })),
  });

/** The state behind a document's live feed: all of it, or what changed after a sequence. */
export function fetchDocumentSync(id: string, since: number | null, signal?: AbortSignal): Promise<DocumentSync> {
  return ok(
    api.GET('/documents/{id}/sync', {
      params: { path: { id }, query: since === null ? {} : { since } },
      ...(signal === undefined ? {} : { signal }),
    }),
  );
}

// ---------------------------------------------------------------- writing

/** The blocks a refused batch ran into, as they stand now. */
export interface StaleBlocks {
  current: DocumentBlock[];
}

/** The blocks under `current` of a `conflict.stale_revision` answer, or `null` when the error is another. */
export function staleBlocks(error: unknown): StaleBlocks | null {
  if (!(error instanceof ApiError) || !error.is('conflict.stale_revision')) return null;
  const current = (error.problem as { current?: unknown }).current;
  return { current: Array.isArray(current) ? (current as DocumentBlock[]) : [] };
}

/** Sends a batch of operations. It is applied whole or not at all. */
export function postOps(id: string, clientId: string, ops: DocumentOp[], signal?: AbortSignal): Promise<DocumentOpsCreateReply> {
  return ok(
    api.POST('/documents/{id}/ops', {
      params: { path: { id } },
      body: { client_id: clientId, ops },
      ...(signal === undefined ? {} : { signal }),
    }),
  );
}

/** Says how far the caller has had the document on screen. */
export function markSeen(id: string, seq: number): Promise<void> {
  return done(api.POST('/documents/{id}/seen', { params: { path: { id } }, body: { seq } }));
}

export function createDocument(
  nodeId: string,
  body: { title: string; blocks?: NewBlock[]; markdown?: string; access?: NewDocumentAccess },
  key: string,
): Promise<Document> {
  return ok(api.POST('/nodes/{id}/documents', { params: { path: { id: nodeId } }, body, ...repeatable(key) }));
}

/** What a text says as blocks, by the server's one reader, for the document it is pasted into: its files and people are known to it, and the pictures a pasted page carries become files of the document. */
export function convertText(documentId: string, format: 'markdown' | 'html' | 'text', text: string): Promise<DocumentConvertReply> {
  return ok(api.POST('/documents/convert', { body: { format, text, document_id: documentId } }));
}

export function renameDocument(id: string, title: string): Promise<Document> {
  return ok(api.PATCH('/documents/{id}', { params: { path: { id } }, body: { title } }));
}

/** Writes a document's description, which is the writer's from then on; empty takes it away. */
export function describeDocument(id: string, description: string): Promise<Document> {
  return ok(api.PATCH('/documents/{id}', { params: { path: { id } }, body: { description } }));
}

export function deleteDocument(id: string): Promise<void> {
  return done(api.DELETE('/documents/{id}', { params: { path: { id } } }));
}

export function restoreDocument(id: string): Promise<Document> {
  return ok(api.POST('/documents/{id}/restore', { params: { path: { id } }, body: {} }));
}

export function moveDocument(id: string, nodeId: string): Promise<Document> {
  return ok(api.POST('/documents/{id}/move', { params: { path: { id } }, body: { node_id: nodeId } }));
}

/** Who would gain and who lose the document if it moved to another node. */
export function previewMove(id: string, nodeId: string): Promise<PagePreview> {
  return ok(api.POST('/documents/{id}/move/preview', { params: { path: { id } }, body: { node_id: nodeId } }));
}

/** Makes the document private (its owners alone), or lets it follow its node again. */
export function setPrivate(id: string, whole: 'private' | 'reset', expectedRevision?: number): Promise<unknown> {
  return ok(
    api.PATCH('/{object}/{id}/permissions', {
      params: { path: { object: 'documents', id } },
      body: { whole, ...(expectedRevision === undefined ? {} : { expected_revision: expectedRevision }) },
    }),
  );
}

/** Who gains and who loses the document if it were made private, or opened again. */
export function previewPrivate(id: string, whole: 'private' | 'reset'): Promise<PagePreview> {
  return ok(api.POST('/{object}/{id}/permissions/preview', { params: { path: { object: 'documents', id } }, body: { whole } }));
}

// ---------------------------------------------------------------- history

export const VERSIONS_PAGE = 20;

/** The entries of a document's history, newest first, the open span first of all. */
export const versionsQuery = (id: string, author: string | null) =>
  infiniteQueryOptions({
    queryKey: [...versionsKey(id), author] as const,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) =>
      ok(
        api.GET('/documents/{id}/versions', {
          params: { path: { id }, query: { limit: VERSIONS_PAGE, ...(pageParam === null ? {} : { cursor: pageParam }), ...(author === null ? {} : { author }) } },
          signal,
        }),
      ),
    getNextPageParam: (page) => page.next_cursor,
  });

/** What an entry of the history changed: the blocks before and after, and who changed each. */
export const changesQuery = (id: string, from: string, to: number | null) =>
  queryOptions({
    queryKey: changesKey(id, from, to),
    queryFn: ({ signal }): Promise<DocumentChanges> =>
      ok(api.GET('/documents/{id}/changes', { params: { path: { id }, query: { from, ...(to === null ? {} : { to }) } }, signal })),
  });

/** The document as it stood at a version. */
export const versionQuery = (id: string, version: number) =>
  queryOptions({
    queryKey: ['document-version', id, version] as const,
    queryFn: ({ signal }): Promise<DocumentVersionGetReply> =>
      ok(api.GET('/documents/{id}/versions/{version}', { params: { path: { id, version: String(version) } }, signal })),
    staleTime: Number.POSITIVE_INFINITY,
  });

/** Puts a version back: the present state is kept as a version first. */
export function restoreVersion(id: string, version: number): Promise<DocumentSync> {
  return ok(api.POST('/documents/{id}/versions/{version}/restore', { params: { path: { id, version: String(version) } } }));
}

// ---------------------------------------------------------------- discussions

export function startDiscussion(id: string, body: { block_id: string; quote?: string; content: Inline[]; client_id: string }): Promise<DocumentDiscussion> {
  return ok(api.POST('/documents/{id}/discussions', { params: { path: { id } }, body }));
}

export function answerDiscussion(id: string, discussionId: string, content: Inline[], clientId: string): Promise<DocumentComment> {
  return ok(
    api.POST('/documents/{id}/discussions/{discussion_id}/comments', {
      params: { path: { id, discussion_id: discussionId } },
      body: { content, client_id: clientId },
    }),
  );
}

export function resolveDiscussion(id: string, discussionId: string, resolved: boolean): Promise<DocumentDiscussion> {
  return ok(
    api.POST('/documents/{id}/discussions/{discussion_id}/resolve', {
      params: { path: { id, discussion_id: discussionId } },
      body: { resolved },
    }),
  );
}

export interface CommentEditAttempt { clientId: string; expectedSeq: number }

/** A modern edit keeps the original draft revision together with its retry identity. */
export function editComment(id: string, commentId: string, content: Inline[], attempt?: CommentEditAttempt): Promise<DocumentComment> {
  return ok(api.PATCH('/documents/{id}/comments/{comment_id}', { params: { path: { id, comment_id: commentId } },
    body: { content, ...(attempt === undefined ? {} : { client_id: attempt.clientId, expected_seq: attempt.expectedSeq }) } }));
}

/** A stale edit's current native row, projected by this request rather than restored from a local receipt. */
export function staleEditedComment(error: unknown): DocumentComment | null {
  if (!(error instanceof ApiError) || !error.is('conflict.stale_revision')) return null;
  const current: unknown = error.problem.current_comment;
  if (current === null || typeof current !== 'object' || Array.isArray(current)) return null;
  const value = current as Partial<DocumentComment>;
  if (typeof value.id !== 'string' || typeof value.discussion_id !== 'string' || !Array.isArray(value.content)
    || typeof value.seq_updated !== 'number' || !Number.isSafeInteger(value.seq_updated) || value.seq_updated < 0) return null;
  return value as DocumentComment;
}

export function deleteComment(id: string, commentId: string): Promise<void> {
  return done(api.DELETE('/documents/{id}/comments/{comment_id}', { params: { path: { id, comment_id: commentId } } }));
}

export function reactToComment(id: string, commentId: string, emoji: string, on: boolean): Promise<DocumentComment> {
  return ok(api.PUT('/documents/{id}/comments/{comment_id}/reaction', { params: { path: { id, comment_id: commentId } }, body: { emoji, on } }));
}

// ---------------------------------------------------------------- export

/** A file the server wrote, with the name it suggests and what its format could not say. */
export interface ExportedFile {
  blob: Blob;
  name: string;
  /** What the format could not keep, as the server lists it, or nothing. */
  losses: string[];
}

/** Asks the server to write the document as a file. */
export async function exportDocument(
  id: string,
  options: { format: DocumentExport; attachments: DocumentExportChoice; comments: DocumentExportChoice },
): Promise<ExportedFile> {
  const answer = await api.GET('/documents/{id}/export', {
    params: { path: { id }, query: options },
    parseAs: 'blob',
  });
  const blob = await ok(Promise.resolve(answer));
  const header = answer.response.headers.get('Content-Disposition') ?? '';
  const named = /filename\*=UTF-8''([^;]+)/i.exec(header)?.[1] ?? /filename="?([^";]+)"?/i.exec(header)?.[1];
  const losses = (answer.response.headers.get('X-Orch-Export-Losses') ?? '')
    .split(',')
    .map((loss) => loss.trim())
    .filter((loss) => loss !== '');
  return { blob: blob, name: named === undefined ? `document.${options.format}` : decodeURIComponent(named), losses };
}

/** Where the bytes of a file a document holds are served. */
export const documentFileUrl = (id: string, fileId: string, download = false): string =>
  `/api/v1/documents/${id}/files/${fileId}${download ? '?download=true' : ''}`;

/** The request path of a document's file, which the file viewers read through the client. */
export const documentFilePath = (documentId: string, fileId: string, inspectDeleted = false): string =>
  `/documents/${documentId}/files/${fileId}${inspectDeleted ? '?inspect_deleted=true' : ''}`;

/** The request path of the listing of a document file that is an archive. */
export const documentFileArchivePath = (documentId: string, fileId: string, inspectDeleted = false): string =>
  `/documents/${documentId}/files/${fileId}/archive${inspectDeleted ? '?inspect_deleted=true' : ''}`;
