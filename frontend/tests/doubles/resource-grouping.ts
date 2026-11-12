/** Fixtures explicitly report no origin and no machine judgement unless a deciding case supplies them. */
import type { Agent, Machine } from '../../src/generated/api-types.ts';

export function unjudgedProvenance(): Pick<Agent, 'origin_machine' | 'availability'> {
  return { origin_machine: null, availability: [] };
}

/** No home was projected; this is not a claim that an option is available everywhere. */
export function unplacedGrouping(): Pick<Machine, 'home' | 'home_path' | 'origin_machine' | 'availability'> {
  return { home: null, home_path: [], ...unjudgedProvenance() };
}
