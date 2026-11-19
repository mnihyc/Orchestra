/** The report's span: rolling days, the previous calendar week, this year, all time or custom dates.
 *
 * One compact shared control marks the chosen range; Custom opens the same date fields as other pages.
 * Days are counted in UTC, which is what the ledger is kept by, and the row says what
 * span it is in words so that the end of "this year" or of a span of one's own is never a guess.
 */
import { CalendarRange } from 'lucide-react';
import { type ReactNode, type SyntheticEvent, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { type Range, RANGES, type Span, today, type UsageSearch } from '@/features/usage/search';
import { useFormatContext } from '@/lib/appearance';
import { Button } from '@/ui/Button';
import { DateField } from '@/ui/DateField';
import { Popover } from '@/ui/Popover';
import { SegmentedControl } from '@/ui/SegmentedControl';

export interface PeriodProps {
  /** Embedded reports keep their view and export controls on this same wrapping row. */
  actions?:ReactNode;
  span: Span;
  onSearch: (patch: { [Member in keyof UsageSearch]?: UsageSearch[Member] | undefined }) => void;
  now: number;
}

/** A day as the reader writes it: the month and the day, with the year when it is not the year of `now`. */
export function dayWords(day: string, locale: string, now: number): string {
  const sameYear = day.slice(0, 4) === today(now).slice(0, 4);
  return new Intl.DateTimeFormat(locale, { timeZone: 'UTC', month: 'short', day: 'numeric', ...(sameYear ? {} : { year: 'numeric' }) }).format(new Date(`${day}T00:00:00Z`));
}

function CustomSpan({ span, onSearch, now, zoneId }: PeriodProps & { zoneId: string }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [open, setOpen] = useState(false);
  const [from, setFrom] = useState(span.from);
  const [to, setTo] = useState(span.to);
  const last = today(now);
  const valid = from !== '' && to !== '' && from <= to && to <= last;
  const custom = span.range === 'custom';

  const apply = (event: SyntheticEvent) => {
    event.preventDefault();
    if (!valid) return;
    onSearch({ from, to });
    setOpen(false);
  };

  return (
    <Popover
      label={t('usage.period.customLabel')}
      open={open}
      onOpenChange={(next) => {
        // The card opens on the span in force.
        if (next) {
          setFrom(span.from);
          setTo(span.to);
        }
        setOpen(next);
      }}
      trigger={
        <Button size="sm" variant={custom ? 'secondary' : 'ghost'} icon={<CalendarRange aria-hidden />} aria-pressed={custom}>
          {custom ? t('usage.period.customSpan', { from: dayWords(span.from, locale, now), to: dayWords(span.to, locale, now) }) : t('usage.period.custom')}
        </Button>
      }
    >
      <form onSubmit={apply} className="flex w-[18rem] max-w-full flex-col gap-3">
        <DateField label={t('usage.period.from')} aria-describedby={zoneId} value={from} max={to === '' ? last : to} onValueChange={setFrom} />
        <DateField
          label={t('usage.period.to')}
          aria-describedby={zoneId}
          value={to}
          min={from}
          max={last}
          onValueChange={setTo}
          error={to > last ? t('usage.period.future') : from > to && from !== '' && to !== '' ? t('usage.period.backwards') : undefined}
        />
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" size="sm" onClick={() => setOpen(false)}>
            {t('common.cancel')}
          </Button>
          <Button type="submit" variant="primary" size="sm" disabled={!valid}>
            {t('usage.period.apply')}
          </Button>
        </div>
      </form>
    </Popover>
  );
}

export function Period({ span, onSearch, now,actions }: PeriodProps) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const zoneId = useId();
  const pick = (range: Range) => onSearch({ range });
  return (
    <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-2">
      <SegmentedControl
        label={t('usage.period.label')}
        size="sm"
        // A custom span marks its own button, leaving the named ranges unselected.
        value={span.range === 'custom' ? ('' as Range) : span.range}
        onValueChange={pick}
        segments={RANGES.map((range) => ({ value: range, name: t(`usagePerf.periodNames.${range}`) }))}
      />
      <CustomSpan span={span} onSearch={onSearch} now={now} zoneId={zoneId} />
      {span.range !== 'all' && span.range !== 'custom' && <span className="text-sm text-text-muted">{t('usage.period.words', { from: dayWords(span.from, locale, now), to: dayWords(span.to, locale, now) })}</span>}
      {span.range==='all' && <span className="text-sm text-text-muted">{t('usage.period.utcShort')}</span>}
      <span id={zoneId} className="sr-only">{t('usage.period.utc')}</span>
      {actions!==undefined && <div className="ml-auto flex max-w-full flex-wrap items-center gap-2">{actions}</div>}
    </div>
  );
}
