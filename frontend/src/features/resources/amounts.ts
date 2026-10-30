/** Amounts of processor, memory and disk in the words a reader uses: processor in cores, memory in binary units, disk in gigabytes, each with the language's grouping.
 */
import { useTranslation } from 'react-i18next';

import { useAppearance } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';

const BYTES_PER_MEGABYTE = 1024 * 1024;
const MILLIS_PER_CPU = 1000;

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
    disk: (gigabytes) => t('resources.amount.disk', { size: formatNumber(gigabytes, locale) }),
    count: (value) => formatNumber(value, locale),
  };
}
