/** The trail's address, exact filters, date bounds and stable presentation groups. */
import type { Access, NodeRef, Org, Project, Thread, Topic, AuditKind, AuditEntry as TraceEntry } from '@/generated/api-types';
import { DAY_MS } from '@/lib/time';

export const AUDIT_KINDS: readonly AuditKind[] = ['made', 'changed', 'deleted', 'access', 'work', 'sign_ins'];
/** The API's maximum audit-export span includes a leap year. */
export const EXPORT_DAYS = 366;
export type GroupBy = 'day' | 'kind';
/** Reset markers address the exact entry inside the filtered trail. */
export function auditEntryAnchor(id:string):string { return `audit-entry-${id}`; }
export interface TraceSearch { actor?: string; include_agents?: boolean; action?: string; kind?: string; from?: string; to?: string; group?: GroupBy }
export type TraceScope = { type: 'system'; name: string } | { type: 'node'; id: string; name: string; kind: NodeRef['kind']; orgId: string; address?: string; access: Access | undefined };

export function scopeOf(node: Org | Project | Topic | Thread): TraceScope {
  return { type: 'node', id: node.id, name: node.name, kind: node.kind, orgId: node.org_id, access: node.access, ...(node.kind === 'org' ? { address: node.address } : {}) };
}

/** Dates and wire identifiers remain in the URL; unknown kinds never become a broad filter. */
export function traceSearch(search: Record<string, unknown>): TraceSearch {
  const result: TraceSearch = {};
  for (const key of ['actor', 'action', 'from', 'to'] as const) if (typeof search[key] === 'string' && search[key] !== '') result[key] = search[key];
  if (typeof search.kind === 'string' && search.kind !== '') result.kind = search.kind;
  if (search.include_agents === true || search.include_agents === 'true') result.include_agents = true;
  else if (search.include_agents === false || search.include_agents === 'false') result.include_agents = false;
  if (search.group === 'day' || search.group === 'kind') result.group = search.group;
  return result;
}

export function traceHref(scope: TraceScope, actor?: string, filters: TraceSearch = {}): string {
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries({ ...filters, ...(actor === undefined ? {} : { actor, include_agents: filters.include_agents ?? true }) })) if (typeof value === 'string' || typeof value === 'boolean') query.set(key, String(value));
  let route = '/admin/audit';
  if (scope.type === 'node') {
    if (scope.kind === 'org') route = `/org/${encodeURIComponent(scope.address ?? scope.id)}/settings/audit`;
    else if (scope.kind === 'project') { route = `/project/${scope.id}/settings`; query.set('section', 'audit'); }
    else if (scope.kind === 'topic') { route = `/topic/${scope.id}`; query.set('tab', 'settings'); query.set('section', 'audit'); }
    else { route = `/trace/${scope.id}`; }
  }
  return `${route}${query.size === 0 ? '' : `?${query.toString()}`}`;
}

/** A date field covers that entire UTC day; an exact timestamp remains exact. */
export function timestamp(value: string | undefined, end = false): string | undefined {
  if (value === undefined || value === '') return undefined;
  return /^\d{4}-\d{2}-\d{2}$/.test(value) ? `${value}T${end ? '23:59:59.999' : '00:00:00.000'}Z` : value;
}
export function exportSpanValid(from: string | undefined, to: string | undefined): boolean {
  if (!from || !to) return false;
  const start = Date.parse(timestamp(from) ?? ''); const end = Date.parse(timestamp(to, true) ?? '');
  return Number.isFinite(start) && Number.isFinite(end) && end >= start && end - start <= EXPORT_DAYS * DAY_MS;
}

export interface TraceGroup { key: string; rows: TraceEntry[] }
/** Groups the rows already received; a day split across pages stays one group and has no invented total. */
export function groupEntries(entries: readonly TraceEntry[], group: GroupBy, timeZone: string): TraceGroup[] {
  const groups = new Map<string, TraceGroup>();
  const day = new Intl.DateTimeFormat('en-CA', { year: 'numeric', month: '2-digit', day: '2-digit', timeZone });
  const seen = new Set<string>();
  for (const entry of entries) {
    if (seen.has(entry.id)) continue; seen.add(entry.id);
    const key = group === 'kind' ? (entry.kind ?? 'unknown') : day.format(new Date(entry.at));
    const known = groups.get(key);
    if (known === undefined) groups.set(key, { key, rows: [entry] }); else known.rows.push(entry);
  }
  return [...groups.values()];
}
