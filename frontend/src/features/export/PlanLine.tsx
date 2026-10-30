/** The line under the options: how much the archive would hold against what the place allows, how long it is kept, and what is left out; past a limit, by how much, with Choose less.
 *
 * It is asked again as the ticks and the options change, so it says first that it is counting, and keeps the
 * last figures greyed meanwhile. A plan that cannot be asked for says why in words: nothing is ticked, the place
 * allows no export, or the archive is over a limit by so much.
 */
import { LoaderCircle, TriangleAlert } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { useAppearance } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { Hint } from '@/features/help/Hint';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';

import type { PlanReading } from './plan';

export interface PlanLineProps {
  /** The last plan the server made; `null` before the first. */
  reading: PlanReading | null;
  /** The ticks or the options changed since it was made, or it is being asked for. */
  counting: boolean;
  /** What asking for it failed with. */
  error: unknown;
  onRetry: () => void;
  /** Why no plan is asked for, when nothing can be: nothing is ticked, or no kind of thing is wanted. */
  idle: 'nothing-ticked' | 'no-kind' | 'span' | 'too-many-ticks' | null;
}

export function PlanLine({ reading, counting, error, onRetry, idle }: PlanLineProps) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const size = (bytes: number) => formatBytes(bytes, locale);
  const many = (count: number) => formatNumber(count, locale);

  let main: ReactNode;
  let tone: 'plain' | 'danger' = 'plain';
  if (idle !== null) {
    tone = 'danger';
    main = t(`export.plan.idle.${idle}`);
  } else if (error !== null && error !== undefined) {
    return (
      <div className="flex flex-col gap-2">
        <ErrorAlert error={error} message={t('export.plan.failed')} />
        <div>
          <Button size="sm" onClick={onRetry}>
            {t('common.tryAgain')}
          </Button>
        </div>
      </div>
    );
  } else if (reading === null) {
    main = t('export.plan.counting');
  } else if (reading.state === 'none-allowed') {
    tone = 'danger';
    main = t('export.plan.noneAllowed');
  } else if (reading.state === 'nothing') {
    tone = 'danger';
    main = t('export.plan.nothing');
  } else if (reading.state === 'too-large') {
    tone = 'danger';
    main = (
      <>
        {reading.overBytes > 0 && <span className="block">{t('export.plan.overBytes', { size: size(reading.bytes), over: size(reading.overBytes), max: size(reading.maxBytes) })}</span>}
        {reading.overItems > 0 && <span className="block">{t('export.plan.overItems', { count: reading.items, formatted: many(reading.items), over: many(reading.overItems), max: many(reading.maxItems) })}</span>}
        <span className="block font-medium">{t('export.plan.chooseLess')}</span>
      </>
    );
  } else {
    main = (
      <span className="inline-flex flex-wrap items-center gap-x-1">
        <span>{t('export.plan.size', { size: size(reading.bytes), max: size(reading.maxBytes) })}</span>
        <span aria-hidden>·</span>
        <span>{t('export.plan.things', { count: reading.items, formatted: many(reading.items), max: many(reading.maxItems) })}</span>
        <span aria-hidden>·</span>
        <span className="inline-flex items-center gap-1">
          {t('export.plan.kept', { count: reading.keepHours, formatted: many(reading.keepHours) })}
          <Hint id="export-kept" />
        </span>
      </span>
    );
  }

  const left = reading !== null && idle === null ? reading : null;
  return (
    <div className={cx('flex flex-col gap-1 text-sm', counting && 'opacity-70')} aria-busy={counting}>
      {/* The sentence that decides whether the button works is read out when it changes. */}
      <p role="status" aria-live="polite" className={cx('flex items-start gap-2', tone === 'danger' ? 'text-danger-text' : 'text-text')}>
        {counting ? <LoaderCircle size={16} className="mt-0.5 shrink-0 animate-spin text-text-muted" aria-hidden /> : tone === 'danger' ? <TriangleAlert size={16} className="mt-0.5 shrink-0" aria-hidden /> : null}
        <span className="min-w-0">
          {counting && reading !== null && <span className="sr-only">{t('export.plan.counting')} </span>}
          {main}
        </span>
      </p>
      {left !== null && left.notExportable > 0 && (
        <p className="flex items-center gap-1 text-text-muted">
          {t('export.plan.notExportable', { count: left.notExportable, formatted: many(left.notExportable) })}
          <Hint id="export-left-out" />
        </p>
      )}
      {left !== null && left.tooLarge > 0 && <p className="text-text-muted">{t('export.plan.tooLarge', { count: left.tooLarge, formatted: many(left.tooLarge) })}</p>}
    </div>
  );
}
