/** Native tree defaults have one owner while reader facts and independent namespace stamps stay explicit. */
import { expect, it } from 'vitest';
import type { AdminAccess, TreeNode } from '@/generated/api-types';
import { listedTreeNode, treeNode, treeThread } from '../doubles/tree-node';
import { display } from '../doubles/thread-display';

it.each(['org', 'project', 'topic', 'thread'] as const)('supplies the complete common defaults of a %s without inventing work facts', kind => {
  const row: TreeNode = treeNode({ id: kind, kind, parent_id: null });
  expect(row.docs).toBe(0); expect(row.docs_seq).toBe(0);
  expect(row.thread).toBeNull(); expect(row.topic).toBeNull();
});

it('preserves actual disclosure/participation and keeps document membership independent of a node revision', () => {
  const admin: AdminAccess = { place: 'system', private: true };
  const row = treeNode({ id: 'project', kind: 'project', parent_id: 'org', participating: false, admin_access: admin, tree_seq: 18, docs_seq: 4 });
  expect(row.participating).toBe(false); expect(row.admin_access).toEqual(admin);
  expect(row.tree_seq).toBe(18); expect(row.docs_seq).toBe(4);
});

it('marks disclosed list members explicitly while ordinary native rows keep their own shape', () => {
  const seed = { id: 'topic', kind: 'topic', parent_id: 'project' } as const;
  expect(treeNode(seed)).not.toHaveProperty('content_hidden');
  expect(listedTreeNode(seed)).toHaveProperty('content_hidden', false);
});

it('completes execution-only control input before publishing a native thread row', () => {
  const row = treeThread({ state: 'running', activity_seq: 7, last_activity_at: null, pending_approvals: 0, blocked_reason: null });
  expect(row.display).toMatchObject({ state: 'running', seq: 7, blocked_reason: null });
  expect(row.attention).toEqual({ uncommitted: null, unpushed: null, failed: false, as_of: null, stale: false, reason: null });
});

it('preserves the supplied reader display and independent work attention', () => {
  const presentation = display('held', { held_inputs: 1 });
  const attention = { uncommitted: true, unpushed: false, failed: true, as_of: null, stale: true, reason: null };
  const row = treeThread({ state: 'idle', activity_seq: 7, last_activity_at: null, pending_approvals: 0, blocked_reason: null, display: presentation, attention });
  expect(row.display).toBe(presentation); expect(row.attention).toBe(attention);
});
