/** Personal reading choices override installation defaults without adding a read to a hidden view. */
import { QueryClientContext } from '@tanstack/react-query';
import { useCallback, useContext, useSyncExternalStore } from 'react';
import type { Meta, PersonalStateKey } from '@/generated/api-types';

/** Reading choices have their own declared native personal-state key. */
export const READING_STATE_KEY: PersonalStateKey = 'interface';

// Used while the shell's metadata is still arriving; the registry supplies the same defaults.
export const DEFAULT_INTERFACE_READING = { kept_closed_tabs: 10, progress_delay_ms: 200 };
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const count = (value: unknown, fallback: number) => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : fallback;

export function interfaceReading(state: Record<string, unknown> | undefined, meta?: Meta) {
  const defaults = meta?.interface_defaults ?? DEFAULT_INTERFACE_READING;
  const own = record(state?.interface) ? state.interface : {};
  return { kept_closed_tabs: count(own.kept_closed_tabs, defaults.kept_closed_tabs), progress_delay_ms: count(own.progress_delay_ms, defaults.progress_delay_ms) };
}

export function useInterfaceReading() {
  const cache = useContext(QueryClientContext);
  const subscribe = useCallback((changed: () => void) => cache?.getQueryCache().subscribe(changed) ?? (() => undefined), [cache]);
  const reading = () => interfaceReading(cache?.getQueryData<Record<string, unknown>>(['me-state']), cache?.getQueryData<Meta>(['meta']));
  const recent = useSyncExternalStore(subscribe, () => reading().kept_closed_tabs, () => DEFAULT_INTERFACE_READING.kept_closed_tabs);
  const delay = useSyncExternalStore(subscribe, () => reading().progress_delay_ms, () => DEFAULT_INTERFACE_READING.progress_delay_ms);
  return { kept_closed_tabs: recent, progress_delay_ms: delay };
}
