/** A duration entered in familiar units and sent as whole minutes. */
import { useTranslation } from 'react-i18next';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';

const MINUTES = { minutes: 1, hours: 60, days: 24 * 60 } as const;
export type MinuteUnit = keyof typeof MINUTES;

/** Empty, fractional and unrepresentable lengths cannot become an indefinite grant. */
export function durationMinutes(amount: string, unit: MinuteUnit): number | null {
  if (amount.trim() === '') return null;
  const value = Number(amount) * MINUTES[unit];
  return Number.isSafeInteger(value) && value >= 1 ? value : null;
}

export function MinuteDuration({ amount, unit, onAmount, onUnit, maxMinutes }: {
  amount: string;
  unit: MinuteUnit;
  onAmount: (amount: string) => void;
  onUnit: (unit: MinuteUnit) => void;
  maxMinutes?: number;
}) {
  const { t } = useTranslation();
  const minutes = durationMinutes(amount, unit);
  return <div className="flex min-w-0 flex-wrap items-start gap-2">
    <Input type="number" min={1 / MINUTES[unit]} step={1 / MINUTES[unit]} label={t('access.request.length')} value={amount}
      fieldClassName="min-w-0 flex-1" onChange={(event) => onAmount(event.target.value)}
      error={amount !== '' && (minutes === null || maxMinutes !== undefined && minutes > maxMinutes) ? t('access.request.lengthRule') : undefined} />
    <Select<MinuteUnit> label={t('access.request.lengthUnit')} value={unit} onValueChange={onUnit} fieldClassName="min-w-0 flex-1"
      options={(Object.keys(MINUTES) as MinuteUnit[]).map((value) => ({ value, label: t(`access.request.units.${value}`) }))} />
  </div>;
}
