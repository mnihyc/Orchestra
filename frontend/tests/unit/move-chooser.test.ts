/** The places a thing can be moved to, as the tree they lie in: each choice after the places above it that are no choice, each of those named once. */
import { describe, expect, it } from 'vitest';

import type { PlaceRef } from '@/features/navigation/places';
import { chooserRows, destinationsFor } from '@/features/org/Move';
import type { TreeNode } from '@/generated/api-types';

const node = (id: string, kind: TreeNode['kind'], parent: string | null): TreeNode => ({
  participating: true,
  id,
  address: null,
  kind,
  name: id,
  parent_id: parent,
  color: null,
  docs: 0,
  archived: false,
  deleted: false,
  restricted: false,
  chat: { id: `chat-${id}`, seq: 0 },
  thread: null,
  topic: kind === 'topic' ? { status: 'open', tag_ids: [] } : null,
  tree_seq: 1,
});

const nodes = new Map(
  [node('acme', 'org', null), node('research', 'org', 'acme'), node('web', 'project', 'research'), node('fix', 'topic', 'web'), node('infra', 'project', 'acme')].map((entry) => [entry.id, entry]),
);

const place = (id: string, kind: PlaceRef['kind'], parentId: string): PlaceRef => ({ id, kind, name: id, parentId, archived: false, status: null, folder: false });

const shape = (moved: PlaceRef) =>
  chooserRows(nodes, destinationsFor(nodes, moved, ' / ')).map((row) =>
    row.type === 'above' ? `${'  '.repeat(row.depth)}(${row.name})` : `${'  '.repeat(row.destination.depth)}${row.destination.name}${row.destination.why === null ? '' : ` [${row.destination.why}]`}`,
  );

describe('the tree of places to move to', () => {
  it('names the orgs above the projects a topic can go to, once each and as no choice', () => {
    expect(shape(place('fix', 'topic', 'web'))).toEqual(['(acme)', '  (research)', '    web [here]', '  infra']);
  });

  it('offers every org and project to a project, with no place named twice', () => {
    expect(shape(place('infra', 'project', 'acme'))).toEqual(['acme [here]', '  research', '    web', '  infra [inside]']);
  });
});
