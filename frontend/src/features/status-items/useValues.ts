/** Visible status values observe the thread and existing resource feeds; clocks never make HTTP reads. */
import { useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import { parametersQuery } from '@/api/threads';
import { providerQuery, hostQuery } from '@/api/resources';
import { environmentQuery } from '@/api/environments';
import { useResourceSample } from '@/features/resource-facts/live';
import { useWorkbench } from '@/features/workbench/store';
import { useFormatContext } from '@/lib/appearance';
import { holds } from '@/lib/permissions';
import { useRunningClock } from '@/lib/time';
import { modelTokenEstimate } from '@/lib/tokenEstimate';
import { useTranslation } from 'react-i18next';
import { runningTurn, useThreadPart } from '@/realtime/stores/thread';
import { STATUS_ITEMS, type StatusContext, type StatusPlace } from './registry';

export function useStatusValues(threadId: string, place: StatusPlace, activeLabel?: string) {
  const { t } = useTranslation(), format = useFormatContext();
  const thread = useThreadPart(threadId, (view) => view.thread);
  const turn = useThreadPart(threadId, runningTurn);
  const arrangement = useWorkbench((state) => state.statusItems);
  const selected = arrangement.filter((entry) => entry.place === place && entry.shown);
  const needsSpeed = selected.some((entry) => entry.id === 'speed'), needsTokens = selected.some((entry) => entry.id === 'tokens');
  const needsText = (needsSpeed || needsTokens) && (turn?.usage?.output_tokens ?? 0) <= 0;
  const outputCounts = useThreadPart(threadId, (view) => needsText && view.outputCounts.turnId === turn?.id ? view.outputCounts.counts : null);
  const providerNeeded = selected.some((entry) => entry.id === 'provider_window' || entry.id === 'provider_running');
  const environmentNeeded = selected.some((entry) => entry.id === 'environment_cpu' || entry.id === 'environment_memory' || entry.id === 'host_load');
  const hostNeeded = selected.some((entry) => entry.id === 'host_load');
  // Opening/Composer owns parameters. Its permitted provider projection is also the status source.
  const parameters = useQuery({ ...parametersQuery(threadId), enabled: false });
  const modelOptions = parameters.data?.model.options;
  const activeModel = turn?.model;
  const tokenEstimate = useMemo(() => modelTokenEstimate(modelOptions?.find((option) => option.model_name === activeModel)), [modelOptions, activeModel]);
  const providerId = thread?.binding?.provider.id ?? '';
  const supplied = parameters.data?.provider?.id === providerId && providerId !== '' ? parameters.data.provider : undefined;
  const provider = useQuery({ ...providerQuery(providerId), ...(!providerNeeded || supplied === undefined ? {} : { initialData: supplied }), enabled: false });
  const environmentId = thread?.environment?.id ?? '';
  const environment = useQuery({ ...environmentQuery(environmentId), enabled: environmentNeeded && environmentId !== '' });
  const mayWatch = environmentNeeded && environment.data !== undefined && holds(environment.data.access, 'terminal.view') && environment.data.status === 'running';
  const sample = useResourceSample(environmentId, mayWatch);
  const hostId = environment.data?.host?.id ?? '';
  const host = useQuery({ ...hostQuery(hostId), enabled: hostNeeded && hostId !== '' });
  const now = useRunningClock(turn !== null && thread?.state === 'running' && selected.some((entry) => entry.id === 'speed' || entry.id === 'elapsed'));
  if (thread === null) return [];
  const context: StatusContext = { ...format, t, now, thread, turn, outputCounts, tokenEstimate, provider: supplied ?? provider.data, environment: environment.data, sample, host: host.data, activeLabel };
  return selected.flatMap((entry) => {
    const definition = STATUS_ITEMS.find((item) => item.id === entry.id);
    if (!definition?.places.includes(place)) return [];
    const value = definition.read(context);
    return value === null ? [] : [{ id: definition.id, title: t(definition.title), value }];
  });
}
