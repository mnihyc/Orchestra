/** When an answer was made, and what its turn used, under the pointer on the answer's toolbar.
 *
 * The time of day stands in the toolbar. Under the pointer are the exact moment, how long the turn worked, and its
 * tokens as the harness counted them: input, cached input and output, by model where it says so. One click opens the
 * turn's details. No sum of the requests is drawn as a figure of its own: it is not what a turn used.
 */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';

import { TurnCostDetails } from '@/features/usage/context/TurnCost';
import type { Turn } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatTokens } from '@/lib/format';
import { formatExact, formatSpan, formatTimeOfDay } from '@/lib/time';
import { cx } from '@/ui/cx';
import { Popover } from '@/ui/Popover';
import { Tooltip } from '@/ui/Tooltip';
import { turnActiveMs } from './turnTime';

/** The lines of a turn's use: input, cached input and output; for each model, where the harness reports its use by model. */
export function figureLines(turn: Turn, locale: string, t: TFunction): string[] {
  const usage = turn.usage;
  if (usage === null) return [];
  const one = (name: string | null, tokens: { input_tokens: number; cached_input_tokens: number; output_tokens: number }): string => [
    ...(name === null ? [] : [name]),
    t('timeline.figures.input', { tokens: formatTokens(tokens.input_tokens, locale) }),
    ...(tokens.cached_input_tokens > 0 ? [t('timeline.figures.cached', { tokens: formatTokens(tokens.cached_input_tokens, locale) })] : []),
    t('timeline.figures.output', { tokens: formatTokens(tokens.output_tokens, locale) }),
  ].join(' · ');
  const models = (turn.usage_detail?.models ?? []).filter((model) => model.tokens_reported !== false);
  return models.length === 0 ? [one(null, usage)] : models.map((model) => one(model.model, model.tokens));
}

export function TurnFigures({ threadId, turn, at }: { threadId: string; turn: Turn; at: string }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const took = turn.completed_at === null ? null : turnActiveMs(turn, Date.parse(turn.completed_at));
  const lines = [
    formatExact(at, format),
    ...(took !== null && took >= 1000 ? [t('timeline.footer.duration', { time: formatSpan(took, format) })] : []),
    ...figureLines(turn, format.locale, t),
  ];
  const tip = <span className="flex flex-col gap-0.5">{lines.map((line, index) => <span key={index}>{line}</span>)}</span>;
  const label = <time dateTime={at}>{formatTimeOfDay(at, format)}</time>;
  const face = 'inline-flex min-h-6 items-center rounded-md px-1 text-xs text-text-subtle tabular-nums hover:text-text';
  if (turn.usage === null) return <Tooltip content={tip}><span tabIndex={0} className={face} data-turn-figures={turn.id}>{label}</span></Tooltip>;
  return (
    <Popover
      label={t('timeline.figures.title', { ordinal: turn.ordinal })}
      side="top"
      align="start"
      className="max-h-(--radix-popover-content-available-height) overflow-y-auto"
      trigger={<Tooltip content={tip}><button type="button" className={cx(face, 'underline decoration-dotted underline-offset-2')} data-turn-figures={turn.id}>{label}</button></Tooltip>}
    >
      <TurnCostDetails threadId={threadId} turn={turn} />
    </Popover>
  );
}
