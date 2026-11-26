/** Amounts of processor, memory and disk in the words a reader uses: processor in cores, memory in binary units, disk in gigabytes, each with the language's grouping.
 */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';

import { useAppearance } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';

const BYTES_PER_MEGABYTE = 1024 * 1024;
const MILLIS_PER_CPU = 1000;
/** A disk is counted in decimal gigabytes, as every report of a storage or a data disk gives it (the driver contract). */
const BYTES_PER_DISK_GIGABYTE = 1_000_000_000;

/** A disk in decimal gigabytes, with the language's grouping: "267 GB". Every place that shows a disk says it so, so that one disk reads the same everywhere. */
export function diskWords(gigabytes: number, t: TFunction, locale: string): string {
  return t('resources.amount.disk', { size: formatNumber(gigabytes, locale) });
}

/** A disk measured in bytes, in the whole decimal gigabytes its storage report gives: reports count whole gigabytes and drop the rest. */
export function diskGigabytes(bytes: number): number {
  return Math.floor(Math.max(0, bytes) / BYTES_PER_DISK_GIGABYTE);
}

export interface AmountWords {
  /** A processor share in thousandths of a processor: "2 cores", "0.5 cores". */
  cpu: (millis: number) => string;
  /** Memory in megabytes: "4 GiB". */
  memory: (megabytes: number) => string;
  /** Disk in gigabytes: "100 GB". */
  disk: (gigabytes: number) => string;
  /** A plain count: processes, units. */
  count: (value: number) => string;
}

export function useAmountWords(): AmountWords {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  return {
    cpu: (millis) => t('resources.amount.cpu', { count: millis / MILLIS_PER_CPU }),
    memory: (megabytes) => formatBytes(megabytes * BYTES_PER_MEGABYTE, locale),
    disk: (gigabytes) => diskWords(gigabytes, t, locale),
    count: (value) => formatNumber(value, locale),
  };
}
