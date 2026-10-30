/** Apply current document-operation projections and repair only affected visible directories. */
import type { QueryClient } from '@tanstack/react-query';
import { record } from '@/api/cache';
import { ApiError, client } from '@/api/client';
import { documentDirectoryQuery, documentTreeEntryQuery, type DocumentDirectoryRequest } from '@/api/documentTree';
import { retireDocumentDirectory } from '@/api/documentTreeCache';
import type { DocumentDirectory, DocumentTreeItem, DocumentTreeOperationBody, DocumentTreePage, DocumentTreeRead, DocumentTreeReceipt } from '@/generated/api-types';

export interface DocumentDirectoryChange { documentDirectories: readonly DocumentDirectory[] }
export interface DocumentTreeChange { documentTree: { receipt: DocumentTreeReceipt; input: DocumentTreeOperationBody } }
interface Observer { request: DocumentDirectoryRequest; reset: (request: DocumentDirectoryRequest) => void }
const observers = new WeakMap<QueryClient, Set<Observer>>();
const entryObservers = new WeakMap<QueryClient, Set<{ item: DocumentTreeItem; reset: () => void }>>();
const identity = (item: DocumentDirectory | DocumentTreeItem) => `${item.kind}:${item.id}`;

/** Only mounted, visible readers register; no directory or cursor history is retained here. */
export function observeDocumentDirectory(cache: QueryClient, request: DocumentDirectoryRequest, reset: (request: DocumentDirectoryRequest) => void): () => void {
  let held = observers.get(cache);
  if (held === undefined) { held = new Set(); observers.set(cache, held); }
  const entry = { request, reset }; held.add(entry);
  return () => { held.delete(entry); };
}

export function observeDocumentEntry(cache: QueryClient, item: DocumentTreeItem, reset: () => void): () => void {
  let held = entryObservers.get(cache);
  if (held === undefined) { held = new Set(); entryObservers.set(cache, held); }
  const entry = { item, reset }; held.add(entry);
  return () => { held.delete(entry); };
}

/** Explicit retry restarts the one metadata owner even when a cache-only view supplied the button. */
export function restartDocumentEntry(cache: QueryClient, item: DocumentTreeItem): void {
  for (const observer of entryObservers.get(cache) ?? []) if (identity(observer.item) === identity(item)) observer.reset();
}

export function isDocumentDirectory(value: unknown): value is DocumentDirectory {
  return record(value) && (value.kind === 'node_docs' || value.kind === 'container') && typeof value.id === 'string';
}

export function isDocumentTreeOperation(value: unknown): value is DocumentTreeOperationBody {
  return record(value) && typeof value.client_id === 'string' && typeof value.command === 'string'
    && ['create_folder', 'copy', 'move', 'rename', 'delete', 'restore', 'restore_version'].includes(value.command)
    && Array.isArray(value.items) && (value.to === null || isDocumentDirectory(value.to));
}

export function isDocumentTreeReceipt(value: unknown): value is DocumentTreeReceipt {
  return record(value) && typeof value.operation === 'string' && Array.isArray(value.entries)
    && value.entries.every((entry: unknown) => record(entry) && record(entry.item)
      && (entry.item.kind === 'document' || entry.item.kind === 'container') && typeof entry.item.id === 'string')
    && record(value.view) && Array.isArray(value.view.entries) && Array.isArray(value.view.ancestors);
}

