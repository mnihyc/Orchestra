/** One editor's native reference choices, read only after its caret asks for them. */
import { client } from '@/api/client';
import { documentsOfNodeQuery } from '@/api/documents';
import { queryClient } from '@/api/query';
import type { Document } from '@/generated/api-types';
import { useTreeStore } from '@/realtime/stores/tree';
import type { PlaceChoice } from './BlockMenu';

export interface ReferenceContext {
  document: Pick<Document, 'id' | 'node' | 'path'>;
  enabled: boolean;
}

export async function referenceChoices(current: () => ReferenceContext | undefined, query: string): Promise<PlaceChoice[]> {
  const before = current();
  if (!before?.enabled) return [];
  const reader = client.session.getState().readerVersion;
  const doc = before.document;
  const location = doc.path.map((one) => one.id).join('/');
  const page = doc.node.redacted ? null : await queryClient.infiniteQuery(documentsOfNodeQuery(doc.node.id, { under: true }));
  const after = current();
  if (after === undefined || !after.enabled || after.document.id !== doc.id || after.document.node.id !== doc.node.id
    || after.document.node.redacted !== doc.node.redacted || after.document.path.map((one) => one.id).join('/') !== location
    || client.session.getState().readerVersion !== reader) return [];
  const root = doc.path[0]?.id;
  const tree = root === undefined ? undefined : useTreeStore.getState().byOrg.get(root);
  const nodes: PlaceChoice[] = [...(tree?.nodes.values() ?? [])]
    .filter((node) => !node.deleted && node.kind !== 'org')
    .map((node) => ({ id: node.id, kind: 'node', name: node.name, hint: node.kind, ...(node.kind === 'thread' ? { threadId: node.id } : {}) }));
  const documents: PlaceChoice[] = (page?.pages.flatMap((one) => one.items) ?? [])
    .filter((one) => one.id !== doc.id).map((one) => ({ id: one.id, kind: 'document', name: one.title, hint: 'document' }));
  const text = query.trim().toLowerCase();
  // The existing caret menu offers eight choices from the same bounded first page.
  return [...documents, ...nodes].filter((one) => one.name.toLowerCase().includes(text)).slice(0, 8);
}
