/** A tag's eligible project choice never escapes its defining place or includes hidden project rows. */
import { describe, expect, it } from 'vitest';
import { pathOf } from '@/features/finder/destinations';
import { projectsForTag, tagProjectDestination } from '@/features/finder/TagProjects';
import type { TreeNode } from '@/generated/api-types';

const row = (id: string, kind: TreeNode['kind'], parent_id: string | null): TreeNode => ({ id, kind, name: id, parent_id, color: null, deleted: false }) as TreeNode;

describe('a tag in the finder', () => {
  const tag = { id: 'tag-id', name: 'Urgent', node: { id: 'research', kind: 'org' as const, name: 'Research', redacted: false } };
  it('keeps admitted matches through private ancestry and never promotes context rows into choices', () => {
    const inside=row('inside','project','private'),web=row('web','project','research');
    const page={nodes:[inside,web,row('thread','thread','web')],ancestors:[row('outside','project','root')]};
    expect(projectsForTag(page).map((project)=>project.id)).toEqual(['inside','web']);
  });
  it('writes the selected project and the exact tag as the route’s array filter', () => {
    const url = new URL(pathOf(tagProjectDestination(tag, 'web')), 'https://orch.invalid');
    expect(url.pathname).toBe('/project/web');
    expect(url.searchParams.get('view')).toBe('list');
    expect(url.searchParams.get('tag')).toBe(JSON.stringify(['tag-id']));
  });
});