/** Only a concrete affected directory is retired; unopened and unrelated directories cause no reads. */
export async function repairDocumentDirectories(cache: QueryClient, selected: readonly DocumentDirectory[], changed = new Set<string>(), anchors: ReadonlyMap<string, string> = new Map()): Promise<void> {
  const reader = client.session.getState().readerVersion;
  const directories = new Set(selected.map(identity));
  const requests = new Map<string, DocumentDirectoryRequest>();
  for (const query of cache.getQueryCache().getAll()) {
    const [head, node, kind, id, deleted] = query.queryKey;
    if (head !== 'document-directory' || typeof node !== 'string' || typeof id !== 'string'
      || kind !== 'container' && kind !== 'node_docs') continue;
    const page = query.state.data as DocumentTreePage | undefined;
    if (page !== undefined && (changed.has(identity(page.directory.at))
      || page.items.some((row) => changed.has(identity(row.entry.item)))
      || page.ancestors.some((ancestor) => changed.has(`container:${ancestor.id}`)))) directories.add(`${kind}:${id}`);
    requests.set(`${kind}:${id}:${deleted === true}`, { node, at: { kind, id }, deleted: deleted === true });
  }
  // Stop older live callbacks and requests before any asynchronous repair. A late old page
  // must not put an item back after a move, deletion or loss of access.
  const active = new Map<string, DocumentDirectoryRequest>();
  for (const observer of observers.get(cache) ?? []) {
    if (!directories.has(identity(observer.request.at))) continue;
    const request = { ...observer.request, node: observer.request.at.kind === 'container' ? anchors.get(observer.request.at.id) ?? observer.request.node : observer.request.node, cursor: null };
    observer.reset(request);
    active.set(`${identity(request.at)}:${request.deleted === true}`, request);
  }
  for (const request of requests.values()) if (directories.has(identity(request.at))) retireDocumentDirectory(cache, request, reader);
  const repairs: Promise<unknown>[] = [];
  for (const request of active.values()) repairs.push(cache.query(documentDirectoryQuery(request, cache)));
  await Promise.allSettled(repairs);
}

/** Durable entry IDs identify changed objects. Only receipt.view may supply current visible metadata. */
export async function applyDocumentTreeChange(cache: QueryClient, { receipt, input }: DocumentTreeChange['documentTree']): Promise<void> {
  const changed = new Set(receipt.entries.map((entry) => identity(entry.item)));
  for (const observer of entryObservers.get(cache) ?? []) if (changed.has(identity(observer.item))) observer.reset();
  const repairs: Promise<unknown>[] = [];
  for (const query of cache.getQueryCache().getAll()) {
    const [head, kind, id, retained, scope] = query.queryKey;
    if (head !== 'document-tree-entry' || typeof id !== 'string' || kind !== 'document' && kind !== 'container' || !changed.has(`${kind}:${id}`)) continue;
    void query.cancel({ revert: false, silent: true });
    const current = receipt.view.entries.find((row) => row.entry.item.kind === kind && row.entry.item.id === id);
    // A single-entry answer can carry its full current folder context and feed. A multi-entry
    // receipt cannot recreate that independently authorized view from old standing or names.
    const refused = query.state.error instanceof ApiError && (query.state.error.is('authz.denied') || query.state.error.is('not_found'));
    const complete = !refused && receipt.view.entries.length === 1 && current !== undefined
      && receipt.view.place !== undefined && receipt.view.place !== null
      && (kind === 'document' || (receipt.view.directory !== undefined && receipt.view.directory !== null));
    if (complete && (current.entry.deleted_at !== null) === (retained === true)) {
      const previous = query.state.data as DocumentTreeRead | undefined;
      cache.setQueryData(query.queryKey, { ...receipt.view, feed: receipt.view.feed ?? previous?.feed });
    } else {
      query.setState({ data: undefined, dataUpdatedAt: 0, error: null, status: 'pending', fetchStatus: 'idle' });
      if (query.isActive()) repairs.push(cache.query(documentTreeEntryQuery({ kind, id }, cache, retained === true, typeof scope === 'string' ? scope : undefined)));
    }
  }
  repairs.push(repairDocumentDirectories(cache, [
    ...(input.to === null ? [] : [input.to]), ...receipt.view.entries.map((row) => row.entry.parent),
  ], changed, new Map(receipt.view.entries.filter((row) => row.entry.item.kind === 'container').map((row) => [row.entry.item.id, row.entry.node]))));
  // The operation already committed. A failed repair belongs to its reader, not a second write.
  await Promise.allSettled(repairs);
}
