/** One document-directory family retires and adopts metadata independently of its mounted views. */
import type { QueryClient, QueryKey } from '@tanstack/react-query';
import { client } from '@/api/client';
import type { DocumentDirectoryRequest } from '@/api/documentTree';
import type { DocumentTreePage } from '@/generated/api-types';

export function canonicalDirectoryRequest(request: DocumentDirectoryRequest): DocumentDirectoryRequest {
  return request.at.kind === 'node_docs' ? { ...request, node: request.at.id } : request;
}

export const documentDirectoryKey = (request: DocumentDirectoryRequest) => {
  const selected = canonicalDirectoryRequest(request);
  return ['document-directory', selected.node, selected.at.kind, selected.at.id, selected.deleted === true, selected.cursor ?? null] as const;
};

function inDirectory(key: QueryKey, request: DocumentDirectoryRequest): boolean {
  return key[0] === 'document-directory' && key[2] === request.at.kind && key[3] === request.at.id && key[4] === (request.deleted === true);
}

/** Erasure is synchronous and applies to every old cursor, even if its last view just left. */
export function retireDocumentDirectory(cache: QueryClient, request: DocumentDirectoryRequest, reader: number | undefined, error?: Error, options: { keep?: QueryKey; settled?: QueryKey } = {}): void {
  if (reader !== client.session.getState().readerVersion) return;
  const keep = options.keep === undefined ? null : JSON.stringify(options.keep);
  const settled = options.settled === undefined ? null : JSON.stringify(options.settled);
  for (const query of cache.getQueryCache().findAll({ predicate: (query) => inDirectory(query.queryKey, request) && JSON.stringify(query.queryKey) !== keep })) {
    // Capture this query instance. Cancellation must not later restore its previous data.
    if (JSON.stringify(query.queryKey) !== settled) void query.cancel({ revert: false, silent: true });
    query.setState({ data: undefined, dataUpdatedAt: 0, error: error ?? null, status: error === undefined ? 'pending' : 'error', fetchStatus: 'idle' });
  }
}

/** A delivered projection supersedes an older pending HTTP cut of the same page. */
export function adoptDocumentDirectory(cache: QueryClient, request: DocumentDirectoryRequest, page: DocumentTreePage, reader: number | undefined, restart: boolean): DocumentDirectoryRequest | null {
  if (reader !== client.session.getState().readerVersion || page.directory.at.kind !== request.at.kind || page.directory.at.id !== request.at.id) return null;
  const selected = canonicalDirectoryRequest({ ...request, node: page.directory.node.id, cursor: restart ? null : request.cursor ?? null });
  const key = documentDirectoryKey(selected);
  if (restart || selected.node !== canonicalDirectoryRequest(request).node) retireDocumentDirectory(cache, request, reader, undefined, { keep: key });
  const previous = cache.getQueryCache().find({ queryKey: key, exact: true });
  if (previous !== undefined) void previous.cancel({ revert: false, silent: true });
  cache.setQueryData(key, page);
  return selected;
}
