import { treeNode } from '../doubles/tree-node';
/** The tree of an org as the left bar lays it out, the store that keeps it, and where a visitor may be sent after signing in. */
import { describe, expect, it } from 'vitest';

import { buildRows, firstExpanded, type NavItem } from '@/features/navigation/NodeTree';
import type { TreeNode } from '@/generated/api-types';
import { type OrgTree, pathTo, treeWriter, useTreeStore } from '@/realtime/stores/tree';
import { safeRedirect } from '@/routes/-access';
import type { TreeRow } from '@/ui/Tree';
import { display } from '../doubles/thread-display';

const node = (id: string, kind: TreeNode['kind'], parent: string | null, more: Partial<TreeNode> = {}): TreeNode => treeNode({ id, kind, parent_id: parent, tree_seq: 1, topic: kind === 'topic' ? { status: 'open', tag_ids: [] } : null, ...more });

const thread = (id: string, parent: string, state: NonNullable<TreeNode['thread']>['state']): TreeNode =>
  node(id, 'thread', parent, { thread: { state, display: display(state === 'starting' ? 'running' : state), blocked_reason: null, pending_approvals: 0, activity_seq: 1, last_activity_at: null, attention: { uncommitted: null, unpushed: null, failed: false, stale: true, as_of: null, reason: null } } });

const treeOf = (nodes: TreeNode[]): OrgTree => ({ nodes: new Map(nodes.map((entry) => [entry.id, entry])), lastSeq: 1, loaded: true, error: null });

const acme = treeOf([
  node('acme', 'org', null),
  node('web', 'project', 'acme'),
  node('research', 'org', 'acme'),
  node('fix login', 'topic', 'web'),
  node('upgrade', 'topic', 'web'),
  node('migrated', 'topic', 'web', { topic: { status: 'done', tag_ids: [] } }),
  node('old idea', 'topic', 'web', { archived: true }),
  node('removed', 'topic', 'web', { deleted: true }),
  thread('main', 'fix login', 'running'),
  thread('repro', 'fix login', 'waiting_approval'),
  thread('quiet', 'upgrade', 'idle'),
  node('ml', 'project', 'research'),
]);

const names = (rows: TreeRow<NavItem>[]) => rows.map((row) => `${'  '.repeat(row.depth)}${row.item.type === 'node' ? row.item.node.name : row.item.type === 'doc' ? row.item.doc.title : '…'}`);

describe('the tree of an org in the left bar', () => {
  it('shows orgs before projects, each level by name, and only what is open', () => {
    expect(names(buildRows(acme, 'acme', new Set(), 'en'))).toEqual(['research', 'web']);
    expect(names(buildRows(acme, 'acme', new Set(['web', 'research']), 'en'))).toEqual([
      'research',
      '  ml',
      'web',
      '  fix login',
      '  migrated',
      '  upgrade',
    ]);
  });

  it('keeps a finished topic where its name puts it among the others, and leaves out what is archived or deleted', () => {
    const open = buildRows(acme, 'acme', new Set(['web']), 'en');

    expect(names(open)).toEqual(['research', 'web', '  fix login', '  migrated', '  upgrade']);
    expect(names(open).join()).not.toContain('old idea');
    expect(names(open).join()).not.toContain('removed');
  });

  it('shows on a topic the most urgent thing among its threads', () => {
    const rows = buildRows(acme, 'acme', new Set(['web', 'fix login']), 'en');
    const urgent = (id: string) => {
      const row = rows.find((candidate) => candidate.id === id);
      return row?.item.type === 'node' ? row.item.urgent : undefined;
    };

    // Waiting for an approval is more urgent than running.
    expect(urgent('fix login')).toBe('waiting_approval');
    expect(urgent('upgrade')).toBeNull();
    expect(urgent('main')).toBe('running');
  });

  it('opens orgs and projects the first time somebody sees it, so that topics are in view', () => {
    expect(firstExpanded(acme).sort()).toEqual(['acme', 'ml', 'research', 'web']);
  });

  it('knows the way from the root org down to a node', () => {
    expect(pathTo(acme, 'main').map((entry) => entry.id)).toEqual(['acme', 'web', 'fix login', 'main']);
    expect(pathTo(acme, 'nowhere')).toEqual([]);
  });
});

