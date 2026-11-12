/** One measured server/device offset, shared by HTTP admission and realtime countdowns. */
import { useSyncExternalStore } from 'react';

export interface ServerClock {
  offset: () => number | null;
  subscribe: (listener: () => void) => () => void;
  /** The server timestamp was produced during this actual request's round trip. */
  observe: (serverTime: string, sentAt: number, receivedAt: number) => void;
  clear: () => void;
}

export function createServerClock(): ServerClock {
  let ahead: number | null = null;
  const listeners = new Set<() => void>();
  const set = (value: number | null) => {
    if (value === ahead) return;
    ahead = value;
    for (const listener of listeners) listener();
  };
  return {
    offset: () => ahead,
    subscribe: (listener) => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    observe: (serverTime, sentAt, receivedAt) => {
      const time = Date.parse(serverTime);
      if (![time, sentAt, receivedAt].every(Number.isFinite) || receivedAt < sentAt) return;
      // The midpoint accounts for transport time without pretending to know its two one-way delays.
      set(time - (sentAt + (receivedAt - sentAt) / 2));
    },
    clear: () => set(null),
  };
}

export const serverClock = createServerClock();

/** Reuses the caller's existing tick; an answered clock sample also updates it immediately. */
export function useServerNow(deviceNow: number): number | null {
  const ahead = useSyncExternalStore(serverClock.subscribe, serverClock.offset);
  return ahead === null ? null : deviceNow + ahead;
}
