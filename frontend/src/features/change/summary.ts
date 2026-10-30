/** What this conversation's inherited configuration says of carrying its history: whether it chooses a summary
 * producer, and what a switch or a fork carries when the person chooses nothing. */
import type { ResolvedSetting } from '@/generated/api-types';

export function summaryConfigured(settings: readonly Pick<ResolvedSetting, 'key' | 'value'>[] | undefined): boolean {
  const value = settings?.find((setting) => setting.key === 'auto.summarizer')?.value;
  return value !== null && typeof value === 'object' && !Array.isArray(value) && 'kind' in value && typeof value.kind === 'string' && value.kind.trim() !== '';
}

/** The place's `thread.switch_history`, as the history choice names it: carry is all of it, as natively as the new agent program allows. */
export function placeHistory(settings: readonly Pick<ResolvedSetting, 'key' | 'value'>[] | undefined): 'all' | 'summary' | 'none' {
  const value = settings?.find((setting) => setting.key === 'thread.switch_history')?.value;
  return value === 'summary' || value === 'none' ? value : 'all';
}
