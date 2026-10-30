/** Update actions say how many environments and runtimes they affect, with the reader's plurals. */
import type { TFunction } from 'i18next';
import { formatNumber } from '@/lib/format';

export function updateUnits(environments: number, runtimes: number, locale: string, t: TFunction): string {
  const units = [
    ...(environments > 0 || runtimes === 0 ? [t('programVersions.environmentCount', { count: environments, number: formatNumber(environments, locale) })] : []),
    ...(runtimes > 0 ? [t('programVersions.runtimeCount', { count: runtimes, number: formatNumber(runtimes, locale) })] : []),
  ];
  return new Intl.ListFormat(locale, { style: 'long', type: 'conjunction' }).format(units);
}
