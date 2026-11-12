// @vitest-environment node
/** Full membership and a protected confirmation prove the same current document observations. */
import { expect, it } from 'vitest';
import type { DocumentTreeOperationBody, DocumentTreePlan } from '@/generated/api-types';
import { Refusal, World } from '../doubles/world';

function scene() {
  const world = new World(), person = world.people.get('ada');
  const doc = [...world.documents.docs.values()].find(value => value.title === 'Rollout plan');
  if (person === undefined || doc === undefined) throw new Error('A native manageable document is required');
  const request: DocumentTreeOperationBody = { command: 'delete', items: [{ kind: 'document', id: doc.id }], client_id: 'delete-native-observations', deletion_plan: null, to: null, name: null, version: null };
  return { world, person, doc, request };
}

it.each(['full', 'protected'] as const)('accepts an unchanged %s preview through the same native deletion guard', kind => {
  const { world, person, doc, request } = scene();
  const preview = world.documents.handle(person, 'POST', '/document-tree/delete/preview', new URLSearchParams(), request);
  if (preview === null) throw new Error('The native preview is required');
  const plan = preview.body as DocumentTreePlan;
  request.deletion_plan = kind === 'full' ? plan.stamp : world.documents.protectDeletionPlan(person, doc.id);
  const reply = world.documents.handle(person, 'POST', '/document-tree/operations', new URLSearchParams(), request);
  expect(reply?.status).toBe(200); expect(doc.deleted).toBe(true);
});

it.each(['full', 'protected'] as const)('refuses a stale %s preview without deleting the changed document', kind => {
  const { world, person, doc, request } = scene();
  const preview = world.documents.handle(person, 'POST', '/document-tree/delete/preview', new URLSearchParams(), request);
  if (preview === null) throw new Error('The native preview is required');
  const plan = preview.body as DocumentTreePlan;
  request.deletion_plan = kind === 'full' ? plan.stamp : world.documents.protectDeletionPlan(person, doc.id);
  world.documents.handle(person, 'PATCH', `/documents/${doc.id}`, new URLSearchParams(), { title: 'Changed after preview' });
  expect(() => world.documents.handle(person, 'POST', '/document-tree/operations', new URLSearchParams(), request)).toThrow(Refusal);
  expect(doc.deleted).toBe(false);
});

it('does not treat a fabricated confirmation as permission or current observations', () => {
  const { world, person, doc, request } = scene();
  request.deletion_plan = { confirmation: 'not-an-observed-preview' };
  expect(() => world.documents.handle(person, 'POST', '/document-tree/operations', new URLSearchParams(), request)).toThrow(Refusal);
  expect(doc.deleted).toBe(false);
});
