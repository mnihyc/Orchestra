/** A tag's eligible project choice never escapes its defining place or includes hidden project rows. */
import { describe, expect, it } from 'vitest';
import { pathOf } from '@/features/finder/destinations';
import { projectsForTag, tagProjectDestination } from '@/features/finder/TagProjects';
import type { TreeNode } from '@/generated/api-types';

const row = (id: string, kind: TreeNode['kind'], parent_id: string | null): TreeNode => ({ id, kind, name: id, parent_id, color: null, deleted: false }) as TreeNode;

describe('a tag in the finder', () => {
  const tag = { id: 'tag-id', name: 'Urgent', node: { id: 'research', kind: 'org' as const, name: 'Research', redacted: false } };
  it('offers only readable projects in the defining subtree, including a readable child through a private ancestor', () => {
    const nodes = [row('root', 'org', null), row('research', 'org', 'root'), row('web', 'project', 'research'), row('outside', 'project', 'root'), row('inside', 'project', 'private'), row('thread', 'thread', 'web')];
    expect(projectsForTag(tag, nodes, [{ id: 'private', parent_id: 'research' }]).map((project) => project.id)).toEqual(['inside', 'web']);
    expect(projectsForTag(tag, nodes).map((project) => project.id)).toEqual(['web']);
  });
  it('writes the selected project and the exact tag as the route’s array filter', () => {
    const url = new URL(pathOf(tagProjectDestination(tag, 'web')), 'https://orch.invalid');
    expect(url.pathname).toBe('/project/web');
    expect(url.searchParams.get('view')).toBe('list');
    expect(url.searchParams.get('tag')).toBe(JSON.stringify(['tag-id']));
  });
});
