/** Independent current metadata changes placement/standing without replacing document content or inventing parent rights. */
import { expect, it } from 'vitest';
import { documentWithPlacement } from '@/features/documents/placementView';
import type { Document, DocumentTreeRead } from '@/generated/api-types';
import { World } from '../doubles/world';
import { id } from '../doubles/fixtures';

function original(): Document {
  const world = new World(), person = world.people.get('ada'), doc = world.documents.docs.get(id(7, 1));
  if (person === undefined || doc === undefined) throw new Error('The existing document fixture is required');
  return world.documents.full(person, doc);
}
function metadata(doc: Document): DocumentTreeRead {
  const node = { id: 'new-node', kind: 'topic' as const, name: null, redacted: true };
  return { place: { node, path: [{ id: 'private-org', kind: 'org', name: null, redacted: true }, node] }, ancestors: [],
    entries: [{ entry: { item: { kind: 'document', id: doc.id }, parent: { kind: 'container', id: 'folder' }, node: node.id,
      root: 'private-org', title: doc.title, basename: 'Plan.md', owner: doc.owner.id, revision: doc.seq + 1,
      deleted_at: null, deletion: null, files: doc.files.length, versions: doc.version }, standing: { read: true, write: false, manage: false } }] };
}

it('uses the current redacted containing place and exact standing while retaining file/history and content-owned facts', () => {
  const doc = original(), view = metadata(doc), merged = documentWithPlacement(doc, view);
  expect(merged?.node).toEqual(view.place?.node); expect(merged?.path).toEqual(view.place?.path);
  expect(merged?.location).toEqual({ parent: { kind: 'container', id: 'folder' }, basename: 'Plan.md', ancestors: [] });
  expect(merged?.access.permissions).not.toContain('doc.edit'); expect(merged?.access.permissions).not.toContain('doc.manage');
  expect(merged?.access.permissions).toContain('doc.view'); expect(merged?.files).toBe(doc.files);
  expect(merged?.seq).toBe(doc.seq); expect(merged?.title).toBe(doc.title);
});
it('ignores another item and a late earlier-place cut after a newer native move', () => {
  const doc = original(), earlier = metadata(doc); const row = earlier.entries[0];
  if (row === undefined) throw new Error('Metadata is required');
  row.entry.revision = doc.seq - 1;
  expect(documentWithPlacement(doc, earlier)).toBe(doc);
  row.entry.revision = doc.seq + 1; row.entry.item = { kind: 'document', id: 'another-document' };
  expect(documentWithPlacement(doc, earlier)).toBe(doc);
});
