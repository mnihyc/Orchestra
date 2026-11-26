/** Personal model history is read from the server and records one idempotent selection at a time. */
import { queryOptions } from '@tanstack/react-query';
import { api, done, ok } from '@/api/client';
import type { ModelRecent, ModelRecents } from '@/generated/api-types';

export const modelHistoryKey = (person: string | null) => ['model-recents', person] as const;

export const modelHistoryQuery = (person: string | null) => queryOptions({
  queryKey: modelHistoryKey(person),
  queryFn: ({ signal }): Promise<ModelRecents> => ok(api.GET('/me/model-recents', { signal })),
});

export function recordModelChoice(model: ModelRecent, key: string): Promise<void> {
  return done(api.POST('/me/model-recents', { body: model, headers: { 'Idempotency-Key': key } }));
}
