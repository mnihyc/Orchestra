/** Preview and retained occurrence facts share M1 without borrowing a later schedule definition. */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import type { InputTimingPolicy } from '@/generated/api-types';
import { FactDisclosure, type DisclosureFact } from '@/ui/FactDisclosure';

export function timingFacts(policy: InputTimingPolicy | null | undefined, zone: string | null | undefined, t: TFunction): DisclosureFact[] {
  const current = policy ?? null;
  return [
    { key: 'zone', label: t('scheduleTimingFacts.zone'), value: zone },
    { key: 'clock', label: t('scheduleTimingFacts.clock'), value: current === null ? undefined : t(`scheduleTimingFacts.${current.clock}`) },
    { key: 'fold', label: t('scheduleTimingFacts.repeated'), value: current === null ? undefined : t(`scheduleTimingFacts.${current.repeated_time}`) },
    { key: 'gap', label: t('scheduleTimingFacts.missing'), value: current === null ? undefined : t(`scheduleTimingFacts.${current.missing_time}`) },
    { key: 'overdue', label: t('scheduleTimingFacts.overdue'), value: current === null ? undefined : t(`scheduleTimingFacts.${current.overdue}`) },
    { key: 'paused', label: t('scheduleTimingFacts.paused'), value: current === null ? undefined : t(`scheduleTimingFacts.${current.paused_dates}`) },
  ];
}

export function ScheduleTimingFacts({ policy, zone }: { policy: InputTimingPolicy; zone: string }) {
  const { t } = useTranslation();
  return <FactDisclosure groups={[{ key: 'preview-timing', kind: 'current_setting', facts: timingFacts(policy, zone, t) }]} />;
}
