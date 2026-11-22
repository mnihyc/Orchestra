import { useTranslation } from 'react-i18next';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatDiskBytes, formatNumber } from '@/lib/format';
import { formatSpan } from '@/lib/time';

/** Facts arrive as JSON on machine rows and as the same four permitted figures on a placement. */
export function MachineCapacityLine({ facts }: { facts: unknown }) {
  const { t } = useTranslation();
  const context = useFormatContext();
  if (typeof facts !== 'object' || facts === null) return null;
  const value = facts as Record<string, unknown>;
  const positive = (n: unknown): n is number => typeof n === 'number' && Number.isFinite(n) && n > 0;
  const parts: string[] = [];
  if (positive(value.cpus)) parts.push(t('machines.capacity.cpus', { number: formatNumber(value.cpus, context.locale) }));
  if (positive(value.memory_bytes)) parts.push(t('machines.capacity.ram', { size: formatBytes(value.memory_bytes, context.locale) }));
  // The disk in the decimal gigabytes its storages are reported in, so that it reads as on the Storage tab.
  if (positive(value.data_volume_bytes)) parts.push(t('machines.capacity.disk', { size: formatDiskBytes(value.data_volume_bytes, context.locale) }));
  if (parts.length === 0) return null;
  if (positive(value.capacity_refresh_secs)) parts.push(t('machines.capacity.refresh', { interval: formatSpan(value.capacity_refresh_secs * 1000, context) }));
  return <span className="text-sm text-text-muted">{parts.join(' · ')}</span>;
}
