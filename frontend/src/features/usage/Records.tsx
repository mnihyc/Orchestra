/** The ledger rows behind what is on screen, each followed to its turn, with the rate its credits were counted at and where that rate came from.
 *
 * Every figure of the screen is the sum of rows that are listed here, newest first and a page at a time,
 * as far back as the first turn. A row says when, in which thread, whose it is, what model it ran on and
 * how much it came to; its thread opens for whoever may read it. Use that had no rate reads "–" and never 0,
 * a credit counted at an estimate carries "≈", and the rest of a row (what it ran on, how large its prompt
 * was, who reported it, and the rate it was counted at with where that came from) is behind the button
 * beside its credits.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { Info } from 'lucide-react';
import { memo } from 'react';
import { Section } from '@/shell/Page';
import { useTranslation } from 'react-i18next';

import { eventsQuery } from '@/api/usage';
import type { Rate, UsageEvent } from '@/generated/api-types';
import { EventCredits } from '@/features/usage/Amounts';
import { type Span, type UsageScope, type UsageSearch, recordsParams } from '@/features/usage/search';
import { effortDisplayName, modelDisplayName } from '@/features/models/names';
import { programNameIn } from '@/features/resources/programNames';
import { offeredChoice } from '@/i18n/choices';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber, formatTokens } from '@/lib/format';
import { IconButton } from '@/ui/IconButton';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { Popover } from '@/ui/Popover';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';

/** The largest step of a rate whose prompt size the row reached, or none: the prices that row was counted at beyond the ordinary ones. */
export function stepReached(rate: Rate, contextTokens: number | null): number | null {
  if (contextTokens === null) return null;
  const steps = (rate.above ?? []).filter((step) => step.context_tokens < contextTokens).map((step) => step.context_tokens);
  return steps.length === 0 ? null : Math.max(...steps);
}

/** What the rate of a row says, in words: the three prices, the speed's multiple, the step a large prompt crossed. */
function RateWords({ event }: { event: UsageEvent }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  if (event.rate === null || event.rate_source === null) return <p>{t('usage.records.noRate')}</p>;
  const rate = event.rate;
  const step = stepReached(rate, event.context_tokens);
  const multiple = event.tier === null ? undefined : rate.tiers?.[event.tier];
  const where = event.rate_source === 'provider' ? t('usage.records.fromProvider', { provider: event.provider.name }) : event.rate_source === 'rate_card' ? t('usage.records.fromCard') : t('usage.records.fromCatalogue');
  return (
    <div className="flex flex-col gap-1">
      <p className="text-text">{t('usage.records.prices', { input: rate.input_per_mtok, cached: rate.cached_input_per_mtok, output: rate.output_per_mtok })}</p>
      {step !== null && <p>{t('usage.records.step', { tokens: formatTokens(step, locale) })}</p>}
      {multiple !== undefined && event.tier !== null && <p>{t('usage.records.multiple', { tier: event.tier, multiple })}</p>}
      <p>{where}</p>
    </div>
  );
}

/** What else a record says: what it ran on, how large its prompt was, who reported it, and the rate it was counted at. */
function RecordButton({ event }: { event: UsageEvent }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const where = [event.environment?.name ?? t('usage.key.noEnvironment'), event.environment_kind === null ? null : event.environment_kind === 'managed' ? t('usage.key.managed') : t('usage.key.connected')].filter((part): part is string => part !== null);
  const program = programNameIn(event, locale) ?? programNameIn(event.agent, locale) ?? t('resources.harnessNameUnknown');
  const agent = event.agent?.name !== undefined && event.agent.name !== event.harness_kind ? event.agent.name : event.harness_kind === null ? t('usage.key.noAgent') : program;
  return (
    <Popover
      label={t('usage.records.aboutRecord')}
      side="left"
      trigger={<IconButton size="sm" label={t('usage.records.aboutRecord')} icon={<Info aria-hidden />} />}
      className="w-[20rem] text-sm text-text-muted"
    >
      <div className="flex flex-col gap-2">
        <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1">
          <dt>{t('usage.records.provider')}</dt>
          <dd className="text-text">{event.provider.name}</dd>
          <dt>{t('usage.records.agent')}</dt>
          <dd className="text-text">{agent}</dd>
          <dt>{t('usage.records.environment')}</dt>
          <dd className="text-text">{where.join(t('startFlow.separator'))}</dd>
          {event.context_tokens !== null && (
            <>
              <dt>{t('usage.records.promptSize')}</dt>
              <dd className="text-text tabular-nums">{formatNumber(event.context_tokens, locale)}</dd>
            </>
          )}
          <dt>{t('usage.records.reportedBy')}</dt>
          <dd className="text-text">{t(`usage.records.origin.${event.origin}`)}</dd>
        </dl>
        <div className="border-t border-border pt-2">
          <RateWords event={event} />
        </div>
      </div>
    </Popover>
  );
}

