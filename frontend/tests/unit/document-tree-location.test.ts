/** Document breadcrumb order follows typed parent identities, including redacted ancestors. */
import { expect, it } from 'vitest';
import { documentAncestors, documentDirectoryIdentity } from '@/features/documents/treeLocation';
import type { DocumentDirectory, DocumentTreeAncestor } from '@/generated/api-types';

const root: DocumentDirectory = { kind: 'node_docs', id: 'project' };
const folder = (id: string, parent: DocumentDirectory, title: string | null = id): DocumentTreeAncestor => ({
  id, parent, node: 'project', root: 'org', basename: title, title,
  standing: { read: title !== null, write: false, manage: false },
});

it('orders an unordered projected ancestor set without including a selected folder twice', () => {
  const parent = folder('research', root);
  const child = folder('sources', { kind: 'container', id: parent.id });
  const selected = folder('today', { kind: 'container', id: child.id });
  expect(documentAncestors(selected.parent, [child, selected, parent]).map((one) => one.id)).toEqual(['research', 'sources']);
});

it('preserves a redacted ancestor and never substitutes its identifier as a visible name', () => {
  const privateParent = folder('hidden-id', root, null);
  const child = folder('shared', { kind: 'container', id: privateParent.id });
  const path = documentAncestors({ kind: 'container', id: child.id }, [child, privateParent]);
  expect(path.map((one) => one.title)).toEqual([null, 'shared']);
  expect(path[0]?.basename).toBeNull();
});

it('keeps node roots distinct from folders and bounds malformed ancestry without duplicate crumbs', () => {
  expect(documentDirectoryIdentity(root)).not.toBe(documentDirectoryIdentity({ kind: 'container', id: root.id }));
  const first = folder('first', { kind: 'container', id: 'second' });
  const second = folder('second', { kind: 'container', id: 'first' });
  expect(documentAncestors({ kind: 'container', id: 'first' }, [second, first])).toHaveLength(2);
  expect(documentAncestors(root, [second, first])).toEqual([]);
});
