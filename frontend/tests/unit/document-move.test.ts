/** Native deletion witnesses and accepted identities are carried without a second Delete or invented tree map. */
import { QueryClient } from '@tanstack/react-query';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { documentMoveOpening, executeDocumentMove, previewDocumentMoveDeletion, type DocumentMoveBody } from '@/api/documentMove';
import { applyDocumentTreeChange, isDocumentTreeOperation, observeDocumentEntry } from '@/api/documentTreeChanges';
import type { DocumentTreeEntry, DocumentTreeOperationReceipt, DocumentTreeReceipt } from '@/generated/api-types';
const calls = vi.hoisted(() => ({ post: vi.fn<(path: string, options: unknown) => Promise<{ data: unknown; response: Response }>>(), trace: vi.fn<(path: string, options: unknown) => Promise<unknown>>() }));
vi.mock('@/api/client', async (original) => { const actual = await original<typeof Client>(); return { ...actual, api: { ...actual.api, POST: calls.post } }; });
vi.mock('@/api/trace', () => ({ traceJson: calls.trace }));
beforeEach(() => vi.clearAllMocks());
const source = { kind: 'document', id: 'source' } as const, destination = { kind: 'node_docs', id: 'destination' } as const;
const body: DocumentMoveBody = { command: 'move_out', items: [source], to: destination, name: null, version: null, client_id: 'one-move', deletion_plan: { confirmation: 'native-sealed-stamp' } };
it('accepts a protected deletion preview without requiring hidden membership', async () => {
  const data = { projection: 'receipt', deletion_plan: { confirmation: 'sealed-current-membership' } };
  calls.post.mockResolvedValue({ data, response: new Response(null, { status: 200 }) });
  expect(await previewDocumentMoveDeletion([source], 'preview')).toEqual(data);
  expect(calls.post).toHaveBeenCalledExactlyOnceWith('/document-tree/delete/preview', { body: { command: 'delete', items: [source], to: null, name: null, client_id: 'preview', version: null, deletion_plan: null } });
});
it('moves out with the exact native confirmation and one actor-scoped request identity', async () => {
  const data: DocumentTreeOperationReceipt = { projection: 'receipt', operation: 'operation', entries: [{ kind: 'document', id: 'new-doc' }] }; calls.trace.mockResolvedValue(data);
  expect(await executeDocumentMove(body)).toEqual(data); expect(calls.post).not.toHaveBeenCalled();
  expect(calls.trace).toHaveBeenCalledExactlyOnceWith('/document-tree/operations', { method: 'POST', body, key: 'one-move' }); expect(isDocumentTreeOperation(body)).toBe(true);
});
it('keeps same-root identity Move on its ordinary typed native variant', async () => {
  const data: DocumentTreeOperationReceipt = { projection: 'receipt', operation: 'operation', entries: [source] }; calls.post.mockResolvedValue({ data, response: new Response(null, { status: 200 }) });
  expect(await executeDocumentMove({ ...body, command: 'move', deletion_plan: null })).toEqual(data); expect(calls.trace).not.toHaveBeenCalled();
  expect(calls.post).toHaveBeenCalledExactlyOnceWith('/document-tree/operations', expect.objectContaining({ body: { ...body, command: 'move', deletion_plan: null } }));
});
it('opens a single native sparse document identity without a readback requirement', () => {
  expect(documentMoveOpening({ projection: 'receipt', operation: 'op', entries: [{ kind: 'document', id: 'new-doc' }] }, source, destination)).toMatch(/\/doc\/new-doc$/u);
});
it('a sparse nested folder receipt opens the chosen directory rather than guessing its new root', () => {
  const receipt: DocumentTreeOperationReceipt = { projection: 'receipt', operation: 'op', entries: [{ kind: 'container', id: 'child-first' }, { kind: 'container', id: 'actual-new-root' }] };
  expect(documentMoveOpening(receipt, { kind: 'container', id: 'source-folder' }, destination)).toBe('/documents/destination');
  expect(documentMoveOpening(receipt, { kind: 'container', id: 'source-folder' }, { kind: 'container', id: 'chosen-folder' })).toBe('/document-folder/chosen-folder');
});
it('a descriptive receipt uses the native original-to-copy map, independent of array order', () => {
  const receipt: DocumentTreeReceipt = { operation: 'op', entries: [], copied: [{ source: { kind: 'container', id: 'child' }, destination: { kind: 'container', id: 'child-first' } }, { source: { kind: 'container', id: 'source-folder' }, destination: { kind: 'container', id: 'new-root' } }], gaining: 0, losing: 0, restore_until: null, view: { entries: [], ancestors: [] } };
  expect(documentMoveOpening(receipt, { kind: 'container', id: 'source-folder' }, destination)).toBe('/document-folder/new-root');
});

it('a descriptive move-out resets source and nested metadata alongside the native copied identities', async () => {
  const cache = new QueryClient(), root = vi.fn(), child = vi.fn(), other = vi.fn();
  const entry = (kind: 'document' | 'container', id: string): DocumentTreeEntry => ({ item: { kind, id }, node: 'source-node', root: 'source-org', parent: { kind: 'node_docs', id: 'source-node' }, basename: id, title: id, owner: 'owner', revision: 1, files: 0, versions: 0, deleted_at: null, deletion: null });
  const removeRoot = observeDocumentEntry(cache, { kind: 'container', id: 'source-folder' }, root), removeChild = observeDocumentEntry(cache, { kind: 'document', id: 'source-child' }, child), removeOther = observeDocumentEntry(cache, { kind: 'document', id: 'unrelated' }, other);
  await applyDocumentTreeChange(cache, { input: { ...body, items: [{ kind: 'container', id: 'source-folder' }], deletion_plan: { entries: [entry('container', 'source-folder'), entry('document', 'source-child')], authority_epoch: 1, authority_fingerprint: 'current', authority_until: null } }, receipt: { operation: 'op', entries: [], copied: [], gaining: 0, losing: 0, restore_until: null, view: { entries: [], ancestors: [] } } });
  expect(root).toHaveBeenCalledOnce(); expect(child).toHaveBeenCalledOnce(); expect(other).not.toHaveBeenCalled(); removeRoot(); removeChild(); removeOther(); cache.clear();
});
it('a sealed confirmation resets its known root without inventing a hidden descendant inventory', async () => {
  const cache = new QueryClient(), root = vi.fn(), unrelated = vi.fn(); const removeRoot = observeDocumentEntry(cache, source, root), removeOther = observeDocumentEntry(cache, { kind: 'document', id: 'unknown-to-confirmation' }, unrelated);
  await applyDocumentTreeChange(cache, { input: body, receipt: { operation: 'op', entries: [], copied: [], gaining: 0, losing: 0, restore_until: null, view: { entries: [], ancestors: [] } } });
  expect(root).toHaveBeenCalledOnce(); expect(unrelated).not.toHaveBeenCalled(); removeRoot(); removeOther(); cache.clear();
});
