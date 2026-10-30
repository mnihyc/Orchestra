/** API client for Processes and resources.
 *
 * What runs in an environment, each process with the thread and the tool call that started it where
 * that is known, a signal sent to one, and the samples of its load that the API keeps for the last
 * hours. The list and the samples are asked for through a thread, which is the door the person
 * holds; the signal needs the process's start time, so that a reused process number is never signalled.
 */
import { queryOptions } from '@tanstack/react-query';

import { done, ok, api } from '@/api/client';
import { liveValue } from '@/api/liveValue';
import type { ProcessInfo, ProcessSignal, ResourceSample } from '@/generated/api-types';
import type { Door } from '@/api/workspace';

/** How far back the charts reach by default: the last hour, which is what the API answers with unless asked for more. */
export const SAMPLE_MINUTES = 60;

const through = (door: Door): { thread?: string } => (door.threadId === null ? {} : { thread: door.threadId });

export const processKeys = {
  list: (door: Door) => ['processes', door.environmentId] as const,
  samples: (door: Door, minutes: number) => ['resources', door.environmentId, minutes] as const,
};

/** The processes of the environment with the moment the list was taken. */
export const processesQuery = (door: Door) =>
  queryOptions({
    queryKey: processKeys.list(door),
    queryFn: ({ signal }): Promise<{ as_of: string; processes: ProcessInfo[] }> =>
      liveValue(processKeys.list(door), signal),
  });

/** The samples of the environment's load, oldest first. */
export const samplesQuery = (door: Door, minutes: number = SAMPLE_MINUTES) =>
  queryOptions({
    queryKey: processKeys.samples(door, minutes),
    queryFn: ({ signal }): Promise<ResourceSample[]> =>
      // The feed carries new samples; history is a different read and is taken only once.
      ok(api.GET('/environments/{id}/resources', { params: { path: { id: door.environmentId }, query: { minutes, ...through(door) } }, signal })),
  });

/** Sends a signal to a process; `startedAt` is the start the list gave, so that a reused number is not hit. */
export function signalProcess(door: Door, pid: number, signal: ProcessSignal, startedAt: string): Promise<void> {
  return done(
    api.POST('/environments/{id}/processes/{pid}/signal', {
      params: { path: { id: door.environmentId, pid: String(pid) }, query: through(door) },
      body: { signal, started_at: startedAt },
    }),
  );
}
