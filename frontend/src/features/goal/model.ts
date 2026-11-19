/** What a goal says of itself, as the screens put it in words: its chip, its reason, its limits and its figures. */
import type { TFunction } from 'i18next';

import type { ThreadGoal } from '@/generated/api-types';
import { formatNumber, formatTokens } from '@/lib/format';
import { formatSpan } from '@/lib/time';
import { formatCredits } from '@/features/usage/figures';

/** The five states a person reads on the goal line. */
export type GoalChip = 'working' | 'waiting' | 'paused' | 'met' | 'stopped';

/** The chip of a goal: an active goal that waits for a person says so, a goal the agent is blocked on too. */
export function chipOf(goal: ThreadGoal): GoalChip {
  switch (goal.status) {
    case 'active': return goal.waiting === null ? 'working' : 'waiting';
    case 'paused': return goal.reason === 'blocked' ? 'waiting' : 'paused';
    case 'met': return 'met';
    default: return 'stopped';
  }
}

/** Whether the goal is still the loop's to run: nothing is over yet. */
export const isLive = (goal: ThreadGoal): boolean => goal.status === 'active' || goal.status === 'paused';

/** The words of the reason a goal is paused or stopped, with the limit that was reached where it was one. */
export function reasonWords(goal: ThreadGoal, t: TFunction, locale: string): string | null {
  const reason = goal.reason;
  if (reason === null) return null;
  switch (reason) {
    case 'turns': return t('goals.reason.turns', { limit: formatNumber(goal.budgets.max_turns, locale) });
    case 'minutes': return t('goals.reason.minutes', { limit: formatNumber(goal.budgets.max_minutes, locale) });
    case 'credits': return t('goals.reason.credits', { limit: formatCredits(Number(goal.budgets.max_credits), locale) });
    default: return t(`goals.reason.${reason}`);
  }
}

/** What an active goal that does not work waits for, in words. */
export function waitingWords(goal: ThreadGoal, t: TFunction): string | null {
  return goal.waiting === null ? null : t(`goals.waiting.${goal.waiting}`);
}

/** Whether a limit that stopped the goal is still reached, so that resuming would stop it again at once. */
export function limitStillReached(goal: ThreadGoal): boolean {
  if (goal.status !== 'paused' && goal.status !== 'stopped') return false;
  const { budgets, used } = goal;
  if (goal.reason === 'turns') return budgets.max_turns > 0 && used.turns >= budgets.max_turns;
  if (goal.reason === 'minutes') return budgets.max_minutes > 0 && used.elapsed_secs >= budgets.max_minutes * 60;
  if (goal.reason === 'credits') return Number(budgets.max_credits) > 0 && Number(used.credits) >= Number(budgets.max_credits);
  return false;
}

/** How long the goal has been active at `now`, in milliseconds: what it had before its run began and what has run since. */
export function elapsedMs(goal: ThreadGoal, now: number): number {
  const base = goal.used.elapsed_secs * 1000;
  const since = goal.used.running_since === null ? Number.NaN : Date.parse(goal.used.running_since);
  return Number.isNaN(since) ? base : base + Math.max(0, now - since);
}

/** The figures of the line: turns, the time active and tokens, each as the formatters write it. */
export function figures(goal: ThreadGoal, t: TFunction, locale: string, elapsed: number): string[] {
  const turns = goal.budgets.max_turns > 0
    ? t('goals.line.turnsOf', { count: goal.used.turns, max: goal.budgets.max_turns, number: formatNumber(goal.used.turns, locale) })
    : t('goals.line.turns', { count: goal.used.turns, number: formatNumber(goal.used.turns, locale) });
  const parts = [turns, formatSpan(elapsed, { locale }, 'short')];
  if (goal.used.tokens > 0) parts.push(t('goals.line.tokens', { tokens: formatTokens(goal.used.tokens, locale) }));
  if (Number(goal.budgets.max_credits) > 0) parts.push(t('goals.line.credits', { credits: formatCredits(Number(goal.used.credits), locale) }));
  return parts;
}
