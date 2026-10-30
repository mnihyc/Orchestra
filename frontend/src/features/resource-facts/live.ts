/** Current resource measurements share the existing environment feed and perform no history reads. */
import { useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';

import { processKeys, SAMPLE_MINUTES } from '@/api/processes';
import type { ResourceSample } from '@/generated/api-types';
import { socket } from '@/realtime/live';

const readers = new WeakMap<QueryClient, Map<string, { users: number; stop: () => void }>>();
export const currentSampleKey = (environmentId: string) => ['resource-facts-sample', environmentId] as const;

/** Multiple visible status items or an opened card share one subscription to the same environment. */
export function useResourceSample(environmentId: string | null, allowed = true): ResourceSample | null {
  const client = useQueryClient();
  const id = environmentId ?? '';
  const sample = useQuery<ResourceSample | null>({
    queryKey: currentSampleKey(id),
    queryFn: () => Promise.resolve(client.getQueryData<ResourceSample | null>(currentSampleKey(id)) ?? null),
    enabled: false,
    initialData: () => client.getQueryData<ResourceSample[]>(processKeys.samples({ environmentId: id, threadId: null }, SAMPLE_MINUTES))?.at(-1) ?? null,
  });
  useEffect(() => {
    if (id === '' || !allowed) return;
    let active = readers.get(client);
    if (active === undefined) { active = new Map(); readers.set(client, active); }
    let reader = active.get(id);
    if (reader === undefined) {
      const subscription = socket.subscribe(`env:${id}:stats`, {
        since: () => null,
        onFrame: (frame) => {
          if (frame.op === 'closed' || frame.op === 'error') { client.setQueryData(currentSampleKey(id), null); return; }
          if (frame.op !== 'event' || typeof frame.data.at !== 'string' || typeof frame.data.cpu_percent !== 'number' || typeof frame.data.memory_bytes !== 'number') return;
          client.setQueryData<ResourceSample>(currentSampleKey(id), frame.data as unknown as ResourceSample);
        },
      });
      reader = { users: 0, stop: () => subscription.unsubscribe() };
      active.set(id, reader);
    }
    reader.users += 1;
    return () => {
      reader.users -= 1;
      if (reader.users === 0) { reader.stop(); active.delete(id); }
    };
  }, [client, id, allowed]);
  return allowed ? sample.data ?? null : null;
}
