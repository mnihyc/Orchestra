/** The navigation owns open levels; folded native facts do not need descendant rows. */
import { afterEach, expect, it } from 'vitest';
import { treeWriter, useTreeStore, pathTo } from '@/realtime/stores/tree';
import { buildRows } from '@/features/navigation/treeRows';
import { treeNode } from '../doubles/tree-node';

const root = treeNode({ id: 'root', kind: 'org', parent_id: null });
const project = treeNode({ id: 'project', kind: 'project', parent_id: root.id });
const topic = treeNode({ id: 'topic', kind: 'topic', parent_id: project.id });
const thread = treeNode({ id: 'thread', kind: 'thread', parent_id: topic.id });
const held = () => useTreeStore.getState().byOrg.get(root.id)!;
afterEach(() => treeWriter.clear());

it('closing a branch releases its children and deeper pages while retaining its own folded row', () => {
  treeWriter.branch(root.id, root.id, [project], 1, [], [root], null);
  treeWriter.branch(root.id, project.id, [topic], 1, [], [], 'more-topics');
  treeWriter.branch(root.id, topic.id, [thread], 1);
  expect([...held().nodes.keys()]).toEqual([root.id, project.id, topic.id, thread.id]);
  treeWriter.releaseBranch(root.id, project.id);
  expect([...held().nodes.keys()]).toEqual([root.id, project.id]);
  expect(held().branches?.has(topic.id)).toBe(false);
  expect(held().branches?.has(project.id)).toBe(false);
});

it('a live unread change inside a closed branch replaces its folded facts without reading its rows', () => {
  treeWriter.branch(root.id, root.id, [{ ...project, folded: { children: 50000, waiting: 0, unread: false, mentions: 0, urgency: null } }], 1, [], [root]);
  treeWriter.branch(root.id, root.id, [{ ...project, folded: { children: 50000, waiting: 2, unread: true, mentions: 3, urgency: 'running' } }], 2, [], [root]);
  expect(held().nodes.size).toBe(2);
  const row=buildRows(held(),root.id,new Set(),'en').find((row)=>row.id===project.id);
  expect(row?.expandable).toBe(true);
  expect(row?.item).toMatchObject({ type:'node', unread:true, mentions:3, waiting:2, urgent:'running' });
});

it('a deep address is revealed from its path without retaining unrelated siblings', () => {
  treeWriter.branch(root.id, root.id, [project], 1, [], [root]);
  treeWriter.reveal(root.id, [root, project, topic, thread], 2);
  expect(pathTo(held(),thread.id).map((row)=>row.id)).toEqual(['root','project','topic','thread']);
  treeWriter.releaseBranch(root.id,project.id);
  expect(held().nodes.has(thread.id)).toBe(false);
});

it('replacing one live page removes a moved child without dropping another open branch', () => {
  const other=treeNode({ id:'other',kind:'project',parent_id:root.id });
  treeWriter.branch(root.id,root.id,[project,other],1,[],[root]);
  treeWriter.branch(root.id,project.id,[topic],1);
  treeWriter.branch(root.id,other.id,[{...thread,parent_id:other.id}],2);
  treeWriter.branch(root.id,project.id,[],3);
  expect(held().nodes.has(topic.id)).toBe(false);
  expect(held().nodes.get(thread.id)?.parent_id).toBe(other.id);
});