function Thread({ event,scope }: { event: UsageEvent;scope:UsageScope }) {
  const { t } = useTranslation();
  const topic = event.topic.redacted || event.topic.name === null ? t('usage.key.hidden', { kind: t('usage.key.kind.topic') }) : event.topic.name;
  const context=[event.project,event.topic].filter((place)=>scope.kind!=='node' || place.id!==scope.id).map((place)=>place.redacted || place.name===null ? t('usage.key.hidden',{kind:t(`usage.key.kind.${place.kind}`)}) : place.name).join(' / ');
  if (event.thread === null) {
    return (
      <span className="flex min-w-0 flex-col">
        <span className="wrap-anywhere text-text">{t(event.purpose === 'embeddings' ? 'usage.records.embeddings' : 'usage.records.summary')}</span>
        {!(scope.kind==='node' && scope.id===event.topic.id) && <span className="wrap-anywhere text-sm text-text-muted">{topic}</span>}
      </span>
    );
  }
  const thread = event.thread;
  const readable = !thread.redacted && thread.name !== null;
  return (
    <span className="flex min-w-0 flex-col">
      {readable ? (
        <Link to="/thread/$threadId" params={{ threadId: thread.id }} search={event.turn_ordinal === null ? {} : { turn:event.turn_ordinal }} className="font-medium wrap-anywhere text-text hover:underline">
          {scope.kind==='node' && scope.id===thread.id && event.turn_ordinal!==null ? t('usage.records.turn',{id:event.turn_ordinal}) : thread.name}
        </Link>
      ) : (
        <span className="wrap-anywhere text-text-muted">{t('usage.key.hidden', { kind: t('usage.key.kind.thread') })}</span>
      )}
      <span className="flex flex-wrap gap-x-3 gap-y-1 text-sm wrap-anywhere text-text-muted">
        {context!=='' && <span>{context}</span>}{event.turn_ordinal!==null && readable && !(scope.kind==='node' && scope.id===thread.id) && <span>{t('usage.records.turn',{id:event.turn_ordinal})}</span>}
      </span>
    </span>
  );
}

function ModelLine({ event,scope }: { event: UsageEvent;scope:UsageScope }) {
  const {t}=useTranslation();
  const parts = [
    ...(event.effort===null?[]:[{name:t('usage.chip.effort'),value:effortDisplayName(event.effort,t)}]),
    ...(event.tier===null?[]:[{name:t('usage.chip.tier'),value:offeredChoice(t,'serviceTier',event.tier)?.name??event.tier}]),
    ...(scope.kind==='provider'?[]:[{name:t('usage.chip.provider'),value:event.provider.name}]),
  ];
  return (
    <span className="flex min-w-0 flex-col">
      <span className="wrap-anywhere text-text">
        {modelDisplayName(event.model_display,t)}
      </span>
      {parts.map((part)=><span key={part.name} className="flex flex-wrap gap-x-2 gap-y-1 text-sm wrap-anywhere text-text-muted"><span>{part.name}</span><span>{part.value}</span></span>)}
    </span>
  );
}

