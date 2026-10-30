/** A project's personal view is normalized, explicit addresses win as a whole, and hidden saved tags are not used. */
import { describe, expect, it } from 'vitest';

import { explicitProjectAddress, normalizeProjectView, projectViewAddress, rememberedProjectView, searchFromView, SHIPPED_PROJECT_VIEW, viewFromSearch } from '@/features/project/viewState';

describe('a personal project view', () => {
  it('has one complete shipped reset target', () => {
    expect(normalizeProjectView(undefined)).toEqual(SHIPPED_PROJECT_VIEW);
    expect(viewFromSearch({ view: 'list' })).toEqual(SHIPPED_PROJECT_VIEW);
  });

  it('normalizes unsupported values and duplicate tag ids without adding a new mode', () => {
    expect(normalizeProjectView({ view: 'calendar', status: 'unknown', tag_ids: ['b', 'a', 'b', 9], attention: 'manual', sort: 'name', q: 'hello', with_folders: true })).toEqual({ ...SHIPPED_PROJECT_VIEW, tag_ids: ['a', 'b'], sort: 'name', q: 'hello', with_folders: true });
  });

  it('an explicit default address wins and leaves remembered filters out', () => {
    const saved = rememberedProjectView({ project_views: { web: { view: 'board', tag_ids: ['private'], q: 'old' } } }, 'web');
    const address = { view: 'list' } as const;
    expect(explicitProjectAddress(address)).toBe(true);
    expect(viewFromSearch(address)).toEqual(SHIPPED_PROJECT_VIEW);
    expect(saved.view).toBe('board');
    expect(explicitProjectAddress({})).toBe(false);
  });

  it('uses only the chosen project and currently readable tags', () => {
    expect(rememberedProjectView({ project_views: { web: { view: 'board', tag_ids: ['hidden', 'visible'] }, other: { q: 'sibling' } } }, 'web', ['visible'])).toEqual({ ...SHIPPED_PROJECT_VIEW, view: 'board', tag_ids: ['visible'] });
  });

  it('a starred view has an ordinary explicit project address with its complete scope', () => {
    const view = { ...SHIPPED_PROJECT_VIEW, view: 'board' as const, with_folders: true, tag_ids: ['infra'] };
    expect(searchFromView(view)).toEqual({ view: 'board', tag: ['infra'], with_folders: true });
    expect(projectViewAddress('web', view)).toBe('/project/web?view=board&tag=%5B%22infra%22%5D&with_folders=true');
  });
});
