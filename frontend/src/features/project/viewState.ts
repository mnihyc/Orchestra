/** Normalizes personal project views and keeps explicit addresses independent of a remembered view. */
import type { ProjectSearch } from '@/features/project/Project';
import type { ProjectView as PersonalProjectView } from '@/generated/api-types';

/** The shipped view is the complete reset target, including scope and sorting. */
export const SHIPPED_PROJECT_VIEW: PersonalProjectView = { view: 'list', status: 'open', tag_ids: [], attention: null, sort: 'activity', q: '', show_archived: false, with_folders: false };

const record = (value: unknown): Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};

/** Unknown values never become new modes; tag ids are unique and in stable order. */
export function normalizeProjectView(value: unknown): PersonalProjectView {
  const held = record(value);
  return {
    view: held.view === 'board' ? 'board' : 'list',
    status: held.status === 'done' || held.status === 'all' ? held.status : 'open',
    tag_ids: Array.isArray(held.tag_ids) ? [...new Set(held.tag_ids.filter((id): id is string => typeof id === 'string' && id !== ''))].sort() : [],
    attention: held.attention === 'needs_attention' || held.attention === 'running' || held.attention === 'waiting_approval' || held.attention === 'blocked' ? held.attention : null,
    sort: held.sort === 'created' || held.sort === 'name' ? held.sort : 'activity',
    q: typeof held.q === 'string' ? held.q : '',
    show_archived: held.show_archived === true,
    with_folders: held.with_folders === true,
  };
}

/** Reads only this project's entry, omitting a saved tag which the current list does not expose. */
export function rememberedProjectView(state: unknown, projectId: string, readableTags?: readonly string[]): PersonalProjectView {
  const held = normalizeProjectView(record(record(state).project_views)[projectId]);
  return readableTags === undefined ? held : { ...held, tag_ids: held.tag_ids.filter((id) => readableTags.includes(id)) };
}

/** Supported explicit parameters, including explicit defaults, take precedence as a whole. */
export function explicitProjectAddress(search: ProjectSearch): boolean {
  return ['view', 'status', 'tag', 'attention', 'sort', 'q', 'archived', 'with_folders', 'participating'].some((key) => Object.hasOwn(search, key));
}

export function viewFromSearch(search: ProjectSearch): PersonalProjectView {
  return normalizeProjectView({ view: search.view, status: search.status, tag_ids: search.tag, attention: search.attention, sort: search.sort, q: search.q, show_archived: search.archived, with_folders: search.with_folders });
}

/** Always includes the view, so an explicit reset cannot reload the previously saved filters. */
export function searchFromView(view: PersonalProjectView): ProjectSearch {
  return {
    view: view.view,
    ...(view.status === 'open' ? {} : { status: view.status }),
    ...(view.tag_ids.length === 0 ? {} : { tag: view.tag_ids }),
    ...(view.attention === null ? {} : { attention: view.attention }),
    ...(view.sort === 'activity' ? {} : { sort: view.sort }),
    ...(view.q === '' ? {} : { q: view.q }),
    ...(view.show_archived ? { archived: true } : {}),
    ...(view.with_folders ? { with_folders: true } : {}),
  };
}

/** A saved view uses the ordinary project address, with no extra shared-view route. */
export function projectViewAddress(projectId: string, view: PersonalProjectView): string {
  const params = new URLSearchParams();
  const search = searchFromView(view);
  for (const [key, value] of Object.entries(search)) params.set(key, Array.isArray(value) ? JSON.stringify(value) : String(value));
  return `/project/${encodeURIComponent(projectId)}?${params.toString()}`;
}
