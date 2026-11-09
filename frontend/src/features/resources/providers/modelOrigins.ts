/** Model observations retain their field sources; reading one never changes a manager's choices. */
import type { ModelSuggestion, ProviderModel } from '@/generated/api-types';
import { isGroupName } from '@/features/models/patterns';

const FIELDS = {
  '/display_name': 'name',
  '/parameters/context_window': 'window',
  '/parameters/offered/context_max': 'context',
  '/parameters/input_images': 'images',
  '/parameters/efforts': 'efforts',
  '/parameters/offered/web_search': 'search',
  '/parameters/offered/report/output_max': 'output',
  '/parameters/offered/report/line': 'line',
  '/parameters/offered/report/lifecycle': 'lifecycle',
  '/parameters/rate/input_per_mtok': 'inputPrice',
  '/parameters/rate/output_per_mtok': 'outputPrice',
  '/parameters/rate/cached_input_per_mtok': 'cachedPrice',
  '/parameters/rate/cache_write_per_mtok': 'cachePrice',
  '/parameters/rate/cache_write_1h_per_mtok': 'hourCachePrice',
  '/parameters/rate/cache_write_12h_per_mtok': 'twelveHourCachePrice',
  '/parameters/rate/web_search_per_call': 'searchPrice',
  '/parameters/rate/web_fetch_per_call': 'fetchPrice',
} as const;
export type ModelField = (typeof FIELDS)[keyof typeof FIELDS];
export interface ModelObservation { pointer: string; field: ModelField; value: unknown; origin: 'vendor' | 'catalogue'; differs: boolean }
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
function at(value: unknown, pointer: string): unknown {
  return pointer.slice(1).split('/').reduce<unknown>((value, key) => record(value) && Object.hasOwn(value, key) ? value[key] : undefined, value);
}
/** Rates keep decimal precision; equal written amounts do not become a false correction. */
function decimal(value: unknown): unknown {
  if (typeof value !== 'string' || !/^\d+(?:\.\d+)?$/.test(value)) return value;
  const [whole = '', fraction = ''] = value.split('.');
  const integer = whole.replace(/^0+(?=\d)/, '');
  const rest = fraction.replace(/0+$/, '');
  return rest === '' ? integer : `${integer}.${rest}`;
}
function equal(pointer: string, first: unknown, second: unknown): boolean {
  if (pointer.startsWith('/parameters/rate/')) return decimal(first) === decimal(second);
  if (Array.isArray(first) && Array.isArray(second) && first.every((one): one is string => typeof one === 'string') && second.every((one): one is string => typeof one === 'string')) {
    return JSON.stringify([...new Set(first)].sort()) === JSON.stringify([...new Set(second)].sort());
  }
  return JSON.stringify(first) === JSON.stringify(second);
}
export function modelObservations(observed: ModelSuggestion, current?: ProviderModel): ModelObservation[] {
  const origins: unknown = 'origins' in observed ? observed.origins : undefined;
  if (!record(origins)) return [];
  return Object.entries(FIELDS).flatMap(([pointer, field]): ModelObservation[] => {
    const origin = origins[pointer];
    const value = at(observed, pointer);
    if ((origin !== 'vendor' && origin !== 'catalogue') || value === undefined || value === null) return [];
    const previous = at(current, pointer);
    return [{ pointer, field, value, origin, differs: previous !== undefined && previous !== null && !equal(pointer, previous, value) }];
  });
}

/** A family is proposed verbatim only when the vendor supplied it and the existing group grammar accepts it. */
export function proposedModelGroup(observed: ModelSuggestion | undefined): string | null {
  const family = observed === undefined ? undefined : modelObservations(observed).find((field) => field.field === 'line' && field.origin === 'vendor');
  const name = typeof family?.value === 'string' ? family.value.trim() : '';
  return isGroupName(name) ? name : null;
}
