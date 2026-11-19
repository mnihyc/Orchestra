/** The order of an org's tree: the reader's own choice kept by the server, the org's default, and how the rows of a parent stand in each. */
import { describe, expect, it } from 'vitest';

import { arranged, choiceOf, effectiveOrder, orderOf, SHIPPED_ORDER, type TreeOrder, withChoice } from '@/features/navigation/treeOrder';
import { buildRows } from '@/features/navigation/treeRows';
import type { TreeNode } from '@/generated/api-types';
import type { OrgTree } from '@/realtime/stores/tree';

const node = (id: string, kind: TreeNode['kind'], parent: string | null, more: Partial<TreeNode> = {}): TreeNode => ({
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
  ...more,
});

// Ids grow with time: 01 is the oldest. Bravo changed last; a department stands before the projects.
const tree: OrgTree = {
  nodes: new Map(
    [
      node('00-acme', 'org', null),
      node('01-charlie', 'project', '00-acme', { name: 'Charlie', tree_seq: 2 }),
      node('02-alpha', 'project', '00-acme', { name: 'Alpha', tree_seq: 3 }),
      node('03-bravo', 'project', '00-acme', { name: 'Bravo', tree_seq: 9 }),
      node('04-research', 'org', '00-acme', { name: 'Research', tree_seq: 1 }),
    ].map((entry) => [entry.id, entry]),
  ),
  lastSeq: 9,
  loaded: true,
  error: null,
};

const names = (order?: TreeOrder) => buildRows(tree, '00-acme', new Set(), 'en', order === undefined ? {} : { order }).map((row) => (row.item.type === 'node' ? row.item.node.name : '?'));

it('puts work with newer rolled-up activity first even when its own summary sequence is older', () => {
  const source: OrgTree = { ...tree, nodes: new Map([
    ['00-acme', node('00-acme', 'org', null)],
    ['active', node('active', 'project', '00-acme', { name: 'Zulu', tree_seq: 1, activity_at: '2026-10-15T09:00:00.000Z' })],
    ['older', node('older', 'project', '00-acme', { name: 'Alpha', tree_seq: 999, activity_at: '2026-10-14T09:00:00.000Z' })],
  ]) };
  expect(buildRows(source, '00-acme', new Set(), 'en', { order: { by: 'changed' } }).map((row) => row.id)).toEqual(['active', 'older']);
});

describe('the rows of a parent', () => {
  it('stand by name, the newest first or the most recently changed first, places of one kind together', () => {
    expect(names()).toEqual(['Research', 'Alpha', 'Bravo', 'Charlie']);
    expect(names({ by: 'name' })).toEqual(['Research', 'Alpha', 'Bravo', 'Charlie']);
    expect(names({ by: 'newest' })).toEqual(['Research', 'Bravo', 'Alpha', 'Charlie']);
    expect(names({ by: 'changed' })).toEqual(['Research', 'Bravo', 'Alpha', 'Charlie']);
  });

  it('stand as the reader put them in their own order, and what they did not put after it, the newest first', () => {
    expect(names({ by: 'custom', custom: { '00-acme': ['01-charlie', '04-research'] } })).toEqual(['Charlie', 'Research', 'Bravo', 'Alpha']);
  });
});

describe('the choice kept for the reader', () => {
  it('is read safely, and the org default stands where the reader chose nothing', () => {
    const state = { sidebar: { folded: ['docs'], order: { a: { by: 'newest' }, b: { by: 'custom', custom: { b: ['x', 2] } }, c: { by: 'sideways' } } } };
    expect(choiceOf(state, 'a')).toEqual({ choice: 'newest', own: { by: 'newest' } });
    expect(choiceOf(state, 'b')).toEqual({ choice: 'custom', own: { by: 'custom', custom: { b: ['x'] } } });
    expect(choiceOf(state, 'c')).toEqual({ choice: 'default', own: null });
    expect(choiceOf(undefined, 'a')).toEqual({ choice: 'default', own: null });
    expect(effectiveOrder(null, orderOf({ by: 'name' }))).toEqual({ by: 'name' });
    expect(effectiveOrder(null, null)).toBe(SHIPPED_ORDER);
  });

  it('changes one org and keeps the rest of the sidebar as the server holds it', () => {
    const sidebar = { folded: ['docs'], last_place: { node_id: 'n', route: '/topic/n' }, order: { a: { by: 'name' }, b: { by: 'newest' } } };
    expect(withChoice(sidebar, 'a', { by: 'changed' })).toEqual({ ...sidebar, order: { a: { by: 'changed' }, b: { by: 'newest' } } });
    expect(withChoice(sidebar, 'a', null)).toEqual({ ...sidebar, order: { b: { by: 'newest' } } });
  });

  it('puts a row dragged among its brothers where it was dropped', () => {
    expect(arranged(['a', 'b', 'c', 'd'], 'd', 'b')).toEqual(['a', 'd', 'b', 'c']);
    expect(arranged(['a', 'b', 'c'], 'a', null)).toEqual(['b', 'c', 'a']);
  });
});
