/** Personal reading choices override installation defaults without adding a read to a hidden view. */
import { QueryClientContext } from '@tanstack/react-query';
import { useCallback, useContext, useSyncExternalStore } from 'react';
import type { Meta, PersonalStateKey } from '@/generated/api-types';

/** Reading choices have their own declared native personal-state key. */
export const READING_STATE_KEY: PersonalStateKey = 'interface';

// Used while the shell's metadata is still arriving; the registry supplies the same defaults.
export const DEFAULT_INTERFACE_READING = { kept_closed_tabs: 10, progress_delay_ms: 200 };
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const count = <Fallback extends number | null>(value: unknown, fallback: Fallback): number | Fallback => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : fallback;

export function interfaceReading(state: Record<string, unknown> | undefined, meta?: Meta) {
  const reported = meta?.interface_defaults;
  const defaults = reported ?? DEFAULT_INTERFACE_READING;
  const own = record(state?.interface) ? state.interface : {};
  const catchUpDefault = count(reported?.catch_up_above_items, null);
  const catchUpOwn = count(own.catch_up_above_items, null);
  return { kept_closed_tabs: count(own.kept_closed_tabs, defaults.kept_closed_tabs), progress_delay_ms: count(own.progress_delay_ms, defaults.progress_delay_ms),
    catch_up_above_items: catchUpOwn ?? catchUpDefault, catch_up_above_items_own: catchUpOwn, catch_up_above_items_default: catchUpDefault,
    catch_up_above_items_source: reported?.catch_up_above_items_source ?? null };
}

export function useInterfaceReading() {
  const cache = useContext(QueryClientContext);
  const subscribe = useCallback((changed: () => void) => cache?.getQueryCache().subscribe(changed) ?? (() => undefined), [cache]);
  const reading = () => interfaceReading(cache?.getQueryData<Record<string, unknown>>(['me-state']), cache?.getQueryData<Meta>(['meta']));
  const recent = useSyncExternalStore(subscribe, () => reading().kept_closed_tabs, () => DEFAULT_INTERFACE_READING.kept_closed_tabs);
  const delay = useSyncExternalStore(subscribe, () => reading().progress_delay_ms, () => DEFAULT_INTERFACE_READING.progress_delay_ms);
  const confirmed = () => interfaceReading(
    cache?.getQueryState(['me-state'])?.status === 'success' ? cache.getQueryData<Record<string, unknown>>(['me-state']) : undefined,
    cache?.getQueryState(['meta'])?.status === 'success' ? cache.getQueryData<Meta>(['meta']) : undefined);
  const catchUp = useSyncExternalStore(subscribe, () => confirmed().catch_up_above_items, () => null);
  const catchUpOwn = useSyncExternalStore(subscribe, () => confirmed().catch_up_above_items_own, () => null);
  const catchUpDefault = useSyncExternalStore(subscribe, () => confirmed().catch_up_above_items_default, () => null);
  const ready = useSyncExternalStore(subscribe, () => cache?.getQueryState(['meta'])?.status === 'success' && cache.getQueryState(['me-state'])?.status === 'success', () => false);
  const personalReady = useSyncExternalStore(subscribe, () => cache?.getQueryState(['me-state'])?.status === 'success', () => false);
  const catchUpSource = useSyncExternalStore(subscribe, () => confirmed().catch_up_above_items_source, () => null);
  return { kept_closed_tabs: recent, progress_delay_ms: delay, catch_up_above_items: catchUp, catch_up_above_items_own: catchUpOwn, catch_up_above_items_default: catchUpDefault,
    catch_up_above_items_source: catchUpSource, ready, personalReady };
}
