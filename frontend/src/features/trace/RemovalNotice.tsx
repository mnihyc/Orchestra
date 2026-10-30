/** The resolved retention window before scheduling, and the frozen facts afterwards, with unknown size shown honestly. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { useState } from 'react';
import { environmentSettingsQuery } from '@/api/environments';
import { environmentLifecycle } from '@/features/resources/environments/words';
import { record } from '@/features/resources/UnitCommands';
import type { Environment } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { DAY_MS } from '@/lib/time';
import { ErrorState } from '@/ui/ErrorState';
import { objectHref } from './objects';

export function RemovalNotice({ environment }: { environment: Environment }) {
  const { t } = useTranslation(); const format = useFormatContext();
  const [openedAt] = useState(Date.now);
  const lifecycle = environmentLifecycle(environment);
  const policy = useQuery({ ...environmentSettingsQuery(environment.id), enabled: lifecycle === 'unit', staleTime: 0 });
  const row = policy.data?.find((entry) => entry.key === 'env.removed_keep_days');
  const removal = environment.removal;
  const lifetime = record(environment.config)['env.lifetime'];
  const temporary = environment.temporary !== null && environment.temporary !== undefined || lifetime === 'temporary' || lifetime === 'thread';
  const days = removal?.keep_days ?? (temporary ? 0 : typeof row?.value === 'number' ? row.value : null);
  const bytes = removal?.bytes ?? null; const size = bytes === null ? t('trace.unknown') : formatBytes(bytes, format.locale);
  // This is only the earliest preview; the server freezes the window and returns the actual day when scheduling.
  const earliest = days === null ? null : removal?.purge_at ?? new Date(Math.max(openedAt, environment.remove_at === null ? openedAt : Date.parse(environment.remove_at)) + days * DAY_MS).toISOString();
  const day = earliest === null ? null : new Intl.DateTimeFormat(format.locale, { dateStyle: 'medium', timeZone: format.timeZone }).format(new Date(earliest));
  const locked = row?.locked_by?.node;
  if (lifecycle === 'attach') return <p className="text-base text-text-muted">{t('trace.removalConnected')}</p>;
  if (lifecycle === null) return <p className="text-base text-text-muted">{t('trace.removalKindUnknown')}</p>;
  return <div className="flex flex-col gap-2"><p className="text-base text-text-muted">{t(days === 0 ? 'trace.removalUnitZero' : 'trace.removalUnit')}</p>{days !== 0 && <p className="text-sm text-text-muted">{day === null ? t('trace.removalUnknown') : t('trace.removalKept', { day, size })}</p>}{days !== 0 && day !== null && <p className="text-sm text-text-muted">{t('trace.removalEstimate')}</p>}<p className="text-sm text-text-muted">{t('trace.removalIncludes')}</p>{locked !== undefined && locked !== null && <a href={objectHref(locked.kind, locked.id) ?? undefined} className="text-sm text-accent-text underline">{t('trace.lockedBy', { place: locked.name ?? t('trace.unknown') })}</a>}{policy.isError && <ErrorState compact error={policy.error} onRetry={() => void policy.refetch()} />}</div>;
}
