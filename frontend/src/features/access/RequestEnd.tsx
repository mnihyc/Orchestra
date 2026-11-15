/** The proposed end of an access request and its shared presentation. */
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { DAY_MS, MINUTE_MS, formatSpan, useNow } from '@/lib/time';
import { Input } from '@/ui/Input';
import { RelativeTime } from '@/ui/RelativeTime';
import { Select } from '@/ui/Select';
import { durationMinutes, MinuteDuration, type MinuteUnit } from './MinuteDuration';

type RequestDuration = 'never' | 'day' | 'week' | 'month' | 'custom' | 'length';
/** The day counts behind the three named choices. */
const DAYS = { day: 1, week: 7, month: 30 } as const;

/** Common request choices; the instant is fixed when the person sends the request. */
export function useRequestEnd() {
  const [duration, setDuration] = useState<RequestDuration>('never');
  const [date, setDate] = useState('');
  const [amount, setAmount] = useState('');
  const [unit, setUnit] = useState<MinuteUnit>('minutes');
  const sent = useRef<{ key: string; end: string | null } | null>(null);
  const now = useNow();
  const time = Math.floor(Date.parse(date) / MINUTE_MS) * MINUTE_MS;
  const minutes = durationMinutes(amount, unit);
  const proposed = duration === 'length' ? now + (minutes ?? NaN) * MINUTE_MS : time;
  const valid = duration === 'length' ? minutes !== null && Number.isFinite(new Date(proposed).getTime())
    : duration !== 'custom' || Number.isFinite(time) && time > now;
  const chooseDuration = (next: RequestDuration) => { sent.current = null; setDuration(next); };
  const chooseDate = (next: string) => { sent.current = null; setDate(next); };
  const end = (attempt = 0): string | null => {
    const key = JSON.stringify([duration, date, amount, unit, attempt]);
    if (sent.current?.key === key && (sent.current.end === null || Date.parse(sent.current.end) > Date.now())) return sent.current.end;
    const target = duration === 'custom' ? time : duration === 'length' ? Date.now() + (minutes ?? NaN) * MINUTE_MS
      : duration === 'never' ? NaN : Date.now() + DAYS[duration] * DAY_MS;
    const instant = duration === 'never' ? null : new Date(Math.floor(target / MINUTE_MS) * MINUTE_MS).toISOString();
    sent.current = { key, end: instant };
    return instant;
  };
  return { duration, setDuration: chooseDuration, date, setDate: chooseDate, amount, setAmount, unit, setUnit, valid, end };
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
      { value: 'length', label: t('access.request.customLength') },
      { value: 'custom', label: t('access.request.chooseDate') },
    ]} />
    {value.duration === 'length' && <MinuteDuration amount={value.amount} unit={value.unit} onAmount={value.setAmount} onUnit={value.setUnit} />}
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
