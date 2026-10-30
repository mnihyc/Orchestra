/** Import summaries name only measured, nonzero actions. */
import type { TFunction } from 'i18next';
import type { ImportActionCounts } from '@/generated/api-types';
import { formatNumber } from '@/lib/format';

export function importActionSummary(actions: ImportActionCounts | null, t: TFunction, locale: string): string {
  if (actions === null) return '–';
  const parts = [
    { value: actions.create, key: 'createdCount' }, { value: actions.replace, key: 'replacedCount' },
    { value: actions.keep_both, key: 'keptCount' }, { value: actions.skip, key: 'skippedCount' },
  ] as const;
  return parts.filter((part) => part.value > 0).map((part) => t(`importWizard.report.${part.key}`, { number: formatNumber(part.value, locale) })).join(' · ')
    || t('importWizard.report.noChanges');
}
