/** Loaded-row range selection and revision-captured status batches with explicit refusal and uncertainty. */
import { ApiError } from '@/api/client';
import type { Topic, TopicStatus } from '@/generated/api-types';
import { holds } from '@/lib/permissions';

/** Shift selects the loaded range only, preserving selections outside that range. */
export function selectRange(selected: ReadonlySet<string>, loaded: readonly string[], anchor: string | null, id: string, on: boolean, shift: boolean): Set<string> {
  const next = new Set(selected);
  const first = anchor === null ? -1 : loaded.indexOf(anchor);
  const last = loaded.indexOf(id);
  const ids = shift && first >= 0 && last >= 0 ? loaded.slice(Math.min(first, last), Math.max(first, last) + 1) : [id];
  for (const one of ids) {
    if (on) next.add(one);
    else next.delete(one);
  }
  return next;
}

/** Every refusal names the affected row; a parent's role or permission never enables a topic. */
export function statusBlockers(topics: readonly Topic[], status: TopicStatus): { topic: Topic; reason: 'permission' | 'archived' | 'already' }[] {
  const blocked: { topic: Topic; reason: 'permission' | 'archived' | 'already' }[] = [];
  for (const topic of topics) {
    if (!holds(topic.access, 'topic.interact')) blocked.push({ topic, reason: 'permission' });
    else if (topic.archived) blocked.push({ topic, reason: 'archived' });
    else if (topic.status === status) blocked.push({ topic, reason: 'already' });
  }
  return blocked;
}

export interface StatusResult {
  before: Topic;
  state: 'queued' | 'pending' | 'succeeded' | 'refused' | 'uncertain';
  after?: Topic;
  error?: unknown;
  observed?: Topic;
}

/** A server refusal is confirmed; a lost answer or a server failure leaves the write uncertain. */
export async function statusBatch(topics: readonly Topic[], status: TopicStatus, write: (topic: Topic, status: TopicStatus) => Promise<Topic>, progress: (result: StatusResult) => void): Promise<StatusResult[]> {
  const results: StatusResult[] = [];
  for (const before of topics) {
    progress({ before, state: 'pending' });
    let result: StatusResult;
    try { result = { before, state: 'succeeded', after: await write(before, status) }; }
    catch (error) { result = { before, state: error instanceof ApiError && error.status < 500 ? 'refused' : 'uncertain', error }; }
    results.push(result);
    progress(result);
  }
  return results;
}

/** Only replies which confirmed success supply an inverse; an observed uncertain write is never guessed into one. */
export function confirmedInverses(results: readonly StatusResult[]): { before: Topic; after: Topic }[] {
  return results.flatMap((result) => result.state === 'succeeded' && result.after !== undefined ? [{ before: result.before, after: result.after }] : []);
}
