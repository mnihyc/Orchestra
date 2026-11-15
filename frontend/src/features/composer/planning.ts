/** Ordinary inputs and reviews use the same explicit choice or first-message place default. */
import type { ResolvedSetting, Thread } from '@/generated/api-types';

export function planningFor(thread: Thread | null, chosen: boolean | undefined, settings: readonly Pick<ResolvedSetting, 'key' | 'value'>[] | undefined): boolean {
  if (thread?.binding?.plan_mode !== true || thread.binding.program?.build?.check?.off.includes('plan_mode') === true) return false;
  return chosen ?? (thread.usage === null && thread.current_turn_id === null && settings?.some((setting) => setting.key === 'thread.plan_first' && setting.value === true) === true);
}

/** Only the implicit first-message choice depends on reading a place default. */
export function needsPlanDefault(thread: Thread | null, chosen: boolean | undefined): boolean {
  return thread?.binding?.plan_mode === true && thread.binding.program?.build?.check?.off.includes('plan_mode') !== true
    && chosen === undefined && thread.usage === null && thread.current_turn_id === null;
}
