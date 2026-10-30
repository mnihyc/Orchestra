/** What a turn's tokens were made of, in its footer: a thin bar of fresh input, cache read, cache write, output, reasoning and sub-agents with the numbers and the cache hit rate under the pointer, and one click to the turn's details.
 *
 * The details say the same part by part, then by model, by sub-agent, what the harness's permission
 * reviewer used where it reports that apart, what the turn was counted at where the provider's rates are
 * known, and how full the context was when it ended. A turn whose harness reported nothing has no bar.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import type { TFunction } from 'i18next';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { modelDisplayName } from '@/features/models/names';
import { useShallow } from 'zustand/react/shallow';

import { turnCostQuery } from '@/api/context';
import { turnsRevision } from '@/features/usage/context/ContextPanel';
import { type CostPart, cacheHitRate, costParts, costSegments, COST_LOOKS, freedBy, type Segment } from '@/features/usage/context/fill';
import { PartsBar, Swatch } from '@/features/usage/context/PartsBar';
import { decimal, ESTIMATE_MARK, formatCredits } from '@/features/usage/figures';
import { speedText, timeText } from '@/features/usage/performance';
import type { Item, TokenUsage, Turn } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatPercent, formatTokens } from '@/lib/format';
import { useThreadPart } from '@/realtime/stores/thread';
import { ErrorAlert } from '@/ui/ErrorState';
import { Popover } from '@/ui/Popover';
import { SkeletonRows } from '@/ui/Skeleton';
import { Tooltip } from '@/ui/Tooltip';

/** The name a harness's permission reviewer reports its reviews under. */
const REVIEW_TOOL = 'harness/approvalReview';

/** What the reviews of a turn reported: their models and the use reported apart, where any was. */
export interface Reviews {
  count: number;
  models: string[];
  usage: TokenUsage | null;
}

/** The reviews of a turn, from its items: each a call of the reviewer, with its model and, where reported apart, its use. */
export function reviewsOf(items: readonly Item[]): Reviews {
  const models = new Set<string>();
  let usage: TokenUsage | null = null;
  let count = 0;
  for (const item of items) {
    if (item.kind !== 'tool_call' || item.payload.tool.kind !== 'other' || item.payload.tool.name !== REVIEW_TOOL) continue;
    count += 1;
    const result = item.payload.tool.result as { model?: unknown; usage?: unknown } | null | undefined;
    if (typeof result?.model === 'string') models.add(result.model);
    const used = result?.usage as TokenUsage | null | undefined;
    if (typeof used?.total_tokens === 'number') {
      usage = usage === null ? { ...used } : {
        input_tokens: usage.input_tokens + used.input_tokens,
        cached_input_tokens: usage.cached_input_tokens + used.cached_input_tokens,
        cache_write_tokens: (usage.cache_write_tokens ?? 0) + (used.cache_write_tokens ?? 0),
        output_tokens: usage.output_tokens + used.output_tokens,
        reasoning_tokens: usage.reasoning_tokens + used.reasoning_tokens,
        total_tokens: usage.total_tokens + used.total_tokens,
      };
    }
  }
  return { count, models: [...models].sort(), usage };
}

/** The words of each part of a turn's use. */
function costWords(part: CostPart, tokens: number, locale: string, t: TFunction): string {
  return t(`contextUsage.cost.${part}`, { tokens: formatTokens(tokens, locale) });
}

/** What a turn used, in one line: its parts and the cache hit rate. */
export function useCostSummary(usage: TokenUsage, detail: Turn['usage_detail']): string {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const parts = costParts(usage, detail).map((part) => costWords(part.part, part.tokens, locale, t));
  const hit = cacheHitRate(usage);
  if (hit !== null && usage.cached_input_tokens > 0) parts.push(t('contextUsage.cost.hitRate', { percent: formatPercent(hit, locale) }));
  return new Intl.ListFormat(locale).format(parts);
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-1">
      <h3 className="text-xs font-semibold text-text-muted">{title}</h3>
      {children}
    </section>
  );
}

function Row({ name, value, swatch }: { name: ReactNode; value: ReactNode; swatch?: Pick<Segment, 'look'> }) {
  return (
    <li className="flex items-baseline justify-between gap-3 text-sm">
      <span className="flex min-w-0 items-center gap-2 wrap-anywhere">{swatch !== undefined && <Swatch segment={swatch} />}{name}</span>
      <span className="shrink-0 text-text-muted tabular-nums">{value}</span>
    </li>
  );
}