export interface RecordsProps {
  scope: UsageScope;
  search: UsageSearch;
  span: Span;
  /** What the reader was shown, so that a summary can say it lists their own records alone. */
  depth: 'own' | 'summary' | 'reports' | null;
  className?: string;
}

function RecordsView({ scope, search, span, depth, className }: RecordsProps) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const query = useInfiniteQuery(eventsQuery(recordsParams(scope, search, span)));
  const rows = query.data?.pages.flatMap((page) => page.items) ?? [];

  // What the room does not hold is told under the thread, and what is not told is behind the button beside the credits: a record is read at a glance, and opened for the rest.
  const columns: Column<UsageEvent>[] = [
    {
      id: 'thread',
      header: t('usage.records.thread'),
      cell: (event, shown) => {
        const folded = [
          ...(shown.has('when') ? [] : [<RelativeTime key="when" at={event.at} />]),
          // One's own records are all one's own.
          ...(shown.has('who') || scope.kind === 'me' ? [] : [event.user === null ? t('usage.key.nobody') : event.user.display_name]),
          ...(shown.has('model') ? [] : [modelDisplayName(event.model_display,t)]),
          ...(shown.has('tokens') ? [] : [formatTokens(event.tokens.total_tokens, locale)]),
        ];
        return (
          <span className="flex min-w-0 flex-col">
            <Thread event={event} scope={scope} />
            {folded.length > 0 && (
              <span className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-text-muted">
                {folded.map((part, at) => (
                  // The parts are few and in a fixed order.
                  <span key={at}>{part}</span>
                ))}
              </span>
            )}
          </span>
        );
      },
    },
    { id: 'when', header: t('usage.records.when'), from: 'sm', className: 'whitespace-nowrap', cell: (event) => <RelativeTime at={event.at} className="whitespace-nowrap text-text-muted" /> },
    ...(scope.kind === 'me'
      ? []
      : [
          {
            id: 'who',
            header: t('usage.records.who'),
            from: 'lg' as const,
            cell: (event: UsageEvent) => (event.user === null ? <span className="text-text-muted">{t('usage.key.nobody')}</span> : <UserChip user={event.user} size={24} />),
          },
        ]),
    { id: 'model', header: t('usage.records.model'), from: 'md', cell: (event) => <ModelLine event={event} scope={scope} /> },
    {
      id: 'tokens',
      header: t('usage.records.tokens'),
      from: 'md',
      align: 'end',
      cell: (event) => (
        <span className="tabular-nums" title={event.context_tokens === null ? undefined : t('usage.records.prompt', { tokens: formatNumber(event.context_tokens, locale) })}>
          {formatTokens(event.tokens.total_tokens, locale)}
        </span>
      ),
    },
    {
      id: 'credits',
      header: t('usage.records.credits'),
      align: 'end',
      className: 'whitespace-nowrap',
      cell: (event) => (
        <span className="flex items-center justify-end gap-1">
          <EventCredits event={event} />
          <RecordButton event={event} />
        </span>
      ),
    },
  ];

  return (
    <Section id="usage-records" title={t('usage.records.title')} className={className}>
      {depth==='summary' && scope.kind==='node' && <p className="text-sm text-text-muted">{t('usage.records.ownOnly')}</p>}
      {query.isPending ? (
        <SkeletonRows rows={5} />
      ) : query.isError && rows.length === 0 ? (
        <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />
      ) : rows.length === 0 ? (
        <p className="text-sm text-text-muted">{t('usage.records.empty')}</p>
      ) : (
        <>
          <Table label={t('usage.records.title')} columns={columns} rows={rows} rowKey={(event) => event.id} />
          <Pagination hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.isFetchNextPageError ? query.error : undefined} onMore={() => void query.fetchNextPage()} />
        </>
      )}
    </Section>
  );
}

/** The records are not drawn again while the screen asks for other things: their own request, and what they stand for, did not change. */
export const Records = memo(RecordsView);
