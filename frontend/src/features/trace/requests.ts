/** Request presentation keeps every matching row and never guesses a legacy request from its words or time. */
import { groupEntries, type GroupBy } from './params';
import type { AuditEntry as TraceEntry } from '@/generated/api-types';

export interface RequestLine { key: string; chosen: TraceEntry; rows: TraceEntry[]; mayContinue: boolean }
export interface RequestGroup { key: string; lines: RequestLine[] }
/** Accumulated pages extend the same request before day/kind headings are applied. */
export function requestGroups(entries: readonly TraceEntry[], group: GroupBy, timeZone: string, hasMore: boolean): RequestGroup[] {
  const requests = new Map<string, RequestLine>(); const seen = new Set<string>();
  for (const entry of entries) {
    if (seen.has(entry.id)) continue; seen.add(entry.id);
    const key = entry.request_id ? `request:${entry.request_id}` : `row:${entry.id}`;
    const known = requests.get(key);
    if (known === undefined) requests.set(key, { key, chosen: entry, rows: [entry], mayContinue: hasMore && Boolean(entry.request_id) });
    else known.rows.push(entry);
  }
  const lines = [...requests.values()]; const byId = new Map(lines.map((line) => [line.chosen.id, line]));
  return groupEntries(lines.map((line) => line.chosen), group, timeZone).map((part) => ({ key: part.key, lines: part.rows.flatMap((entry) => { const line = byId.get(entry.id); return line === undefined ? [] : [line]; }) }));
}

/** Names come from captured descriptive facts, without a lookup that could rename the holder. */
export function recordedHolder(details: unknown): string | null {
  if (typeof details !== 'object' || details === null || !('holder' in details)) return null;
  const holder = details.holder;
  if (typeof holder !== 'object' || holder === null) return null;
  if ('display_name' in holder && typeof holder.display_name === 'string') return holder.display_name;
  if ('name' in holder && typeof holder.name === 'string') return holder.name;
  return null;
}