/** The details of one turn's use. */
export function TurnCostDetails({ threadId, turn }: { threadId: string; turn: Turn }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const revision = useThreadPart(threadId, turnsRevision);
  const cost = useQuery(turnCostQuery(threadId, turn.id, revision));
  const items = useThreadPart(threadId, useShallow((view) => view.order.map((id) => view.items.get(id)).filter((item): item is Item => item?.turn_id === turn.id)));
  const subagentNames = useThreadPart(
    threadId,
    useShallow((view) => {
      const names: Record<string, string> = {};
      for (const item of view.items.values()) {
        if (item.kind === 'tool_call' && item.payload.tool.kind === 'subagent') names[item.payload.tool.child_stream] = item.payload.tool.name ?? item.payload.tool.role ?? item.payload.title;
      }
      return names;
    }),
  );
  const usage = turn.usage;
  if (usage === null) return null;
  const detail = turn.usage_detail ?? null;
  const segments = costSegments(usage, detail);
  const hit = cacheHitRate(usage);
  const reviews = reviewsOf(items);
  const reportedModels = detail?.models ?? [];
  const models = reportedModels.length > 0 ? reportedModels.map((model) => ({ name: model.model, tokens: model.tokens.total_tokens })) : [{ name: turn.model_display, tokens: usage.total_tokens }];
  const subagentShares = detail?.by_subagent ?? [];
  const contextUsed = detail?.context_used ?? null;
  const contextWindow = detail?.context_window ?? null;
  const shortened = detail?.shortenings ?? [];
  return (
    <div className="flex w-(--container-form) max-w-full flex-col gap-3" data-turn-cost={turn.id}>
      <h2 className="text-base font-semibold text-text">{t('contextUsage.turn.title', { ordinal: turn.ordinal, tokens: formatTokens(usage.total_tokens, locale) })}</h2>
      <ul className="flex flex-col gap-1">
        <Row name={t('usagePerf.modelTime')} value={timeText(cost.data?.performance?.model_ms, locale, t)} />
        <Row name={t('usagePerf.toolTime')} value={timeText(cost.data?.performance?.tool_ms, locale, t)} />
        <Row name={t('usagePerf.requestFirstToken')} value={timeText(cost.data?.performance?.first_output_ms, locale, t)} />
        <Row name={t('usagePerf.speed')} value={speedText(cost.data?.performance?.speed_milli === null || cost.data?.performance?.speed_milli === undefined ? null : cost.data.performance.speed_milli / 1000, locale, t)} />
      </ul>
      {cost.data?.performance?.reasoning_hidden && <p className="text-sm text-text-muted">{t('usagePerf.includesThinking')}</p>}
      {cost.data?.performance?.tool_includes_subagents && <p className="text-sm text-text-muted">{t('usagePerf.toolIncludesSubagents')}</p>}
      {cost.data?.performance?.tool_includes_unmeasured && <p className="text-sm text-text-muted">{t('usagePerf.toolIncludesUnmeasured')}</p>}
      <PartsBar segments={segments} label={t('contextUsage.turn.bar')} describe={(segment) => `${costWords(segment.key, segment.tokens, locale, t)} (${formatPercent(segment.share, locale)})`} />
      <ul className="flex flex-col gap-1">
        {segments.map((segment) => (
          <Row key={segment.key} swatch={segment} name={t(`contextUsage.cost.name.${segment.key}`)} value={`${formatTokens(segment.tokens, locale)} (${formatPercent(segment.share, locale)})`} />
        ))}
      </ul>
      {hit !== null && <p className="text-sm text-text-muted">{t('contextUsage.turn.hitRate', { percent: formatPercent(hit, locale) })}</p>}
      <Section title={t('contextUsage.turn.byModel')}>
        <ul className="flex flex-col gap-1">{models.map((model) => <Row key={model.name} name={model.name} value={formatTokens(model.tokens, locale)} />)}</ul>
      </Section>
      {subagentShares.length > 0 && (
        <Section title={t('contextUsage.turn.bySubagent')}>
          <ul className="flex flex-col gap-1">
            {subagentShares.map((share) => <Row key={share.stream} swatch={{ look: COST_LOOKS.subagents }} name={subagentNames[share.stream] ?? t('contextUsage.turn.subagentUnnamed')} value={formatTokens(share.tokens.total_tokens, locale)} />)}
          </ul>
        </Section>
      )}
      {reviews.count > 0 && (
        <Section title={t('contextUsage.turn.reviewer')}>
          <p className="text-sm text-text-muted">
            {reviews.usage === null
              ? t('contextUsage.turn.reviewerInside', { count: reviews.count, models: reviews.models.join(', ') })
              : t('contextUsage.turn.reviewerApart', { count: reviews.count, models: reviews.models.join(', '), tokens: formatTokens(reviews.usage.total_tokens, locale) })}
          </p>
        </Section>
      )}
      <Section title={t('contextUsage.turn.price')}>
        {cost.isError ? (
          <ErrorAlert error={cost.error} />
        ) : cost.data === undefined ? (
          <SkeletonRows rows={1} />
        ) : cost.data.rows.length === 0 ? (
          <p className="text-sm text-text-muted">{t(turn.completed_at === null ? 'contextUsage.turn.priceRunning' : 'contextUsage.turn.priceNone')}</p>
        ) : (
          <ul className="flex flex-col gap-1">
            {cost.data.rows.map((row) => (
              <Row
                key={row.model}
                name={modelDisplayName(row.model_display,t)}
                value={row.credits === null ? t('contextUsage.turn.noRate') : `${row.rate_source === 'catalogue' ? `${ESTIMATE_MARK} ` : ''}${t('contextUsage.turn.credits', { credits: formatCredits(decimal(row.credits), locale) })}`}
              />
            ))}
          </ul>
        )}
      </Section>
      {contextUsed !== null && (
        <Section title={t('contextUsage.turn.context')}>
          <p className="text-sm text-text-muted tabular-nums">
            {contextWindow === null
              ? t('contextUsage.turn.contextBare', { used: formatTokens(contextUsed, locale) })
              : t('contextUsage.turn.contextOf', { used: formatTokens(contextUsed, locale), window: formatTokens(contextWindow, locale), percent: formatPercent(contextUsed / contextWindow, locale) })}
            {shortened.map((shortening) => {
              const freed = freedBy(shortening);
              return <span key={shortening.at} className="ml-3">{freed === null ? t('contextUsage.turn.shortened') : t('contextUsage.turn.shortenedFreed', { tokens: formatTokens(freed, locale) })}</span>;
            })}
          </p>
        </Section>
      )}
      <Link to="/usage" search={{ scope: `node:${threadId}`, view: 'requests', turn: turn.id, range: 'all' }} className="self-start text-sm text-accent-text hover:underline">{t('usagePerf.turnRequests')}</Link>
    </div>
  );
}

