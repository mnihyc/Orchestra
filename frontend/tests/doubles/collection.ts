/** Native list envelopes for the fake world's already-authorized rows. */
import type { NotificationChannel } from '../../src/generated/api-types.ts';
export function pageOf<T>(rows: readonly T[], query: URLSearchParams, text: (row: T) => string) {
  const words = (query.get('q') ?? '').toLowerCase().split(/\s+/u).filter(Boolean);
  const found = rows.filter((row) => words.every((word) => text(row).toLowerCase().includes(word)));
  const start = Number(query.get('cursor') ?? 0), limit = Number(query.get('limit') ?? 50);
  return { items: found.slice(start, start + limit), next_cursor: start + limit < found.length ? String(start + limit) : null };
}
export function channelPage(rows: readonly NotificationChannel[], query: URLSearchParams) {
  return { ...pageOf(rows, query, (row) => row.name + ' ' + row.kind), kinds: [...new Set(rows.map((row) => row.kind))] };
}
