/** One editor's native reference choices, read only after its caret asks for them. */
import { client } from '@/api/client';
import { documentReferencePageQuery } from '@/api/documents';
import { queryClient } from '@/api/query';
import type { Document } from '@/generated/api-types';
import { useTreeStore } from '@/realtime/stores/tree';
import type { PlaceChoice } from './BlockMenu';

export interface ReferenceContext {
  document: Pick<Document, 'id' | 'node' | 'path'>;
  enabled: boolean;
}

export type ReferenceChoice = PlaceChoice | { kind: 'more'; load: () => Promise<ReferenceChoice[]> };

export async function referenceChoices(current: () => ReferenceContext | undefined, query: string): Promise<ReferenceChoice[]> {
  const before = current();
  if (!before?.enabled) return [];
  const reader = client.session.getState().readerVersion;
  const doc = before.document;
  const location = doc.path.map((one) => one.id).join('/');
  const text = query.trim().toLowerCase();
  const valid = () => {
    const after = current();
    return after !== undefined && after.enabled && after.document.id === doc.id && after.document.node.id === doc.node.id
      && after.document.node.redacted === doc.node.redacted && after.document.path.map((one) => one.id).join('/') === location
      && client.session.getState().readerVersion === reader;
  };
  const load = async (cursor: string | null): Promise<ReferenceChoice[]> => {
    if (!valid()) return [];
    const page = doc.node.redacted ? null : await queryClient.query(documentReferencePageQuery(doc.node.id, text, cursor));
    if (!valid()) return [];
    const root = doc.path[0]?.id;
    const tree = root === undefined ? undefined : useTreeStore.getState().byOrg.get(root);
    const nodes: PlaceChoice[] = cursor !== null ? [] : [...(tree?.nodes.values() ?? [])]
      .filter((node) => !node.deleted && node.kind !== 'org')
      .map((node) => ({ id: node.id, kind: 'node', name: node.name, hint: node.kind, ...(node.kind === 'thread' ? { threadId: node.id } : {}) }));
    const documents: PlaceChoice[] = (page?.items ?? [])
      .filter((one) => one.id !== doc.id).map((one) => ({ id: one.id, kind: 'document', name: one.title, hint: 'document' }));
    const choices: ReferenceChoice[] = [...documents, ...nodes].filter((one) => one.name.toLowerCase().includes(text));
    const next = page?.next_cursor;
    // An access-filtered empty page is still incomplete. Continue it without fetching the corpus on each key.
    if (next !== undefined && next !== null) choices.push({ kind: 'more', load: () => load(next) });
    return choices;
  };
  return load(null);
}
