/** Header counts use the current native feed; opening a review owns its separate HTTP read. */
import { useContext, useEffect, useState } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import type { ThreadBackground } from '@/generated/api-types';
import { socket } from '@/realtime/live';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';

export function useRunning(thread: string, enabled = true): { roster: ThreadBackground | null; closed: boolean; error: unknown } {
  const opening = useContext(ViewOpeningContext);
  const reader = useStore(client.session, (state) => state.readerVersion);
  const [snapshot, setSnapshot] = useState<{ thread: string; opening: typeof opening; reader: typeof reader; enabled: boolean; roster: ThreadBackground | null; closed: boolean; error: unknown }>({ thread, opening, reader, enabled, roster: null, closed: false, error: null });
  if (snapshot.thread !== thread || snapshot.opening !== opening || snapshot.reader !== reader || snapshot.enabled !== enabled) {
    setSnapshot({ thread, opening, reader, enabled, roster: null, closed: false, error: null });
  }
  useEffect(() => {
    if (opening === null || !enabled) return;
    let active = true;
    const clear = (closed: boolean, error: unknown = null) => {
      if (active && client.session.getState().readerVersion === reader) setSnapshot({ thread, opening, reader, enabled, roster: null, closed, error });
    };
    const subscription = socket.subscribe(`thread:${thread}:background`, { since: () => null, onFrame: (frame) => {
      if (!active || client.session.getState().readerVersion !== reader) return;
      if (frame.op === 'event' && socket.status.getState().phase === 'ready') setSnapshot({ thread, opening, reader, enabled, roster: frame.data as ThreadBackground, closed: false, error: null });
      else if (frame.op === 'closed') clear(true);
      else if (frame.op === 'subscribed') clear(false);
      else if (frame.op === 'error') clear(frame.code === 'forbidden' || frame.code === 'not_found', frame);
    } });
    const unwatch = socket.status.subscribe((status) => {
      if (status.phase !== 'ready') clear(false);
    });
    return () => { active = false; subscription.unsubscribe(); unwatch(); };
  }, [thread, opening, reader, enabled]);
  // A different target or reopened view cannot briefly publish its former observation.
  return enabled && opening !== null && snapshot.thread === thread && snapshot.opening === opening && snapshot.reader === reader && snapshot.enabled === enabled ? snapshot : { roster: null, closed: false, error: null };
}
