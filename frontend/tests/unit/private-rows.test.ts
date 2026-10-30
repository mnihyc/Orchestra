/** Legacy minimal references remain parseable and evict cached content, but never become administered navigation rows. */
import { describe, expect, it } from 'vitest';

import { splitListed } from '@/api/nodes';
import { buildRows, type NavItem } from '@/features/navigation/treeRows';
import type { HiddenItem, TreeNode } from '@/generated/api-types';
import { type OrgTree, treeWriter, useTreeStore } from '@/realtime/stores/tree';
import type { TreeRow } from '@/ui/Tree';

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

const hidden = (id: string, kind: string, parent: string): HiddenItem => ({
  id,
  parent_id: parent,
  kind,
  name: id,
  owner: null,
  content_hidden: true,
  private: true,
});

const label = (rows: TreeRow<NavItem>[]) =>
  rows.map((row) => `${'  '.repeat(row.depth)}${row.item.type === 'node' ? row.item.node.name : row.item.type === 'private' ? `(private) ${row.item.row.name}` : '…'}`);

describe('private places in the left bar', () => {
  it('splits what a listing brings into readable rows and minimal ones', () => {
    const split = splitListed([{ ...node('web', 'project', 'acme'), content_hidden: false }, hidden('payroll', 'project', 'acme')]);
    expect(split.nodes.map((row) => row.id)).toEqual(['web']);
    expect(split.hidden.map((row) => row.id)).toEqual(['payroll']);
  });

  it('keeps a place in one map or the other, never both, as frames bring it', () => {
    treeWriter.reset('acme', [node('acme', 'org', null), node('web', 'project', 'acme')], 1, [hidden('payroll', 'project', 'acme')]);
    let tree = useTreeStore.getState().byOrg.get('acme');
    expect(tree?.hidden?.has('payroll')).toBe(true);
    // The place became readable: its row replaces the minimal one.
    treeWriter.apply('acme', [node('payroll', 'project', 'acme', { tree_seq: 2 })], 2);
    tree = useTreeStore.getState().byOrg.get('acme');
    expect(tree?.nodes.has('payroll')).toBe(true);
    expect(tree?.hidden?.has('payroll')).toBe(false);
    // And private again: the minimal row takes the readable one's place.
    treeWriter.apply('acme', [], 3, [hidden('payroll', 'project', 'acme')]);
    tree = useTreeStore.getState().byOrg.get('acme');
    expect(tree?.nodes.has('payroll')).toBe(false);
    expect(tree?.hidden?.has('payroll')).toBe(true);
    treeWriter.evict('acme');
  });

  it('does not draw the retired administered rows, including their hidden descendants', () => {
    const tree: OrgTree = {
      nodes: new Map([node('acme', 'org', null), node('web', 'project', 'acme')].map((entry) => [entry.id, entry])),
      hidden: new Map([hidden('payroll', 'project', 'acme'), hidden('audit', 'project', 'acme'), hidden('salaries', 'topic', 'payroll')].map((row) => [row.id, row])),
      lastSeq: 1,
      loaded: true,
      error: null,
    };
    expect(label(buildRows(tree, 'acme', new Set(), 'en'))).toEqual(['web']);
    expect(label(buildRows(tree, 'acme', new Set(['payroll']), 'en'))).toEqual(['web']);
  });
});
