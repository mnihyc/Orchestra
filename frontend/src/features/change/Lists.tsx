/** What changes, what stays as it is, and what ends or is removed: three short lists, always in view, so that nobody has to work out what a plan means.
 *
 * Each line is one sentence with a mark that says what kind it is: plain, a warning where something is opened or
 * lost, a danger where it cannot go on. Contextual help has one home in the rail. Under the lists, a
 * table of the history says what is kept, converted and dropped, item kind by item kind.
 */
import { OctagonAlert, TriangleAlert } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { MeasurementSize } from '@/features/change/Measurements';
import type { Line } from '@/features/change/plan';
import type { HistoryReport } from '@/generated/api-types';
import { cx } from '@/ui/cx';
import { ScrollRegion } from '@/ui/ScrollRegion';
import { RuntimeChangeDetails } from '@/features/resources/TechnicalValue';
import { ResourceSentence } from '@/features/resource-facts/ResourceSentence';

function Mark({ tone }: { tone: Line['tone'] }) {
  if (tone === 'warning') return <TriangleAlert className="mt-0.5 shrink-0 text-warning" aria-hidden />;
  if (tone === 'danger') return <OctagonAlert className="mt-0.5 shrink-0 text-danger" aria-hidden />;
  return <span aria-hidden className="mt-2 mr-1 size-1 shrink-0 rounded-full bg-text-subtle" />;
}

function List({ title, lines, id }: { title: string; lines: readonly Line[]; id: string }) {
  const { t } = useTranslation();
  return (
    <section aria-labelledby={`${id}-title`} className="grid gap-1 sm:grid-cols-[9rem_minmax(0,1fr)] sm:gap-4" data-plan-list={id}>
      <h4 id={`${id}-title`} className="text-sm font-semibold text-text">
        {title}
      </h4>
      <ul className="flex flex-col gap-1">
        {lines.map((line) => (
          <li key={line.id} data-tone={line.tone} className="flex items-start gap-2 text-base text-text">
            <Mark tone={line.tone} />
            <div className="min-w-0 wrap-anywhere">
              {line.tone !== 'plain' && <span className="sr-only">{t(`change.plan.tone.${line.tone}`)} </span>}
              <ResourceSentence text={line.text} resources={line.resources} />
              {line.size !== undefined && <>{' · '}<MeasurementSize size={line.size} /></>}
              {line.suffix !== undefined && <span className="text-sm text-text-muted">{' · '}{line.suffix}</span>}
              {line.technical !== undefined && <RuntimeChangeDetails change={line.technical} />}
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}

export interface PlanListsProps {
  changes: readonly Line[];
  stays: readonly Line[];
  ends: readonly Line[];
  /** Recovery on failure is separate from what the successful change removes. */
  ifFails?: readonly Line[];
  /** Words for the headings that differ with what is done ("Ends or is removed" is for a switch). */
  className?: string;
  /** An ordinary switch shows only actual consequences, without empty sections or a plan card. */
  compact?: boolean;
}

export function PlanLists({ changes, stays, ends, ifFails = [], className, compact = false }: PlanListsProps) {
  const { t } = useTranslation();
  return (
    <div className={cx('flex flex-col gap-3', !compact && 'rounded-lg border border-border bg-surface p-4', className)}>
      {(!compact || changes.length > 0) && <List id="changes" title={t('change.lists.changes')} lines={changes} />}
      {(!compact || stays.length > 0) && <List id="stays" title={t('change.lists.stays')} lines={stays} />}
      {(!compact && ifFails.length === 0 || ends.length > 0) && <List id="ends" title={t('change.lists.ends')} lines={ends} />}
      {ifFails.length > 0 && <List id="if-fails" title={t('change.lists.ifFails')} lines={ifFails} />}
    </div>
  );
}

/** What became of the items of each kind: kept, converted, dropped, and why what was dropped was. */
export function HistoryReportTable({ report }: { report: HistoryReport }) {
  const { t } = useTranslation();
  const kinds = Object.entries(report.by_kind);
  if (kinds.length === 0) return null;
  return (
    <details className="text-sm" data-history-report>
      <summary className="cursor-default rounded-md text-text-muted select-none hover:text-text">{t('change.report.summary', { count: report.items_total })}</summary>
      <ScrollRegion kind="table" label={t('change.report.caption')} className="mt-2">
        <table className="w-full min-w-(--field-wide) text-left text-sm">
          <caption className="sr-only">{t('change.report.caption')}</caption>
          <thead>
            <tr className="text-text-muted">
              <th scope="col" className="py-1 pr-3 font-medium">
                {t('change.report.kind')}
              </th>
              <th scope="col" className="px-2 py-1 text-right font-medium">
                {t('change.report.kept')}
              </th>
              <th scope="col" className="px-2 py-1 text-right font-medium">
                {t('change.report.converted')}
              </th>
              <th scope="col" className="py-1 pl-2 text-right font-medium">
                {t('change.report.dropped')}
              </th>
            </tr>
          </thead>
          <tbody>
            {kinds.map(([kind, counts]) => (
              <tr key={kind} className="border-t border-border">
                <th scope="row" className="py-1 pr-3 font-normal text-text">
                  {t(`change.report.kinds.${kind}`, { defaultValue: kind.replaceAll('_', ' ') })}
                </th>
                <td className="px-2 py-1 text-right tabular-nums">{counts.kept}</td>
                <td className="px-2 py-1 text-right tabular-nums">{counts.converted}</td>
                <td className="py-1 pl-2 text-right tabular-nums">{counts.dropped}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </ScrollRegion>
      {report.dropped.length > 0 && (
        <ul className="mt-2 flex flex-col gap-1 text-text-muted">
          {report.dropped.map((drop) => (
            <li key={`${drop.kind}-${drop.reason}`}>{t('change.report.droppedBecause', { count: drop.count, kind: t(`change.report.kinds.${drop.kind}`, { defaultValue: drop.kind.replaceAll('_', ' ') }), reason: drop.reason })}</li>
          ))}
        </ul>
      )}
      {report.warnings.length > 0 && (
        <ul className="mt-2 flex flex-col gap-1 text-text-muted">
          {report.warnings.map((warning) => (
            <li key={warning}>{warning}</li>
          ))}
        </ul>
      )}
    </details>
  );
}
