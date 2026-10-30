/** What the address of a Management section says besides which section it is: the search and filters of a list and the span of the charts, each read with everything it may not say left out.
 *
 * A filtered list can be shared and survives a reload, because the filter is in the address; whatever
 * an address leaves out, or says wrongly, is the default.
 */
import { traceSearch, type TraceSearch } from '@/features/trace/params';

export const SPANS = ['day', 'week', 'month', 'year', 'all'] as const;
export type Span = (typeof SPANS)[number];

export interface AdminSearch {
  /** Settings: the registry key the finder opens. */
  setting?: string;
  /** Users and orgs: part of a name. */
  q?: string;
  /** Users: only those who may sign in, or only those who may not. */
  status?: 'active' | 'suspended';
  /** Users: only system admins. */
  admins?: true;
  /** Overview: how far back the charts reach. */
  span?: Span;
  /** Audit: whose acts (a username or an id). */
  actor?: string;
  /** Audit: which act (`user.suspended`). */
  action?: string;
  /** Audit: the place the act was about (an id). */
  node?: string;
  /** Audit: a day, `2026-10-01`; the export takes the same two. */
  from?: string;
  to?: string;
  /** Runtimes: one pool and one host. */
  kind?: 'environment' | 'provider';
  host?: string;
  /** Organizations: only team orgs, or only people's own. */
  orgKind?: 'team' | 'personal';
  /** Agent programs: the program and its selected detail tab. */
  program?: string;
  programTab?: 'builds' | 'capabilities' | 'settings';
  /** Opaque continuations keep each visible list bounded. */
  programCursor?: string;
  buildCursor?: string;
}

const text = (value: unknown): string | undefined => (typeof value === 'string' && value.trim() !== '' ? value.trim() : undefined);
const DAY = /^\d{4}-\d{2}-\d{2}$/;

/** The search an address carries, with each member kept only if it is one of the things it may be. */
export function readSearch(raw: Record<string, unknown>): AdminSearch {
  const found: AdminSearch = {};
  const setting = text(raw.setting);
  if (setting !== undefined) found.setting = setting;
  const q = text(raw.q);
  if (q !== undefined) found.q = q;
  if (raw.status === 'active' || raw.status === 'suspended') found.status = raw.status;
  if (raw.admins === true || raw.admins === 'true') found.admins = true;
  if (typeof raw.span === 'string' && (SPANS as readonly string[]).includes(raw.span)) found.span = raw.span as Span;
  const actor = text(raw.actor);
  if (actor !== undefined) found.actor = actor;
  const action = text(raw.action);
  if (action !== undefined) found.action = action;
  const node = text(raw.node);
  if (node !== undefined) found.node = node;
  const from = text(raw.from);
  if (from !== undefined && DAY.test(from)) found.from = from;
  const to = text(raw.to);
  if (to !== undefined && DAY.test(to)) found.to = to;
  if (raw.kind === 'environment' || raw.kind === 'provider') found.kind = raw.kind;
  const host = text(raw.host);
  if (host !== undefined) found.host = host;
  if (raw.orgKind === 'team' || raw.orgKind === 'personal') found.orgKind = raw.orgKind;
  const programCursor = text(raw.programCursor);
  if (programCursor !== undefined) found.programCursor = programCursor;
  const program = text(raw.program);
  if (program !== undefined) {
    found.program = program;
    const buildCursor = text(raw.buildCursor);
    if (buildCursor !== undefined) found.buildCursor = buildCursor;
    if (raw.programTab === 'builds' || raw.programTab === 'capabilities' || raw.programTab === 'settings') found.programTab = raw.programTab;
  }
  return found;
}

/** The search after a change: a member set to `undefined` or to an empty text is left out of the address. */
export function withSearch(search: AdminSearch, patch: { [Member in keyof AdminSearch]?: AdminSearch[Member] | undefined }): AdminSearch {
  const merged: Record<string, unknown> = { ...search, ...patch };
  return readSearch(Object.fromEntries(Object.entries(merged).filter(([, value]) => value !== undefined && value !== '')));
}

/** The shared route retains both schemas until its addressed section consumes them.
 * Audit reads traceSearch; every other Management page still reads the strict AdminSearch.
 */
export type ManagementRouteSearch = Omit<AdminSearch, keyof TraceSearch> & TraceSearch;
export function readManagementRouteSearch(raw: Record<string, unknown>): ManagementRouteSearch {
  return { ...readSearch(raw), ...traceSearch(raw) };
}
