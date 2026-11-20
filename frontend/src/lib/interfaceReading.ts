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
const optionalCount=(value:unknown):number|undefined=>typeof value==='number'&&Number.isSafeInteger(value)&&value>=0?value:undefined;

export function interfaceReading(state: Record<string, unknown> | undefined, meta?: Meta) {
  const defaults = meta?.interface_defaults ?? DEFAULT_INTERFACE_READING;
  const own = record(state?.interface) ? state.interface : {};
  const personal=state===undefined?undefined:optionalCount(own.catch_up_above_items);
  const installation=state===undefined?undefined:optionalCount(meta?.interface_defaults.catch_up_above_items);
  return { kept_closed_tabs: count(own.kept_closed_tabs, defaults.kept_closed_tabs), progress_delay_ms: count(own.progress_delay_ms, defaults.progress_delay_ms),
    catch_up_above_items:personal??installation,catch_up_source:personal!==undefined?'personal' as const:installation!==undefined?'system' as const:null };
}

export function useInterfaceReading() {
  const cache = useContext(QueryClientContext);
  const subscribe = useCallback((changed: () => void) => cache?.getQueryCache().subscribe(changed) ?? (() => undefined), [cache]);
  const reading = () => interfaceReading(cache?.getQueryData<Record<string, unknown>>(['me-state']), cache?.getQueryData<Meta>(['meta']));
  const recent = useSyncExternalStore(subscribe, () => reading().kept_closed_tabs, () => DEFAULT_INTERFACE_READING.kept_closed_tabs);
  const delay = useSyncExternalStore(subscribe, () => reading().progress_delay_ms, () => DEFAULT_INTERFACE_READING.progress_delay_ms);
  const ready = useSyncExternalStore(subscribe, () => cache?.getQueryState(['meta'])?.status === 'success' && cache.getQueryState(['me-state'])?.status === 'success', () => false);
  const catchup=()=>interfaceReading(cache?.getQueryState(['me-state'])?.status==='success'?cache.getQueryData<Record<string,unknown>>(['me-state']):undefined,
    cache?.getQueryState(['meta'])?.status==='success'?cache.getQueryData<Meta>(['meta']):undefined);
  const catch_up_above_items=useSyncExternalStore(subscribe,()=>catchup().catch_up_above_items,()=>undefined);
  const catch_up_source=useSyncExternalStore(subscribe,()=>catchup().catch_up_source,()=>null);
  return { kept_closed_tabs: recent, progress_delay_ms: delay, ready,catch_up_above_items,catch_up_source };
}
