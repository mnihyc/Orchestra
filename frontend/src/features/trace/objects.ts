import type { AuditEntry as TraceEntry } from '@/generated/api-types';
/** Separately authorized destinations for live and retained objects. */

export function inspectionHref(kind: string, id: string): string { return `/inspect/${encodeURIComponent(kind)}/${encodeURIComponent(id)}`; }
export function objectHref(kind: string, id: string): string | null {
  if (kind === 'document') return `/doc/${id}`;
  if (['org', 'project', 'topic', 'thread'].includes(kind)) return `/${kind}/${id}`;
  const resources: Record<string, string> = { environment: 'environments', host: 'hosts', provider: 'providers', agent: 'agents', template: 'templates', machine: 'machines', mount: 'mounts', mcp: 'mcp', skill: 'skills' };
  return resources[kind] === undefined ? null : `/resources/${resources[kind]}/${id}`;
}
/** A permanent name is not a ticket to content; old rows lacking current guards stay closed. */
/** Only an explicit current wire grant opens an act, including legacy responses. */
export function currentGuard(value: unknown): boolean { return value === true; }
export function objectActs(object: TraceEntry['object']): { open: string | null; inspect: string | null; restore: boolean } {
  if (object.id === null || object.state === 'purged') return { open: null, inspect: null, restore: false };
  return { open: object.state === 'live' && currentGuard(object.can_open) ? objectHref(object.kind, object.id) : null, inspect: object.state === 'deleted' && currentGuard(object.can_inspect) ? inspectionHref(object.kind, object.id) : null, restore: object.state === 'deleted' && currentGuard(object.can_restore) };
}
