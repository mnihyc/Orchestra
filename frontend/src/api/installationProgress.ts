/** Coarse installation activity served by the web edge while the authenticated API is unavailable. */
import { queryOptions } from '@tanstack/react-query';
import { readInstallationProgress } from '@/api/client';

export interface MigrationObservation {
  step: string;
  migration: number | null;
  description: string | null;
  migrations_done: number | null;
  migrations_total: number;
  rows_done: number | null;
  rows_total: number | null;
  detail: string | null;
  updated_at: string;
  elapsed_seconds: number;
  retry_after_seconds: number;
}

/** The first refresh uses an operator override until a status response supplies its configured interval. */
const configured = Number(import.meta.env.VITE_UPDATE_STATUS_INTERVAL_SECONDS);
const INITIAL_SECONDS = Number.isFinite(configured) && configured > 0 ? configured : 1;
const number = (value: unknown): number | null => typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null;
const string = (value: unknown): string | null => typeof value === 'string' ? value : null;

function observation(value: unknown): MigrationObservation | null {
  if (typeof value !== 'object' || value === null) return null;
  const row = value as Record<string, unknown>;
  const step = string(row.step);
  const updated = string(row.updated_at);
  if (step === null || updated === null || !Number.isFinite(Date.parse(updated))) return null;
  return {
    step, migration: number(row.migration), description: string(row.description),
    migrations_done: number(row.migrations_done), migrations_total: number(row.migrations_total) ?? 0,
    rows_done: number(row.rows_done), rows_total: number(row.rows_total), detail: string(row.detail),
    updated_at: updated, elapsed_seconds: number(row.elapsed_seconds) ?? 0,
    retry_after_seconds: (number(row.retry_after_seconds) ?? INITIAL_SECONDS) || INITIAL_SECONDS,
  };
}

/** Healthy sessions never poll this independently served, public status file. */
export const migrationObservationQuery = (active: boolean) => queryOptions({
  queryKey: ['installation-migration'],
  queryFn: async ({ signal }): Promise<MigrationObservation | null> => {
    const value = await readInstallationProgress(signal);
    return observation(value);
  },
  enabled: active,
  retry: false,
  refetchInterval: active ? (query) => (query.state.data?.retry_after_seconds ?? INITIAL_SECONDS) * 1000 : false,
});
