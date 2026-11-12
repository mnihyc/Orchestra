/** Native homes determine placement; discovery, participation and attention remain separate facts. */
import { expect, it } from 'vitest';
import { buildRows, GROUPS_ROW, groupRowId, newLayoutMemory, type GroupChat, type TreeView } from '@/features/navigation/treeRows';
import type { TreeNode } from '@/generated/api-types';
import type { OrgTree } from '@/realtime/stores/tree';
import { chat, state } from '../doubles/chat-rows';
import { treeNode } from '../doubles/tree-node';

const places = () => [
  treeNode({ id: 'root', kind: 'org', parent_id: null }),
  treeNode({ id: 'org', kind: 'org', parent_id: 'root' }),
  treeNode({ id: 'project', kind: 'project', parent_id: 'root' }),
  treeNode({ id: 'topic', kind: 'topic', parent_id: 'project' }),
  treeNode({ id: 'thread', kind: 'thread', parent_id: 'topic' }),
];
const tree = (rows = places()): OrgTree => ({ nodes: new Map(rows.map((row) => [row.id, row])), loaded: true, lastSeq: 1, error: null });
const home = (id: string, kind: TreeNode['kind']) => ({ id, kind, name: id, redacted: false });
const group = (id: string, at?: string, kind: TreeNode['kind'] = 'org', more: Partial<GroupChat> = {}): GroupChat => ({
  id, name: id, last_message_seq: 5, my: state(id), can: chat().can,
  ...(at === undefined ? {} : { home: home(at, kind) }), ...more,
});
const expanded = new Set(['org', 'project', 'topic', 'thread', GROUPS_ROW]);
const lay = (groups: GroupChat[], view: TreeView = {}, source = tree(), root = 'root') => buildRows(source, root, expanded, 'en', { chatRows: true, groups, ...view });

it.each(['en', 'zh-CN'])('places root, nested org, project, topic and thread groups exactly once in %s', (locale) => {
  const groups = [group('at-root', 'root'), group('at-org', 'org'), group('at-project', 'project', 'project'), group('at-topic', 'topic', 'topic'), group('at-thread', 'thread', 'thread')];
  const rows = buildRows(tree(), 'root', expanded, locale, { chatRows: true, groups });
  expect(rows.filter((row) => row.item.type === 'group').map((row) => [row.item.type === 'group' && row.item.chat.id, row.parentId, row.depth])).toEqual([
    ['at-org', 'org', 1], ['at-thread', 'thread', 3], ['at-topic', 'topic', 2], ['at-project', 'project', 1], ['at-root', GROUPS_ROW, 1],
  ]);
});

it('moves the existing row to the new native home without keeping a stale copy in layout memory', () => {
  const memory = newLayoutMemory(), source = tree();
  const first = buildRows(source, 'root', expanded, 'en', { chatRows: true, groups: [group('review', 'project', 'project')] }, memory);
  const second = buildRows(source, 'root', expanded, 'en', { chatRows: true, groups: [group('review', 'thread', 'thread')] }, memory);
  expect(first.find((row) => row.id === groupRowId('review'))?.parentId).toBe('project');
  expect(second.filter((row) => row.id === groupRowId('review'))).toHaveLength(1);
  expect(second.find((row) => row.id === groupRowId('review'))?.parentId).toBe('thread');
});

it('finds a group through closed ancestors and in a tree narrowed to its own home', () => {
  const groups = [group('Review', 'thread', 'thread'), group('Other', 'project', 'project')];
  const rows = buildRows(tree(), 'root', new Set(), 'en', { chatRows: true, groups, filter: 'Review' });
  expect(rows.map((row) => row.id)).toEqual(['project', 'topic', 'thread', groupRowId('Review')]);
  expect(lay(groups, { filter: 'Review' }, tree(), 'thread').map((row) => row.id)).toEqual([groupRowId('Review')]);
});

it('uses actual participation and its native ancestor path without treating public visibility as membership', () => {
  const rows = places().map((row) => ({ ...row, participating: false }));
  const known = group('member', 'thread', 'thread');
  const discovery = group('public', 'topic', 'topic', { can: { ...chat().can, participating: false, join: true }, visibility: 'public' });
  expect(lay([known, discovery], {}, tree(rows)).filter((row) => row.item.type === 'group')).toHaveLength(2);
  const focused = lay([known, discovery], { participating: true }, tree(rows));
  expect(focused.map((row) => row.id)).toEqual(['project', 'topic', 'thread', groupRowId('member')]);
  expect(focused.find((row) => row.id === 'thread')?.item).toMatchObject({ node: { participating: false } });
});

it('respects hidden list facts and newer reader state, keeping unread mentions on folded homes once', () => {
  const visible = group('review', 'thread', 'thread', { my: state('review', { mentions_unread: 2 }) });
  const hidden = group('hidden', 'topic', 'topic', { my: state('hidden', { hidden: true }) });
  const folded = buildRows(tree(), 'root', new Set(), 'en', { chatRows: true, groups: [visible, hidden] });
  expect(folded.find((row) => row.id === 'project')?.item).toMatchObject({ unread: true, mentions: 2 });
  const open = lay([visible, hidden]);
  expect(open.find((row) => row.id === groupRowId('hidden'))).toBeUndefined();
  expect(open.find((row) => row.id === groupRowId('review'))?.item).toMatchObject({ unread: true, mentions: 2 });
  expect(open.filter((row) => row.item.type === 'node').every((row) => row.item.type !== 'node' || row.item.mentions === 0)).toBe(true);
  const unhidden = lay([hidden], { chats: { states: new Map([['hidden', state('hidden', { hidden: false })]]), latest: new Map() } });
  expect(unhidden.find((row) => row.id === groupRowId('hidden'))).toBeDefined();
});

it('keeps legacy and unavailable homes in the root section without inventing tree nodes or leaking a path', () => {
  const privateHome = { id: 'unavailable', kind: 'topic' as const, name: 'Private topic', redacted: true };
  const rows = lay([group('legacy'), group('fallback', undefined, 'org', { home: privateHome })]);
  expect(rows.find((row) => row.id === groupRowId('fallback'))?.item).toMatchObject({ chat: { home: privateHome } });
  expect(rows.find((row) => row.id === 'unavailable')).toBeUndefined();
  expect(rows.filter((row) => row.item.type === 'group').map((row) => row.parentId)).toEqual([GROUPS_ROW, GROUPS_ROW]);
  expect(lay([group('legacy')], {}, tree(), 'project').some((row) => row.item.type === 'group')).toBe(false);
});

it('honours archived places and the no-thread/no-chat tree choices without rehoming a group', () => {
  const archived = tree(places().map((row) => row.id === 'thread' ? { ...row, archived: true } : row));
  const groups = [group('review', 'thread', 'thread')];
  expect(lay(groups, {}, archived).some((row) => row.item.type === 'group')).toBe(false);
  expect(lay(groups, { showArchived: true }, archived).some((row) => row.item.type === 'group')).toBe(true);
  expect(lay(groups, { threads: false }).some((row) => row.item.type === 'group')).toBe(false);
  expect(lay(groups, { chatRows: false }).some((row) => row.item.type === 'group')).toBe(false);
});
