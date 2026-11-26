import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { formatSpan } from '@/lib/time';

/** The machine's size in words, from the same permitted figures; nothing where none is reported. How often they are read
 * again (`refresh`) is said on the machine's own page, under its capacity; a picker's row has no use for it. */
export function machineCapacityWords(facts: unknown, t: TFunction, context: ReturnType<typeof useFormatContext>, refresh = false): string | null {
  if (typeof facts !== 'object' || facts === null) return null;
  const value = facts as Record<string, unknown>;
  const positive = (n: unknown): n is number => typeof n === 'number' && Number.isFinite(n) && n > 0;
  const parts: string[] = [];
  if (positive(value.cpus)) parts.push(t('machines.capacity.cpus', { number: formatNumber(value.cpus, context.locale) }));
  if (positive(value.memory_bytes)) parts.push(t('machines.capacity.ram', { size: formatBytes(value.memory_bytes, context.locale) }));
  if (positive(value.data_volume_bytes)) parts.push(t('machines.capacity.disk', { size: formatBytes(value.data_volume_bytes, context.locale) }));
  if (parts.length === 0) return null;
  if (refresh && positive(value.capacity_refresh_secs)) parts.push(t('machines.capacity.refresh', { interval: formatSpan(value.capacity_refresh_secs * 1000, context) }));
  return parts.join(' · ');
}

/** Facts arrive as JSON on machine rows and as the same four permitted figures on a placement. */
export function MachineCapacityLine({ facts }: { facts: unknown }) {
  const { t } = useTranslation();
  const context = useFormatContext();
  const words = machineCapacityWords(facts, t, context, true);
  return words === null ? null : <span className="text-sm text-text-muted">{words}</span>;
}
