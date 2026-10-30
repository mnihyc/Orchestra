/** A native count comes from current HTTP or ephemeral facts and is discarded on every reconnect. */
import { useEffect, useState } from 'react';
import { fetchBackground } from '@/api/background';
import type { ThreadBackground } from '@/generated/api-types';
import { socket } from '@/realtime/live';

export function useRunning(thread: string): { roster: ThreadBackground | null; closed: boolean; error: unknown } {
  const [snapshot, setSnapshot] = useState<{ thread: string; roster: ThreadBackground | null; closed: boolean; error: unknown }>({ thread, roster: null, closed: false, error: null });
  useEffect(() => {
    let active = true;
    let generation = 0;
    const controller = new AbortController();
    const read = () => {
      const reading = generation;
      void fetchBackground(thread, undefined, controller.signal).then((roster) => {
        if (active && reading === generation && socket.status.getState().phase === 'ready') setSnapshot({ thread, roster, closed: false, error: null });
      }).catch((error: unknown) => {
        if (active && reading === generation && !controller.signal.aborted) setSnapshot({ thread, roster: null, closed: false, error });
      });
    };
    read();
    const subscription = socket.subscribe(`thread:${thread}:background`, { since: () => null, onFrame: (frame) => {
      if (!active) return;
      if (frame.op === 'event' && socket.status.getState().phase === 'ready') setSnapshot({ thread, roster: frame.data as ThreadBackground, closed: false, error: null });
      else if (frame.op === 'closed') { generation += 1; setSnapshot({ thread, roster: null, closed: true, error: null }); }
      else if (frame.op === 'subscribed') { generation += 1; setSnapshot({ thread, roster: null, closed: false, error: null }); }
      else if (frame.op === 'error') { generation += 1; setSnapshot({ thread, roster: null, closed: false, error: frame }); }
    } });
    const unwatch = socket.status.subscribe((status, previous) => {
      if (status.phase !== 'ready') { generation += 1; setSnapshot({ thread, roster: null, closed: false, error: null }); }
      else if (previous.phase !== 'ready') read();
    });
    return () => { active = false; controller.abort(); subscription.unsubscribe(); unwatch(); };
  }, [thread]);
  // Changing screens hides the old thread synchronously; external network events update the new one.
  return snapshot.thread === thread ? snapshot : { roster: null, closed: false, error: null };
}
