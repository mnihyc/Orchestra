/** What fills a thread's context: a bar of its parts, then free space and the reserve kept for shortening, the same as a table, and when the conversation was last shortened.
 *
 * It is what one click on the context ring opens. The parts are the harness's own breakdown where it gives
 * one, marked counted or estimated by it, or Orch's estimate of the parts it sent and saw
 * where it gives none, with the rest named the harness's own; Orch's share of a part the harness
 * counts as its own is a row of that part. Opening the panel on an idle thread whose harness counts on
 * request has it counted once for the turn. Nothing is shown that nobody reported.
 */
import { useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { ChevronRight } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { threadContextQuery } from '@/api/context';
import { programNameIn } from '@/features/resources/programNames';
import { useProviderHealth } from '@/features/usage/PerformancePlaces';
import {
  countKey,
  type FillKey,
  fillSegments,
  freedBy,
  ringOf,
  type Segment,
} from '@/features/usage/context/fill';
import { PartsBar, Swatch } from '@/features/usage/context/PartsBar';
import type { ContextFill, ContextPart, Thread } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatPercent, formatTokens } from '@/lib/format';
import { turnsRevision, useThreadPart } from '@/realtime/stores/thread';
import { ErrorAlert } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';

export { turnsRevision } from '@/realtime/stores/thread';

/** The words of a part of the bar: a part of the context, free space, or the reserve. */
export function partName(key: FillKey, harness: string, t: TFunction): string {
  return t(`contextUsage.parts.${key}`, { harness });
}

function PartRows({ part, harness }: { part: ContextPart; harness: string }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [open, setOpen] = useState(false);
  if (part.rows.length === 0) return null;
  return (
    <>
      <tr>
        <td colSpan={4} className="pb-1 pl-6">
          <button type="button" aria-expanded={open} onClick={() => setOpen((on) => !on)} className="inline-flex items-center gap-1 rounded-md text-xs text-accent-text hover:underline">
            <ChevronRight aria-hidden className={open ? 'size-3 rotate-90' : 'size-3'} />
            {t(open ? 'contextUsage.panel.hideRows' : 'contextUsage.panel.showRows', { count: part.rows.length })}
          </button>
        </td>
      </tr>
      {open &&
        part.rows.map((row) => (
          <tr key={`${row.kind}:${row.name}`} className="text-xs text-text-muted" data-context-row={row.kind}>
            <td />
            <td className="py-1 pr-2 wrap-anywhere">
              {row.kind === 'orch_share' ? t('contextUsage.rows.orchShare', { harness }) : row.name}
              {row.calls !== null && (
                <span className="ml-3 text-text-subtle">{t('contextUsage.rows.calls', { calls: formatTokens(row.calls, locale), results: formatTokens(Math.max(0, row.tokens - row.calls), locale) })}</span>
              )}
              {row.count !== part.count && <span className="ml-3 text-text-subtle">{t(countKey(row.count), { harness })}</span>}
            </td>
            <td className="py-1 text-right tabular-nums">{formatTokens(row.tokens, locale)}</td>
            <td />
          </tr>
        ))}
    </>
  );
}

