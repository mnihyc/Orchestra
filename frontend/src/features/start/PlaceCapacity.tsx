import { useTranslation } from 'react-i18next';
import { freeCpuWords, placementStorage } from '@/features/resources/hosts/words';
import type { PlaceHost, PlaceRoom } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';

/** Measured room belongs to its physical source. Running threads are counted by Orch. As a row's facts, the figures alone:
 * the clock time they were read at tells a person choosing nothing (an offline place says since when itself). */
export function PlaceCapacity({ room, running }: { room: PlaceRoom | null | undefined; running: number | null | undefined }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const cpu = room?.cpu_free_millis ?? null;
  const memory = room?.memory_free_bytes ?? null;
  const values = cpu === null && memory === null ? [t('placeRoom.unknown')] : [
    freeCpuWords(cpu, t, format.locale),
    memory === null ? t('placeRoom.memoryUnknown') : t('placeRoom.memory', { amount: formatBytes(memory, format.locale) }),
  ];
  values.push(running === undefined || running === null ? t('placeRoom.threadsUnknown') : t('placeRoom.threads', { count: running, n: formatNumber(running, format.locale) }));
  return <span className="min-w-0 whitespace-normal text-xs text-text-muted">{values.join(t('startFlow.separator'))}</span>;
}

/** A host's room and free disk, and the reason it is passed over where it is, as one line of a picker row. */
export function HostCapacityLine({ host, reason }: { host: PlaceHost; reason?: string | null | undefined }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const storage = placementStorage(host.storages);
  return <>
    {reason && <><span>{reason}</span>{t('resourceFacts.separator')}</>}
    <PlaceCapacity room={host.room} running={host.running_threads} />
    {host.room && storage !== undefined && storage.capacity_gb > 0 && <span className="text-xs text-text-muted">{t('startFlow.separator')}{t('resourceFacts.diskFree', {
      free: formatNumber(Math.max(0, storage.free_gb), locale), total: formatNumber(storage.capacity_gb, locale),
    })}</span>}
  </>;
}
