/** The move picker only offers live topics connected to the root that supplied them. */
import { expect, it } from 'vitest';
import type { TreeNode } from '@/generated/api-types';
import { placesOfTree } from '@/features/settings/PlacePicker';

const node = (id: string, kind: TreeNode['kind'], parent_id: string | null, archived = false): TreeNode =>
  ({ id, name: id, kind, parent_id, archived, deleted: false, color: null }) as TreeNode;

it('selects topics by their supplying organization and preserves folder ancestry', () => {
  const nodes = [node('first', 'org', null), node('second', 'org', null), node('team', 'project', 'second'),
    node('folder', 'project', 'team'), node('target', 'topic', 'folder'), node('local', 'topic', 'first')];
  const topics = (root: string) => placesOfTree(nodes, ' / ', false, root).filter((row) => row.kind === 'topic').map((row) => row.path);
  expect(topics('first')).toEqual(['first / local']);
  expect(topics('second')).toEqual(['second / team / folder / target']);
});

it('keeps archived branches, deleted topics and disconnected places out of a move', () => {
  const nodes = [node('root', 'org', null), node('archive', 'project', 'root', true), node('hidden', 'topic', 'archive'),
    node('detached', 'topic', 'absent'), { ...node('gone', 'topic', 'root'), deleted: true }, node('live', 'topic', 'root')];
  expect(placesOfTree(nodes, ' / ', false, 'root').map((row) => row.id).sort()).toEqual(['live', 'root']);
});
