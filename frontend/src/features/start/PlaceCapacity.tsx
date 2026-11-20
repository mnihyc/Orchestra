import { useTranslation } from 'react-i18next';
import { freeCpuWords } from '@/features/resources/hosts/words';
import type { PlaceHost, PlaceRoom } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { formatExact, formatTimeOfDay } from '@/lib/time';

/** Measured room belongs to its physical source. Running threads are counted by Orch. */
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
  return <span className="min-w-0 whitespace-normal text-xs text-text-muted">
    {values.join(t('startFlow.separator'))}
    {room && <>{t('startFlow.separator')}<time dateTime={room.observed_at} title={formatExact(room.observed_at, format)}>{formatTimeOfDay(room.observed_at, format)}</time></>}
  </span>;
}

export function HostCapacityLine({ host, reason }: { host: PlaceHost; reason?: string | null | undefined }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const disk = host.used.disk_gb;
  return <>
    <PlaceCapacity room={host.room} running={host.running_threads} />
    {host.room && disk !== null && host.capacity.disk_gb > 0 && <span className="text-xs text-text-muted">{t('startFlow.separator')}{t('resourceFacts.diskFree', {
      free: formatNumber(Math.max(0, host.capacity.disk_gb - disk), locale), total: formatNumber(host.capacity.disk_gb, locale),
    })}</span>}
    {reason && <span className="block">{reason}</span>}
  </>;
}