/** The tokens of a turn in its footer, with the bar of what they were made of; one click opens its details. */
export function TurnCost({ threadId, turn }: { threadId: string; turn: Turn }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const usage = turn.usage;
  const summary = useCostSummary(usage ?? { input_tokens: 0, cached_input_tokens: 0, output_tokens: 0, reasoning_tokens: 0, total_tokens: 0 }, turn.usage_detail ?? null);
  if (usage === null) return null;
  const segments = costSegments(usage, turn.usage_detail ?? null);
  const tokens = t('timeline.footer.tokens', { tokens: formatTokens(usage.total_tokens, locale), count: usage.total_tokens });
  const label = summary.trim().length > 0
    ? t('contextUsage.turn.openWithSummary', { tokens, summary })
    : t('contextUsage.turn.open', { tokens });
  return (
    <Popover
      label={t('contextUsage.turn.title', { ordinal: turn.ordinal, tokens: formatTokens(usage.total_tokens, locale) })}
      side="top"
      align="start"
      className="max-h-(--radix-popover-content-available-height) overflow-y-auto"
      trigger={
        <Tooltip content={summary}>
          <button type="button" aria-label={label} className="inline-flex min-h-6 min-w-6 items-center gap-2 rounded-md underline decoration-dotted underline-offset-2" data-turn-cost-open={turn.id}>
            <span>{tokens}</span>
            {segments.length > 1 && <span className="inline-flex w-12" aria-hidden><PartsBar segments={segments} label={t('contextUsage.turn.bar')} describe={() => ''} size="thin" focusable={false} /></span>}
          </button>
        </Tooltip>
      }
    >
      <TurnCostDetails threadId={threadId} turn={turn} />
    </Popover>
  );
}
