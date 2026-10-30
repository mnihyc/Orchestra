/** Project view parameter: list, board, environments. Loader, permission handling and search schema are pending.
 *
 * The screen is the project's: its topics as a list or a board, and its Activity. What it shows is in
 * the address (`view`, `status`, `tag`, `attention`, `sort`, `q`, `archived`), so that a link
 * reproduces it. A plain address resumes the person's saved view of that project.
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { Project, type ProjectSearch } from '@/features/project/Project';

const VIEWS = ['list', 'board', 'activity'] as const;
const STATUSES = ['open', 'done', 'all'] as const;
const ATTENTIONS = ['needs_attention', 'running', 'waiting_approval', 'blocked'] as const;
const SORTS = ['activity', 'created', 'name'] as const;

function oneOf<Value extends string>(allowed: readonly Value[], value: unknown): Value | undefined {
  return allowed.find((candidate) => candidate === value);
}

/** What the address says, with everything that is not one of the things it may say left out. */
export function readSearch(search: Record<string, unknown>): ProjectSearch {
  const view = oneOf(VIEWS, search.view);
  const status = oneOf(STATUSES, search.status);
  const attention = oneOf(ATTENTIONS, search.attention);
  const sort = oneOf(SORTS, search.sort);
  const tag = Array.isArray(search.tag) ? search.tag.filter((id): id is string => typeof id === 'string')
    : typeof search.tag === 'string' ? [search.tag] : typeof search.tag_id === 'string' ? [search.tag_id] : [];
  return {
    ...(view === undefined ? {} : { view }),
    ...(status === undefined ? {} : { status }),
    ...(tag.length === 0 ? {} : { tag }),
    ...(attention === undefined ? {} : { attention }),
    ...(sort === undefined ? {} : { sort }),
    ...(typeof search.q === 'string' ? { q: search.q } : {}),
    ...(typeof search.archived === 'boolean' ? { archived: search.archived } : {}),
    ...(typeof search.with_folders === 'boolean' ? { with_folders: search.with_folders } : {}),
    ...(search.participating === true || search.participating === 'true' ? { participating: true } : {}),
  };
}

export const Route = createFileRoute('/project/$projectId/')({
  staticData: { screen: 'project' },
  validateSearch: readSearch,
  component: ProjectRoute,
});

function ProjectRoute() {
  const { projectId } = usePageParams(Route);
  const search = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return (
    <Project
      projectId={projectId}
      search={search}
      onSearch={(patch) => { void navigate({ search: (before) => readSearch({ ...before, ...patch }), replace: true }); }}
    />
  );
}
