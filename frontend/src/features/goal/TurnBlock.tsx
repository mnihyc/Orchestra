/** The words that stand in the timeline for an input of a goal's loop: where the goal is, in the reader's language.
 *
 * "Goal: keep working until npm test exits 0", and under it, quiet, "Turn 7 of up to 30 · 1.2M tokens"; and, where the
 * check of the last report found it not enough yet, why. What the agent was told is longer and in English; this is what a
 * person needs to see.
 */
import { useTranslation } from 'react-i18next';

import type { GoalInputFacts } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatTokens } from '@/lib/format';

export function GoalTurnBlock({ facts }: { facts: GoalInputFacts }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const turn = facts.max_turns > 0 ? t('goals.turn.numberOf', { turn: facts.turn, max: facts.max_turns }) : t('goals.turn.number', { turn: facts.turn });
  const detail = [turn, facts.tokens > 0 ? t('goals.turn.tokens', { tokens: formatTokens(facts.tokens, locale) }) : '', facts.plan ? t('goals.turn.plan') : '']
    .filter((part) => part !== '').join(' · ');
  return (
    <div className="flex min-w-0 flex-col gap-0.5" data-goal-turn>
      <p className="wrap-anywhere text-text">{t('goals.turn.headline', { condition: facts.condition })}</p>
      <p className="text-xs tabular-nums text-text-subtle">{detail}</p>
      {facts.not_yet !== null && <p className="wrap-anywhere text-text-muted">{t('goals.turn.notYet', { reason: facts.not_yet })}</p>}
    </div>
  );
}
