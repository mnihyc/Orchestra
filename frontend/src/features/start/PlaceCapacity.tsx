import { useTranslation } from 'react-i18next';
import { diskWords } from '@/features/resources/amounts';
import { freeCpuWords, placementStorage } from '@/features/resources/hosts/words';
import { MEMORY_BYTES_PER_MB } from '@/features/resource-facts/facts';
import type { PlaceHost, PlaceRoom } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';

/** Measured room belongs to its physical source. Running threads are counted by Orch.
 * One line, in the words and units every picker of a host uses: processor free, memory and disk free of what there is,
 * and running threads. The figures alone: the clock time they were read at tells a person choosing nothing (an offline
 * place says since when itself). */
export function PlaceCapacity({ room, running, memoryTotalMb, disk }: { room: PlaceRoom | null | undefined; running: number | null | undefined;
  /** What the host has, where it reports it: the free memory is said of it. */
  memoryTotalMb?: number | undefined;
  /** The disk the volumes go to, already in words ("44 GB free of 267 GB disk"). */
  disk?: string | null | undefined }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const cpu = room?.cpu_free_millis ?? null;
  const memory = room?.memory_free_bytes ?? null;
  const memoryWords = memory === null ? t('placeRoom.memoryUnknown')
    : memoryTotalMb !== undefined && memoryTotalMb > 0
      ? t('resourceFacts.memoryFree', { free: formatBytes(memory, format.locale), total: formatBytes(memoryTotalMb * MEMORY_BYTES_PER_MB, format.locale) })
      : t('placeRoom.memory', { amount: formatBytes(memory, format.locale) });
  const values = cpu === null && memory === null ? [t('placeRoom.unknown')] : [freeCpuWords(cpu, t, format.locale), memoryWords];
  if (disk !== undefined && disk !== null) values.push(disk);
  values.push(running === undefined || running === null ? t('placeRoom.threadsUnknown') : t('placeRoom.threads', { count: running, n: formatNumber(running, format.locale) }));
  return <span className="min-w-0 whitespace-normal text-xs text-text-muted" data-place-capacity>{values.join(t('resourceFacts.separator'))}</span>;
}

/** A host's room and free disk, and the reason it is passed over where it is, as one line of a picker row. */
export function HostCapacityLine({ host, reason }: { host: PlaceHost; reason?: string | null | undefined }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const storage = placementStorage(host.storages);
  const disk = storage === undefined || storage.capacity_gb <= 0 ? null
    : t('resourceFacts.diskFree', { free: diskWords(Math.max(0, storage.free_gb), t, locale), total: diskWords(storage.capacity_gb, t, locale) });
  return <>
    {reason && <><span>{reason}</span>{t('resourceFacts.separator')}</>}
    <PlaceCapacity room={host.room} running={host.running_threads} memoryTotalMb={host.capacity.memory_mb} disk={disk} />
  </>;
}
