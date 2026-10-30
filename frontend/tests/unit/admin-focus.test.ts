/** Focus narrows server-returned work without giving or taking authority, and keeps a match's real ancestors. */
import { expect, it } from 'vitest';
import { participates, participatingTree } from '@/features/access/participation';
import { readChoices, withChoices } from '@/features/navigation/preferences';
import { buildRows } from '@/features/navigation/treeRows';
import { readParams, filtersOf, mergeParams } from '@/features/search/params';
import { namesKey, searchKey } from '@/api/search';
import type { TreeNode } from '@/generated/api-types';
import type { OrgTree } from '@/realtime/stores/tree';

function node(id: string, parent_id: string | null, participating: boolean): TreeNode & { participating: boolean } {
  return { id, parent_id, participating, kind: parent_id === null ? 'org' : 'project', name: id, address: null,
    color: null, archived: false, deleted: false, restricted: false, tree_seq: 1, topic: null, thread: null, chat: { id: `chat-${id}`, seq: 0 }, docs: 0 };
}

it('reads the participation fact, rather than treating an administered thing as personal participation', () => {
  expect(participates({ participating: true })).toBe(true);
  expect(participates({ access: { participating: true } })).toBe(true);
  expect(participates({ can: { participating: true } })).toBe(true);
  expect(participates({ access: { participating: false, administers: true, permissions: ['project.manage'] } })).toBe(false);
  expect(participates({ administers: true })).toBe(false);
  expect(participates({ id: 'ordinary', access: { administers: true, admin_access: null } })).toBe(false);
  expect(participates({ id: 'native-only', access: { permissions: ['project.view'], admin_access: { place: 'system' } } })).toBe(false);
  expect(participates({ id: 'named-work-below', participating: true, access: { admin_access: { place: 'system' } } })).toBe(true);
});

it('retains the matching branch’s real ancestor and leaves the other branch out only while focused', () => {
  const source: OrgTree = { nodes: new Map([
    ['root', node('root', null, false)], ['department', node('department', 'root', false)],
    ['mine', node('mine', 'department', true)], ['other', node('other', 'root', false)],
  ]), lastSeq: 1, loaded: true, error: null };
  const focused = participatingTree(source);
  expect([...focused.nodes.keys()]).toEqual(['root', 'department', 'mine']);
  expect(focused.nodes.get('mine')).toBe(source.nodes.get('mine'));
  expect(participatingTree(source).nodes).toBe(focused.nodes);
  const open = new Set(['root', 'department']);
  expect(buildRows(source, 'root', open, 'en').map((row) => row.id)).toEqual(['department', 'mine', 'other']);
  expect(buildRows(source, 'root', open, 'en', { participating: true }).map((row) => row.id)).toEqual(['department', 'mine']);
  expect([...source.nodes.keys()]).toContain('other');
});

it('does not materialize the old administered placeholders instead of native readable rows', () => {
  const source: OrgTree = { nodes: new Map([['root', node('root', null, true)], ['private-readable', node('private-readable', 'root', false)]]),
    hidden: new Map([['old-placeholder', { content_hidden: true, private: true, id: 'old-placeholder', kind: 'project', name: 'Old administered row', parent_id: 'root', owner: null }]]),
    lastSeq: 1, loaded: true, error: null };
  const rows = buildRows(source, 'root', new Set(), 'en');
  expect(rows.map((row) => row.id)).toEqual(['private-readable']);
  expect(rows[0]?.item.type).toBe('node');
});

it('keeps focus as a display preference and never stores session admin mode in it', () => {
  const selected = withChoices({ last_org_id: 'team' }, { participating: true });
  expect(readChoices(selected).participating).toBe(true);
  expect(selected.last_org_id).toBe('team');
  expect(selected).not.toHaveProperty('as_admin');
  const cleared = withChoices(selected, { participating: false });
  expect(readChoices(cleared).participating === true).toBe(false);
});

it('keeps focused and unrestricted queries distinct, and retains a newer focus while delayed query text is saved', () => {
  const focused = readParams({ q: 'work', participating: 'true' });
  expect(filtersOf(focused).participating).toBe(true);
  expect(searchKey(filtersOf(focused))).not.toEqual(searchKey({ q: 'work' }));
  expect(namesKey({ q: 'work', kinds: ['project'], participating: true })).not.toEqual(namesKey({ q: 'work', kinds: ['project'] }));
  expect(mergeParams({ q: 'before', participating: 'true' }, { q: 'before' }, { q: 'after' })).toEqual({ q: 'after', participating: 'true' });
});
