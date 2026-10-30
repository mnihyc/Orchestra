/** Document-only folders share document authority and never mutate the project hierarchy. */
import { queryOptions, type QueryClient } from '@tanstack/react-query';

import { ApiError, api, client, ok, repeatable } from '@/api/client';
import { canonicalDirectoryRequest, documentDirectoryKey, retireDocumentDirectory } from '@/api/documentTreeCache';
import type {
  Document, DocumentDirectory, DocumentTreeItem, DocumentTreeOperationBody,
  DocumentTreePage, DocumentTreePlan, DocumentTreeRead, DocumentTreeReceipt, NewDocumentAccess,
} from '@/generated/api-types';

/** Match the existing document list's visible page size. Only the cursor marks the end. */
export const DOCUMENT_TREE_PAGE_SIZE = 30;

export interface DocumentDirectoryRequest {
  node: string;
  at: DocumentDirectory;
  deleted?: boolean;
  cursor?: string | null;
}

export { documentDirectoryKey } from '@/api/documentTreeCache';

export const documentDirectoryQuery = (request: DocumentDirectoryRequest, cache: QueryClient) => queryOptions({
  queryKey: documentDirectoryKey(request),
  queryFn: async ({ signal }): Promise<DocumentTreePage> => {
    const reader = client.session.getState().readerVersion;
    const selected = canonicalDirectoryRequest(request);
    try {
      const page = await ok(api.GET('/nodes/{id}/document-tree', {
    params: { path: { id: selected.node }, query: {
      limit: DOCUMENT_TREE_PAGE_SIZE,
      parent_container_id: request.at.kind === 'container' ? request.at.id : null,
      cursor: request.cursor ?? null,
      ...(request.deleted === true ? { deleted: 'only' as const } : {}),
    } }, signal,
      }));
      if (signal.aborted || reader !== client.session.getState().readerVersion) throw new DOMException('The document reader changed', 'AbortError');
      return page;
    } catch (error) {
      if (!signal.aborted && error instanceof ApiError && (error.is('authz.denied') || error.is('not_found'))) {
        retireDocumentDirectory(cache, request, reader, error, { settled: documentDirectoryKey(request) });
      }
      throw error;
    }
  },
  retry: false,
});

/** An optional reader scope keeps a retained card from reusing another reader's entry. */
export const documentTreeEntryKey = (item: DocumentTreeItem, retained = false, cacheScope?: string) =>
  ['document-tree-entry', item.kind, item.id, retained, ...(cacheScope === undefined ? [] : [cacheScope])] as const;

export const documentTreeEntryQuery = (item: DocumentTreeItem, cache: QueryClient, retained = false, cacheScope?: string) => queryOptions({
  queryKey: documentTreeEntryKey(item, retained, cacheScope),
  queryFn: async ({ signal }): Promise<DocumentTreeRead> => {
    const reader = client.session.getState().readerVersion;
    try {
      const view = await ok(api.GET('/document-tree/entries/{id}', {
        params: { path: { id: item.id }, query: { kind: item.kind, ...(retained ? { inspect_deleted: true } : {}) } }, signal,
      }));
      if (signal.aborted || reader !== client.session.getState().readerVersion) throw new DOMException('The document reader changed', 'AbortError');
      return view;
    } catch (error) {
      if (!signal.aborted && reader === client.session.getState().readerVersion && error instanceof ApiError && (error.is('authz.denied') || error.is('not_found'))) {
        cache.getQueryCache().find({ queryKey: documentTreeEntryKey(item, retained, cacheScope), exact: true })
          ?.setState({ data: undefined, dataUpdatedAt: 0, error, status: 'error', fetchStatus: 'idle' });
      }
      throw error;
    }
  },
  retry: false,
});

/** Retries reuse the form's client ID. Only deletion carries a previously read complete plan. */
export function organizeDocuments(body: DocumentTreeOperationBody): Promise<DocumentTreeReceipt> {
  return ok(api.POST('/document-tree/operations', { body, ...repeatable(body.client_id) }));
}

export function previewDocumentDeletion(body: DocumentTreeOperationBody): Promise<DocumentTreePlan> {
  return ok(api.POST('/document-tree/delete/preview', { body }));
}

export function createDocumentFolder(at: DocumentDirectory, name: string, key: string): Promise<DocumentTreeReceipt> {
  return organizeDocuments({ command: 'create_folder', items: [], to: at, name, client_id: key, version: null, deletion_plan: null });
}

/** Content creation stays separate from organizing existing documents and folders. */
export function createDocumentInFolder(folder: string, body: { title: string; markdown?: string; access?: NewDocumentAccess }, key: string): Promise<Document> {
  return ok(api.POST('/document-containers/{id}/documents', { params: { path: { id: folder } }, body, ...repeatable(key) }));
}
