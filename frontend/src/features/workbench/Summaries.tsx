/** Small section summaries read existing thread and query data; they never fetch a second copy. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { processesQuery } from '@/api/processes';
import { changesQuery } from '@/api/workspace';
import { useRouteThread, useThreadDoor } from '@/features/files/door';
import type { EnvironmentChange } from '@/generated/api-types';
import { useThreadPart, useThreadRow } from '@/realtime/stores/thread';

export function changeTotals(changes: readonly EnvironmentChange[]) {
  return { files: new Set(changes.map((change) => change.path)).size,
    added: changes.reduce((sum, change) => sum + change.added, 0), removed: changes.reduce((sum, change) => sum + change.removed, 0) };
}

export function ChangesSummary() {
  const { t } = useTranslation();
  const state = useThreadDoor(useRouteThread());
  const door = state.kind === 'ready' ? state.access.door : { environmentId: '', threadId: null };
  const query = useQuery({ ...changesQuery(door, false), enabled: false });
  if (state.kind !== 'ready' || !state.access.mayView || !query.data?.length) return null;
  const totals = changeTotals(query.data);
  return <>{t('workbench.summary.changes', { count: totals.files, added: totals.added, removed: totals.removed })}</>;
}

export function PeopleSummary() {
  const { t } = useTranslation();
  const id = useRouteThread();
  const count = useThreadPart(id ?? '', (state) => state.presence.length);
  return id === null || count === 0 ? null : <>{t('workbench.summary.here', { count })}</>;
}

export function ProcessesSummary() {
  const { t } = useTranslation();
  const state = useThreadDoor(useRouteThread());
  const door = state.kind === 'ready' ? state.access.door : { environmentId: '', threadId: null };
  const query = useQuery({ ...processesQuery(door), enabled: false });
  const count = query.data?.processes.filter((process) => process.thread_id === door.threadId).length ?? 0;
  return state.kind !== 'ready' || !state.access.mayWatchProcesses || count === 0 ? null : <>{t('workbench.summary.processes', { count })}</>;
}

export function EnvironmentSummary() {
  const { t } = useTranslation();
  const environment = useThreadRow(useRouteThread() ?? '')?.environment;
  return environment === null || environment === undefined ? null : <>{t(`environments.status.${environment.status}`)}</>;
}
