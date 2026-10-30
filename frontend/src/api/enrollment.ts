/** An enrollment's native, issuer-only progress; realtime follows its resource after it appears. */
import { queryOptions, type QueryClient } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import { record } from '@/api/cache';
import type { Change } from '@/api/acts';
import type { EnrollmentProgress } from '@/generated/api-types';
import { resourceFollowing } from '@/realtime/resourceFollowing';

export const ENROLLMENT_PROGRESS = 'enrollment-progress';
export const enrollmentProgressKey = (receipt: string) => [ENROLLMENT_PROGRESS, receipt] as const;

export interface EnrollmentChange { enrollment: { receipt: string; status: Record<string, unknown> } }

/** Discovery can precede or follow an object's first frame. Once acknowledged, that stream owns
 * its known row; an older user notice must not roll an online machine back to its saved state.
 */
export function enrollmentChanges(cache: QueryClient, change: EnrollmentChange['enrollment']): Change[] {
  const key = enrollmentProgressKey(change.receipt), held = cache.getQueryData(key);
  const data = { ...change.status };
  const rows: Change[] = [];
  for (const kind of ['machine', 'host'] as const) {
    const row = data[kind], before = record(held) ? held[kind] : undefined;
    if (!record(row) || typeof row.id !== 'string' || !record(row.access)) continue;
    if (record(before) && before.id === row.id && resourceFollowing(kind, row.id)) data[kind] = before;
    else rows.push({ row: { kind, id: row.id, value: row, insert: true } });
  }
  return [{ value: { key, data } }, ...rows];
}

export const enrollmentProgressQuery = (receipt: string) => queryOptions({
  queryKey: enrollmentProgressKey(receipt), staleTime: Infinity, retry: false,
  // The cache owns this read across transient mounts. Native events cancel its pending result
  // before applying a newer receipt/resource, so a delayed answer cannot roll progress back.
  queryFn: async (): Promise<EnrollmentProgress> => {
    const progress = await ok(api.GET('/machines/enrollment-status/{receipt}', { params: { path: { receipt } } }));
    if (progress.receipt !== receipt) throw new Error('The enrollment response does not match this request');
    return progress;
  },
});
