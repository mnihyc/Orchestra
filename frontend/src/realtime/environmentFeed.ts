/** An environment's own live feed, followed while a screen of the environment is open.
 *
 * The feed carries the environment whole, as its reader may see it, whenever it or a job about it
 * changes: its state, the progress of what is being done to it, an update that waits. The screen holds
 * the environment in its query, and each frame puts the environment it carries in that place, so a
 * start, a stop or a move is seen as it happens and not at the next look. Following begins at the
 * number returned by the same authorized HTTP sync. A reconnect resumes that number; only a
 * server-declared unrepairable gap reads that one environment's sync again.
 */
import { type QueryClient, useQueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';

import { environmentQuery } from '@/api/environments';
import { applyRow, feedSequences, record } from '@/api/cache';
import { socket } from '@/realtime/live';
import type { FeedFrame } from '@/realtime/socket';

/** The name of an environment's feed on the socket. */
export const environmentFeed = (environmentId: string): string => `env:${environmentId}`;

/** What one frame of the feed changes in what the page holds. */
export function applyEnvironmentFrame(client: QueryClient, environmentId: string, frame: FeedFrame): void {
  if (frame.op === 'sync') {
    const environment = frame.data.environment;
    if (record(environment)) {
      applyRow(client, { kind: 'environment', id: environmentId, value: environment });
    }
    feedSequences.set(environmentFeed(environmentId), frame.seq);
    for (const job of Array.isArray(frame.data.jobs) ? frame.data.jobs as unknown[] : []) {
      if (record(job) && typeof job.id === 'string' && 'result' in job) applyRow(client, { kind: 'job', id: job.id, value: job });
    }
  }
}

/** Follows the feed of the environment `environmentId` while the calling screen is open. */
export function useEnvironmentFeed(environmentId: string | null): void {
  const client = useQueryClient();
  useEffect(() => {
    if (environmentId === null || environmentId === '') return;
    let active = true;
    let repairing = false;
    let subscription: ReturnType<typeof socket.subscribe> | null = null;
    const follow = () => socket.subscribe(environmentFeed(environmentId), {
      since: () => feedSequences.get(environmentFeed(environmentId)) ?? null,
      onFrame: (frame) => {
        if (frame.op === 'resync' || frame.op === 'subscribed' && frame.mode === 'reload') {
          // Only a server-declared gap needs an exact snapshot. Ordinary reconnects replay the cursor.
          if (repairing) return;
          repairing = true;
          void client.query({ ...environmentQuery(environmentId), staleTime: 0 }).then(() => {
            if (!active) return;
            subscription?.unsubscribe(); subscription = follow();
          }, () => undefined).finally(() => { repairing = false; });
        } else applyEnvironmentFrame(client, environmentId, frame);
      },
    });
    subscription = follow();
    return () => { active = false; subscription?.unsubscribe(); };
  }, [client, environmentId]);
}
