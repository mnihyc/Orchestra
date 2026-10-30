/** The proposed end of an access request and its shared presentation. */
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { DAY_MS, formatSpan, useNow } from '@/lib/time';
import { Input } from '@/ui/Input';
import { RelativeTime } from '@/ui/RelativeTime';
import { Select } from '@/ui/Select';

type RequestDuration = 'never' | 'day' | 'week' | 'month' | 'custom';
/** The day counts behind the three named choices. */
const DAYS = { day: 1, week: 7, month: 30 } as const;

/** Common request choices; the instant is fixed when the person sends the request. */
export function useRequestEnd() {
  const [duration, setDuration] = useState<RequestDuration>('never');
  const [date, setDate] = useState('');
  const sent = useRef<{ duration: RequestDuration; date: string; attempt: number; end: string | null } | null>(null);
  const now = useNow();
  const time = Date.parse(date);
  const valid = duration !== 'custom' || Number.isFinite(time) && time > now;
  const chooseDuration = (next: RequestDuration) => { sent.current = null; setDuration(next); };
  const chooseDate = (next: string) => { sent.current = null; setDate(next); };
  const end = (attempt = 0): string | null => {
    if (sent.current?.duration === duration && sent.current.date === date && sent.current.attempt === attempt && (sent.current.end === null || Date.parse(sent.current.end) > Date.now())) return sent.current.end;
    const instant = duration === 'never' ? null : new Date(duration === 'custom' ? time : Date.now() + DAYS[duration] * DAY_MS).toISOString();
    sent.current = { duration, date, attempt, end: instant };
    return instant;
  };
  return { duration, setDuration: chooseDuration, date, setDate: chooseDate, valid, end };
}

export function RequestEndField({ value }: { value: ReturnType<typeof useRequestEnd> }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  return <div className="flex flex-col gap-2">
    <Select label={t('access.request.duration')} value={value.duration} onValueChange={value.setDuration} fieldClassName="max-w-(--field-wide)" options={[
      { value: 'never', label: t('access.request.untilRemoved') },
      { value: 'day', label: formatSpan(DAY_MS, format, 'long') },
      { value: 'week', label: formatNumber(1, format.locale, { style: 'unit', unit: 'week', unitDisplay: 'long' }) },
      { value: 'month', label: formatSpan(DAYS.month * DAY_MS, format, 'long') },
      { value: 'custom', label: t('access.request.chooseDate') },
    ]} />
    {value.duration === 'custom' && <Input type="datetime-local" label={t('access.request.endsOn')} value={value.date} onChange={(event) => value.setDate(event.target.value)} error={value.date !== '' && !value.valid ? t('access.request.futureDate') : undefined} />}
  </div>;
}

/** The proposed end, shown even after the request has been decided. */
export function RequestedEnd({ at }: { at: string | null | undefined }) {
  const { t } = useTranslation();
  return <span data-requested-end className="pointer-events-auto relative z-10 inline-flex flex-wrap items-center gap-1 text-sm text-text-muted">
    {at === null || at === undefined ? t('access.request.requestedUntilRemoved') : <>{t('access.request.requestedUntil')} <RelativeTime at={at} /></>}
  </span>;
}
