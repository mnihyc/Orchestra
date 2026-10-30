/** Installation observations served by the web edge while the API is unavailable for a database update. */
import { useQuery } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { migrationObservationQuery, type MigrationObservation } from '@/api/installationProgress';

import { formatSpan, useRunningClock } from '@/lib/time';
import { formatNumber } from '@/lib/format';
import { useFormatContext } from '@/lib/appearance';
import { useSocketStatus } from '@/realtime/live';

/** One inline migration status for the existing reconnecting banner. */
export function MigrationLine({ observation, elapsed }: { observation: MigrationObservation; elapsed: number }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const parts: string[] = [];
  if (observation.step === 'failed') parts.push(t('migrationProgress.failed'));
  else if (observation.step === 'complete') parts.push(t('migrationProgress.restarting'));
  else {
    parts.push(t('migrationProgress.updating'));
    if (observation.migration !== null) parts.push(t('migrationProgress.migration', {
      version: formatNumber(observation.migration, format.locale, { useGrouping: false, minimumIntegerDigits: 4 }),
      done: formatNumber(Math.min((observation.migrations_done ?? 0) + 1, observation.migrations_total), format.locale),
      total: formatNumber(observation.migrations_total, format.locale),
    }));
    if (observation.rows_done !== null) parts.push(t(observation.rows_total === null ? 'migrationProgress.rows' : 'migrationProgress.rowsTotal', {
      done: formatNumber(observation.rows_done, format.locale),
      total: formatNumber(observation.rows_total ?? 0, format.locale),
    }));
    parts.push(formatSpan(elapsed, format, 'clock'));
  }
  const title = [observation.description, observation.detail].filter((part) => part !== null && part !== '').join(' · ');
  return <span className="tabular-nums" role="status" title={title || undefined} data-migration-progress>{parts.join(' · ')}</span>;
}

/** Only a disconnected session reads the independently served update status. */
export function InstallationProgress({ fallback }: { fallback: ReactNode }) {
  const status = useSocketStatus();
  const active = status.lostAt !== null && status.phase !== 'ready';
  const now = useRunningClock(active);
  const query = useQuery(migrationObservationQuery(active));
  const observation = active && query.data !== undefined && query.data !== null && Date.parse(query.data.updated_at) >= (status.lostAt ?? now) ? query.data : null;
  if (observation === null) return fallback;
  const elapsed = observation.elapsed_seconds * 1000 + Math.max(0, now - Date.parse(observation.updated_at));
  return <MigrationLine observation={observation} elapsed={elapsed} />;
}