/** The bar of a fill, and the same as a table under it. */
export function FillView({ fill, harness }: { fill: ContextFill; harness: string }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const segments = fillSegments(fill);
  const describe = (segment: Segment<FillKey>) =>
    t('contextUsage.bar.part', { name: partName(segment.key, harness, t), tokens: formatTokens(segment.tokens, locale), share: formatPercent(segment.share, locale) });
  return (
    <div className="flex flex-col gap-2" data-context-fill={fill.count}>
      <PartsBar segments={segments} label={t('contextUsage.bar.label')} describe={describe} />
      <table className="w-full text-sm">
        <caption className="sr-only">{t('contextUsage.panel.table')}</caption>
        <thead className="sr-only">
          <tr>
            <th scope="col">{t('contextUsage.panel.look')}</th>
            <th scope="col">{t('contextUsage.panel.part')}</th>
            <th scope="col">{t('contextUsage.panel.tokens')}</th>
            <th scope="col">{t('contextUsage.panel.share')}</th>
          </tr>
        </thead>
        <tbody>
          {segments.map((segment) => {
            const part = fill.parts.find((candidate) => candidate.kind === segment.key);
            return (
              <FillRow key={segment.key} segment={segment} part={part} harness={harness} fill={fill} />
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function FillRow({ segment, part, harness, fill }: { segment: Segment<FillKey>; part: ContextPart | undefined; harness: string; fill: ContextFill }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  return (
    <>
      <tr data-context-part={segment.key}>
        <td className="w-4 py-1 align-middle"><Swatch segment={segment} /></td>
        <td className="py-1 pr-2">
          {partName(segment.key, harness, t)}
          {part !== undefined && part.count !== fill.count && <span className="ml-3 text-xs text-text-subtle">{t(countKey(part.count), { harness })}</span>}
        </td>
        <td className="py-1 text-right tabular-nums">{formatTokens(segment.tokens, locale)}</td>
        <td className="w-[3.5rem] py-1 text-right text-text-muted tabular-nums">{formatPercent(segment.share, locale)}</td>
      </tr>
      {part !== undefined && <PartRows part={part} harness={harness} />}
    </>
  );
}

export function ContextPanel({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const revision = useThreadPart(thread.id, turnsRevision);
  useProviderHealth(thread.binding?.provider.id ?? null);
  const kept = useQuery(threadContextQuery(thread.id, 'kept', revision));
  // A harness that counts on request is asked once a turn, when somebody opens this between turns.
  const counted = useQuery({ ...threadContextQuery(thread.id, 'count', revision), enabled: kept.data?.can_count === true });
  const data = counted.data ?? kept.data;
  const harness = programNameIn(data, locale) ?? programNameIn(thread.binding?.agent, locale) ?? t('contextUsage.harness.other');
  const ring = ringOf(thread.usage);
  const fill = data?.fill ?? null;
  const shortened = data?.shortened ?? null;
  const freed = freedBy(shortened);
  return (
    <div className="flex w-(--container-form) max-w-full flex-col gap-3" data-context-panel>
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h2 className="text-base font-semibold text-text">{t('contextUsage.panel.title')}</h2>
        {fill !== null && <span className="text-xs text-text-subtle" data-context-count={fill.count}>{t(countKey(fill.count), { harness })}</span>}
      </div>
      {ring !== null && (
        <p className="text-sm text-text-muted tabular-nums">
          {t(ring.compactPercent === null ? 'contextUsage.ring.labelBare' : 'contextUsage.ring.label', {
            percent: formatPercent(ring.percent / 100, locale),
            used: formatTokens(ring.used, locale),
            window: formatTokens(ring.window, locale),
            compact: formatPercent((ring.compactPercent ?? 0) / 100, locale),
          })}
        </p>
      )}
      {kept.isError ? (
        <ErrorAlert error={kept.error} />
      ) : data === undefined ? (
        <SkeletonRows rows={3} />
      ) : fill === null ? (
        <p className="text-sm text-text-muted" data-context-absent>
          {t(data.reports.context ? 'contextUsage.absent.noFillYet' : 'contextUsage.absent.noContext', { harness })}
        </p>
      ) : (
        <FillView fill={fill} harness={harness} />
      )}
      {counted.isFetching && <p className="text-xs text-text-subtle">{t('contextUsage.panel.counting', { harness })}</p>}
      {fill !== null && fill.count === 'orch' && (
        <p className="text-xs text-text-subtle">{t(fill.scaled ? 'contextUsage.panel.orchScaled' : 'contextUsage.panel.orchMethod', { harness })}</p>
      )}
      {fill !== null && fill.count === 'estimated' && <p className="text-xs text-text-subtle">{t('contextUsage.panel.harnessEstimated', { harness })}</p>}
      <p className="text-xs text-text-muted" data-context-shortened>
        {shortened === null ? (
          t('contextUsage.panel.neverShortened')
        ) : (
          <>
            {t('contextUsage.panel.shortened')} <RelativeTime at={shortened.at} />
            {freed !== null && <span className="ml-3">{t('contextUsage.panel.freed', { tokens: formatTokens(freed, locale) })}</span>}
          </>
        )}
      </p>
    </div>
  );
}
