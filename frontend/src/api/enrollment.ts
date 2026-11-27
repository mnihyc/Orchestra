/** An enrollment's native, issuer-only progress; realtime follows its resource after it appears. */
import { queryOptions, replaceEqualDeep, type QueryClient } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import { record } from '@/api/cache';
import type { Change } from '@/api/acts';
import type { EnrollmentProgress } from '@/generated/api-types';
import { readAgain } from '@/api/readAgain';
import { resourceFollowing } from '@/realtime/resourceFollowing';

export const ENROLLMENT_PROGRESS = 'enrollment-progress';
export const enrollmentProgressKey = (receipt: string) => [ENROLLMENT_PROGRESS, receipt] as const;

export interface EnrollmentChange { enrollment: { receipt: string; status: Record<string, unknown> } }

/** Discovery can precede or follow an object's first frame. Once acknowledged, that stream owns
 * its known row; an older user notice must not roll an online machine back to its saved state. A notice that the
 * machine is online, where the progress held says it is not, is taken at once: reading the progress again first would
 * wait a round trip, and a read from before the session could answer that it is still offline.
 */
export function enrollmentChanges(cache: QueryClient, change: EnrollmentChange['enrollment']): Change[] {
  const key = enrollmentProgressKey(change.receipt), held = cache.getQueryData(key);
  const data = { ...change.status };
  const rows: Change[] = [];
  for (const kind of ['machine', 'host'] as const) {
    const row = data[kind], before = record(held) ? held[kind] : undefined;
    if (!record(row) || typeof row.id !== 'string' || !record(row.access)) continue;
    if (record(before) && before.id === row.id && resourceFollowing(kind, row.id)) {
      const previously = kind === 'host' && record(before.machine) ? before.machine : before;
      const incoming = kind === 'host' && record(row.machine) ? row.machine : row;
      // Its arrival online is the progress's own news: the dialog moves on with it. Its row stays the stream's.
      data[kind] = previously.online === false && incoming.online === true ? row : before;
    } else rows.push({ row: { kind, id: row.id, value: row, insert: true } });
  }
  return [{ value: { key, data } }, ...rows];
}

/** Whether the progress names its machine online: a machine itself, or a host's. */
const onlineIn = (progress: EnrollmentProgress | undefined, kind: 'machine' | 'host'): { id: string; online: boolean } | null => {
  if (progress === undefined) return null;
  if (kind === 'machine') return progress.machine === null ? null : { id: progress.machine.id, online: progress.machine.online };
  return progress.host === null ? null : { id: progress.host.id, online: progress.host.machine.online };
};

/** A progress the dialog has seen online stays online: an answer read before the machine's session (a slower read, or one
 * still out when the notice came) cannot take it back, and the dialog does not wait again for its next re-read. */
export function keepOnline(held: unknown, answer: EnrollmentProgress): EnrollmentProgress {
  if (!record(held) || held.receipt !== answer.receipt) return answer;
  const before = held as unknown as EnrollmentProgress;
  let kept = answer;
  for (const kind of ['machine', 'host'] as const) {
    const was = onlineIn(before, kind), now = onlineIn(kept, kind);
    if (was !== null && now !== null && was.id === now.id && was.online && !now.online) kept = { ...kept, [kind]: before[kind] };
  }
  return kept;
}

export const enrollmentProgressQuery = (receipt: string) => queryOptions({
  queryKey: enrollmentProgressKey(receipt), staleTime: Infinity, retry: false,
  structuralSharing: (held: unknown, answer: unknown) => replaceEqualDeep(held, record(answer) ? keepOnline(held, answer as unknown as EnrollmentProgress) : answer),
  // The machine's arrival is told over the live connection, once. Until it is online the dialog reads again by itself.
  refetchInterval: readAgain<EnrollmentProgress>((progress) => !(progress?.machine?.online === true || progress?.host?.machine.online === true)),
  // The cache owns this read across transient mounts. Native events cancel its pending result
  // before applying a newer receipt/resource, so a delayed answer cannot roll progress back.
  queryFn: async (): Promise<EnrollmentProgress> => {
    const progress = await ok(api.GET('/machines/enrollment-status/{receipt}', { params: { path: { receipt } } }));
    if (progress.receipt !== receipt) throw new Error('The enrollment response does not match this request');
    return progress;
  },
});
