/** Validated device and address choices for the wide hierarchy, excluding typed search text. */
import type { EverythingDirection, EverythingKind, EverythingSort } from '@/generated/api-types';
import type { NavigationArrangement, RecentOrder } from '@/features/navigation/recent';
import { readViewChoice } from '@/lib/viewChoices';

export const EVERYTHING_KINDS: readonly EverythingKind[] = ['org', 'project', 'topic', 'thread', 'document', 'chat', 'environment', 'provider', 'agent', 'mcp_server', 'machine', 'host', 'mount', 'template', 'skill'];
const SORTS: readonly EverythingSort[] = ['name', 'kind', 'created', 'created_by', 'owner', 'last_activity', 'opened', 'where', 'activity_by', 'state', 'size', 'my_access', 'tags'];
export interface EverythingView {
  arrangement: NavigationArrangement;
  order: RecentOrder;
  kinds: readonly EverythingKind[];
  owner: { id: string; name: string } | null;
  createdFrom: string;
  createdTo: string;
  activeFrom: string;
  sort: EverythingSort;
  direction: EverythingDirection;
  participating: boolean;
  adminAccess: boolean;
  navigation: boolean;
  showArchived: boolean;
  hideDone: boolean;
}
export const DEFAULT_VIEW: EverythingView = { arrangement: 'tree', order: 'opened', kinds: [], owner: null, createdFrom: '', createdTo: '', activeFrom: '', sort: 'name', direction: 'asc', participating: false, adminAccess: false, navigation: false, showArchived: true, hideDone: false };

export function validView(value: unknown): EverythingView {
  const own = typeof value === 'object' && value !== null ? value as Record<string, unknown> : {};
  const arrangement = own.arrangement === 'recent' ? 'recent' : 'tree';
  const order = own.order === 'active' ? 'active' : 'opened';
  const text = (key: string) => typeof own[key] === 'string' ? own[key] : '';
  const owner = typeof own.owner === 'object' && own.owner !== null ? own.owner as Record<string, unknown> : {};
  const kinds = Array.isArray(own.kinds) ? own.kinds : [];
  return { ...DEFAULT_VIEW, arrangement, order,
    kinds: EVERYTHING_KINDS.filter((kind) => kinds.includes(kind)),
    owner: typeof owner.id === 'string' ? { id: owner.id, name: typeof owner.name === 'string' ? owner.name : '' } : null,
    createdFrom: text('createdFrom'), createdTo: text('createdTo'), activeFrom: text('activeFrom'),
    sort: SORTS.includes(own.sort as EverythingSort) ? own.sort as EverythingSort : arrangement === 'recent' ? order === 'opened' ? 'opened' : 'last_activity' : 'name',
    direction: own.direction === 'desc' ? 'desc' : own.direction === 'asc' ? 'asc' : arrangement === 'recent' ? 'desc' : 'asc',
    participating: own.participating === true, adminAccess: own.adminAccess === true, navigation: own.navigation === true,
    showArchived: own.showArchived !== false, hideDone: own.hideDone === true };
}

export interface EverythingSearch {
  scope?: 'mine' | 'system'; q?: string; participating?: boolean;
  parent?: string; org?: string; arrangement?: NavigationArrangement; order?: RecentOrder;
  kinds?: string; owner?: string | undefined; sort?: EverythingSort; direction?: EverythingDirection;
  createdFrom?: string; createdTo?: string; activeFrom?: string;
  navigation?: boolean; showArchived?: boolean; hideDone?: boolean; adminAccess?: boolean;
}

/** Invalid address choices disappear; explicit valid values are applied to shipped defaults. */
export function everythingSearch(search: Record<string, unknown>): EverythingSearch {
  const answer: EverythingSearch = {};
  if (search.scope === 'mine' || search.scope === 'system') answer.scope = search.scope;
  for (const key of ['q', 'parent', 'org', 'kinds', 'owner', 'createdFrom', 'createdTo', 'activeFrom'] as const) if (typeof search[key] === 'string' && search[key] !== '') answer[key] = search[key];
  if (search.arrangement === 'tree' || search.arrangement === 'recent') answer.arrangement = search.arrangement;
  if (search.order === 'opened' || search.order === 'active') answer.order = search.order;
  if (SORTS.includes(search.sort as EverythingSort)) answer.sort = search.sort as EverythingSort;
  if (search.direction === 'asc' || search.direction === 'desc') answer.direction = search.direction;
  for (const key of ['participating', 'navigation', 'showArchived', 'hideDone', 'adminAccess'] as const) {
    if (search[key] === true || search[key] === 'true') answer[key] = true;
    else if (search[key] === false || search[key] === 'false') answer[key] = false;
  }
  return answer;
}

export function viewFromAddress(search: EverythingSearch): Partial<EverythingView> | undefined {
  if (!Object.keys(search).some((key) => !['q', 'scope', 'parent', 'org'].includes(key))) return undefined;
  return { ...search, kinds: search.kinds?.split(',').filter((kind): kind is EverythingKind => EVERYTHING_KINDS.includes(kind as EverythingKind)) ?? [], owner: search.owner === undefined ? null : { id: search.owner, name: '' } };
}

export function addressFromView(view: EverythingView): EverythingSearch {
  return { ...view, kinds: view.kinds.join(','), owner: view.owner?.id };
}

export function initialView(person: string, thing: string, explicit?: Partial<EverythingView>): EverythingView {
  if (explicit !== undefined) return validView(explicit);
  try { return validView(JSON.parse(readViewChoice(person, 'everything', 'view', '', thing))); }
  catch { return { ...DEFAULT_VIEW }; }
}
