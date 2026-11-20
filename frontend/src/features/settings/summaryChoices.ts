import { useInfiniteQuery } from '@tanstack/react-query';
import { useId } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { useModelHistory } from '@/features/models/history';
import { everythingQuery } from '@/api/everything';
import type { CapabilityBackendCandidate, EverythingRow, ModelRecent } from '@/generated/api-types';
import { holds } from '@/lib/permissions';

/** Both fields share the same usable provider window for this editing session. */
export function summaryProvidersQuery(reader: string, q = '') {
  return everythingQuery(reader, 'resources', {
    scope: 'mine', kinds: ['provider'], q, owner: null, createdFrom: '', createdTo: '', sort: 'name', direction: 'asc',
    resource: { rows: true, view: 'usable' },
  }, 'provider-default-choices');
}

export function usableProvider(item: EverythingRow) {
  const provider = item.resource_row?.kind === 'provider' ? item.resource_row.value
    : item.resource?.kind === 'provider' ? item.resource.value : undefined;
  if (provider === undefined || item.kind !== 'provider' || item.my_access === 'path_only' || item.name === null
    || item.id !== provider.id || provider.status === 'disabled'
    || !holds(provider.access, 'provider.view') || !holds(provider.access, 'provider.use')) return undefined;
  return provider;
}

/** Start the two independent reads when someone begins editing the summarizer. */
export function useSummaryChoices(editing: boolean) {
  const reader = useStore(client.session, (state) => `${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  const session = useId();
  const window = `${reader}:${session}`;
  const providers = useInfiniteQuery({ ...summaryProvidersQuery(window), enabled: editing, staleTime: Infinity, retry: false });
  const history = useModelHistory(editing);
  const accounts = providers.isSuccess
    ? providers.data.pages.flatMap((page) => page.items.map(usableProvider).filter((row) => row !== undefined)) : [];
  const recent = history.read.isSuccess && history.history !== undefined ? history.history.recent.slice(0, history.history.recent_count) : [];
  return { window, accounts, recent, ready: providers.isSuccess, complete: providers.isSuccess && !providers.hasNextPage };
}

export type SummaryChoices = ReturnType<typeof useSummaryChoices>;

/** History identifies a model. The scoped supplier alone identifies its eligible route and placement. */
export function likelySummary(rows: CapabilityBackendCandidate[], recent: readonly ModelRecent[], provider: string, allowOnly: boolean) {
  const eligible = rows.filter((row) => row.eligible && row.backend.offering?.provider === provider);
  for (const model of recent) {
    if (model.provider_id !== provider) continue;
    const matches = eligible.filter((row) => row.backend.offering?.offering === model.model_name);
    if (matches.length === 1) return { candidate: matches[0]!, recent: true };
  }
  return allowOnly && eligible.length === 1 ? { candidate: eligible[0]!, recent: false } : null;
}
