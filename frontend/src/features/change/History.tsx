/** How much history a fork or a switch carries: Carry, Carry through a turn (which is what forking from a message
 * asks), Carry some turns, Summary, None; and how it travels.
 *
 * Carry takes the conversation as natively as the new agent program allows: its own session or fork, moved to the
 * other provider as it is, imported turn by turn, or, where it imports nothing, the recent turns written into its
 * context. The turns are those the conversation holds, each by what was asked in it. Summary is the place's summary
 * where a summarizer is chosen and succeeds, and otherwise the last turns briefly. The whole conversation goes along
 * as a file either way. Left unchosen it is what the place carries (`thread.switch_history`), shown as chosen.
 */
import type { TFunction } from 'i18next';
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { formatCredits } from '@/features/usage/figures';
import { useAppearance } from '@/lib/appearance';
import { formatNumber, formatTokens } from '@/lib/format';

import type { Choices, HistoryChoice } from '@/features/change/choices';
import { useFocusFirst } from '@/features/change/Sentence';
import { Hint } from '@/features/help/Hint';
import { settingsQuery } from '@/api/settings';
import { placeHistory, summaryConfigured } from './summary';
import type { ForkPlanHistory, HistorySummaryEstimate, Turn } from '@/generated/api-types';
import { programNameIn } from '@/features/resources/programNames';
import { featureName } from '@/features/resources/agents/buildChecks';
import { useThreadView } from '@/realtime/stores/thread';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';

/** The few words a turn is told by: what the person asked in it. */
function turnLabel(turn: Turn, text: string, t: TFunction, whole = false): string {
  const said = text.trim().replace(/\s+/g, ' ');
  const short = !whole && said.length > 60 ? `${said.slice(0, 57)}…` : said;
  return short === '' ? t('change.history.turn', { n: turn.ordinal }) : t('change.history.turnSaid', { n: turn.ordinal, text: short });
}

/** The turns of a thread, oldest first, each with what the person said in it; empty where the conversation is not held here. */
export function useTurns(threadId: string): { turn: Turn; text: string }[] {
  const view = useThreadView(threadId);
  // The words of a turn are the first message of a person in it: the items are walked in the order they were made.
  const said = new Map<string, string>();
  for (const id of view.order) {
    const item = view.items.get(id);
    if (item?.kind !== 'user_message' || item.turn_id === null || said.has(item.turn_id)) continue;
    said.set(item.turn_id, item.payload.parts.map((part) => ('text' in part && typeof part.text === 'string' ? part.text : '')).join(' '));
  }
  return [...view.turns.values()].sort((one, other) => one.ordinal - other.ordinal).map((turn) => ({ turn, text: said.get(turn.id) ?? '' }));
}

function TurnPick({ label, turns, value, onChange }: { label: string; turns: { turn: Turn; text: string }[]; value: number | null; onChange: (turn: number | null) => void }) {
  const { t } = useTranslation();
  if (turns.length === 0) {
    return <Input label={label} type="number" min={1} inputMode="numeric" value={value === null ? '' : String(value)} onChange={(event) => onChange(event.target.value === '' ? null : Math.max(1, Number(event.target.value)))} fieldClassName="w-(--field-narrow)" />;
  }
  return (
    <Select
      label={label}
      value={value === null ? '' : String(value)}
      onValueChange={(next) => onChange(next === '' ? null : Number(next))}
      options={turns.map(({ turn, text }) => ({ value: String(turn.ordinal), label: turnLabel(turn, text, t), fullLabel: turnLabel(turn, text, t, true) }))}
      fieldClassName="min-w-(--field-wide) flex-1"
    />
  );
}

/** What a summary will send to the summary model and what that costs, as the plan estimates it before it is confirmed. */
function SummaryEstimate({ estimate }: { estimate: HistorySummaryEstimate }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  if (!estimate.available) return null;
  const values = { input: formatTokens(estimate.input_tokens, locale), output: formatTokens(estimate.output_tokens, locale), share: formatNumber(estimate.share, locale) };
  const sends = estimate.window === null
    ? t('change.history.estimateWindowUnknown', values)
    : t('change.history.estimate', { ...values, window: formatTokens(estimate.window, locale) });
  const cost = estimate.credits === null ? null : t('change.history.estimateCost', { credits: formatCredits(Number(estimate.credits), locale) });
  return <span className="text-sm wrap-anywhere text-text-muted tabular-nums" role="status" data-summary-estimate>{cost === null ? sends : `${sends} · ${cost}`}</span>;
}

export interface HistoryProps {
  threadId: string;
  choices: Choices;
  onChange: (patch: Partial<Choices>) => void;
  focus?: boolean;
  planned?: ForkPlanHistory;
}

export function HistoryChoices({ threadId, choices, onChange, focus = false, planned }: HistoryProps) {
  const { t, i18n } = useTranslation();
  const turns = useTurns(threadId);
  const settings = useQuery({ ...settingsQuery(threadId), staleTime: 0 });
  const summary = summaryConfigured(settings.data);
  const history = choices.history;
  const unavailable = planned?.native_import_unavailable;
  const estimate = planned?.summary ?? null;
  const nativeReason = unavailable === undefined || unavailable === null ? null : t('programVersions.featureOff', { feature: featureName('import_native', t), name: programNameIn(unavailable, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown'), version: unavailable.version });
  const mode = history.mode ?? placeHistory(settings.data);
  const set = (patch: Partial<Choices['history']>) => onChange({ history: { ...history, ...patch } });
  const last = turns.at(-1)?.turn.ordinal ?? null;
  const ref = useFocusFirst<HTMLDivElement>(focus);

  const option = (value: HistoryChoice) => ({
    value,
    name: t(`changeCompact.history.${value}`),
    // Without a summarizer a summary can still be chosen: the last turns go briefly instead.
    ...(value === 'summary' && !summary && !settings.isPending ? { description: t('change.history.summaryBrief') } : {}),
  });
  return (
    <div ref={ref} className="flex flex-col gap-3" data-change-history>
      <Select
        label={t('change.history.title')}
        help={<Hint id="switch-history" popover />}
        fieldClassName="w-fit max-w-full"
        value={mode}
        onValueChange={(next) => set({ mode: next, toTurn: next === 'upto' || next === 'range' ? (history.toTurn ?? last) : history.toTurn })}
        options={[option('all'), option('upto'), option('range'), option('summary'), option('none')]}
      />
      {nativeReason !== null && (mode === 'all' || mode === 'upto' || mode === 'range') && <span className="text-sm wrap-anywhere text-text-muted" role="status">{planned?.method === 'neutral_import' ? t('programVersions.neutralCarry', { reason: nativeReason }) : nativeReason}</span>}
      {mode === 'summary' && planned?.mode === 'summary' && estimate !== null && <SummaryEstimate estimate={estimate} />}
      {mode === 'upto' && <TurnPick label={t('change.history.upTo')} turns={turns} value={history.toTurn} onChange={(toTurn) => set({ toTurn })} />}
      {mode === 'range' && (
        <div className="flex flex-wrap items-start gap-3">
          <TurnPick label={t('change.history.from')} turns={turns} value={history.fromTurn ?? (turns[0]?.turn.ordinal ?? 1)} onChange={(fromTurn) => set({ fromTurn })} />
          <TurnPick label={t('change.history.to')} turns={turns} value={history.toTurn ?? last} onChange={(toTurn) => set({ toTurn })} />
        </div>
      )}
    </div>
  );
}