describe('the rows the live feed delivers', () => {
  it('replace what is held only when they are newer', () => {
    treeWriter.clear();
    treeWriter.reset('acme', [node('web', 'project', 'acme', { tree_seq: 5, name: 'Web' })], 5);

    treeWriter.apply('acme', [node('web', 'project', 'acme', { tree_seq: 4, name: 'an older name' })], 5);
    expect(useTreeStore.getState().byOrg.get('acme')?.nodes.get('web')?.name).toBe('Web');

    treeWriter.apply('acme', [node('web', 'project', 'acme', { tree_seq: 6, name: 'Website' })], 6);
    expect(useTreeStore.getState().byOrg.get('acme')).toMatchObject({ lastSeq: 6 });
    expect(useTreeStore.getState().byOrg.get('acme')?.nodes.get('web')?.name).toBe('Website');
  });

  it('never take a node away by themselves: one that was deleted arrives as a row that says so', () => {
    treeWriter.clear();
    treeWriter.reset('acme', [node('acme', 'org', null), node('web', 'project', 'acme')], 2);

    treeWriter.apply('acme', [node('web', 'project', 'acme', { tree_seq: 3, deleted: true })], 3);

    const tree = useTreeStore.getState().byOrg.get('acme');
    expect(tree?.nodes.has('web')).toBe(true);
    expect(tree === undefined ? [] : names(buildRows(tree, 'acme', new Set(), 'en'))).toEqual([]);
  });

  it('keep what was there when a later load fails, to be read greyed', () => {
    treeWriter.clear();
    treeWriter.reset('acme', [node('web', 'project', 'acme')], 1);

    treeWriter.failed('acme', new Error('the line went'));

    expect(useTreeStore.getState().byOrg.get('acme')).toMatchObject({ loaded: true });
    expect(useTreeStore.getState().byOrg.get('acme')?.nodes.size).toBe(1);
    expect(useTreeStore.getState().byOrg.get('acme')?.error).toBeInstanceOf(Error);
  });
});

describe('where a visitor goes on to after signing in', () => {
  it('is a path inside the application', () => {
    expect(safeRedirect('/thread/42?item=7#end')).toBe('/thread/42?item=7#end');
    expect(safeRedirect('/')).toBe('/');
  });

  it('is never an address elsewhere, however it is dressed', () => {
    for (const elsewhere of ['https://evil.example/', '//evil.example/', '/\\evil.example', 'javascript:alert(1)', 'thread/42', '', 42, null, undefined]) {
      expect(safeRedirect(elsewhere), String(elsewhere)).toBeNull();
    }
  });
});


it('uses the same closed branch facts with archived rows hidden or shown',()=>{
  const folded={has_children:false,waiting:0,unread:false,mentions:0,urgency:null,
    archived:{has_children:true,waiting:1,unread:true,mentions:2,urgency:'running' as const}};
  const held=treeOf([node('root','org',null),node('web','project','root',{folded})]);
  const off=buildRows(held,'root',new Set(),'en',{showArchived:false});
  expect(off.find((row)=>row.id==='web')?.item).toMatchObject({unread:false,mentions:0,waiting:0,urgent:null});
  const on=buildRows(held,'root',new Set(),'en',{showArchived:true});
  expect(on.find((row)=>row.id==='web')?.item).toMatchObject({unread:true,mentions:2,waiting:1,urgent:'running'});
  expect(on.find((row)=>row.id==='web')?.expandable).toBe(true);
});

it('keeps a large closed branch expandable from native evidence without retaining its descendants',()=>{
  treeWriter.clear();
  const root=node('root','org',null,{folded:{has_children:true,small_tree:false,waiting:0,unread:false,mentions:0,urgency:null}});
  const branch=node('branch','project',root.id,{folded:{has_children:true,waiting:0,unread:false,mentions:0,urgency:null}});
  treeWriter.branch(root.id,root.id,[branch],1,[],[root]);
  const held=useTreeStore.getState().byOrg.get(root.id);if(held===undefined)throw new Error('The first level must be retained');
  expect(held.nodes.size).toBe(2);expect(firstExpanded(held)).toEqual([]);
  const rows=buildRows(held,root.id,new Set(),'en');
  expect(rows).toHaveLength(1);expect(rows[0]).toMatchObject({id:branch.id,expandable:true,expanded:false});
  treeWriter.clear();
});
