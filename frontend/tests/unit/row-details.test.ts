/** Row place hints use the nearest authorized name while retaining a safe full path. */
import { expect, it } from 'vitest';
import { nearestPlace } from '@/ui/RowDetails';
import type { NodeRef } from '@/generated/api-types';

const path: NodeRef[] = [
  { id: 'org', kind: 'org', name: 'Acme', redacted: false },
  { id: 'project', kind: 'project', name: null, redacted: true },
  { id: 'topic', kind: 'topic', name: 'Fix login', redacted: false },
  { id: 'thread', kind: 'thread', name: 'Main thread', redacted: false },
];
it('names the parent of a node and the container of a document, without adding its own name', () => {
  expect(nearestPlace({ id: 'thread', path, place_label: 'Acme / … / Fix login' })).toEqual({ name: 'Fix login', path: 'Acme / … / Fix login' });
  expect(nearestPlace({ id: 'document', path, place_label: null })).toEqual({ name: 'Main thread', path: 'Acme / … / Fix login / Main thread' });
});
it('omits a missing or redacted parent instead of showing an unhelpful fragment or private name', () => {
  expect(nearestPlace({ id: 'org', path: path.slice(0, 1), place_label: null })).toBeNull();
  expect(nearestPlace({ id: 'topic', path: path.slice(0, 3), place_label: null })).toBeNull();
  expect(nearestPlace({ id: 'document', path: [{ id: 'hidden', name: 'private', redacted: true }], place_label: 'private' })).toBeNull();
});
