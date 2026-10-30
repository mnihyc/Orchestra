/** One native observation card with a permitted refresh and optional provider-owned reset control. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { providerQuery, providerSuggestionsQuery, refreshProvider } from '@/api/resources';
import type { PlanProvider } from './plan';
import { ProviderUsageCard } from './ProviderUsageCard';
import { ProviderReset } from './Reset';

export function PlanPanel({ provider, resets = false, onHistory }: { provider: PlanProvider; resets?: boolean; onHistory?: () => void }) {
  const client = useQueryClient();
  const refresh = useMutation({ mutationFn: () => refreshProvider(provider.id), onSuccess: (fresh) => {
    client.setQueryData(providerQuery(provider.id).queryKey, fresh);
    if (fresh.facts.models !== null) client.setQueryData(providerSuggestionsQuery(provider.id).queryKey, fresh.facts.models);
  } });
  return <ProviderUsageCard provider={provider} refreshing={refresh.isPending} error={refresh.error} onRefresh={() => refresh.mutate()} actions={resets ? <ProviderReset provider={provider} onHistory={onHistory} /> : undefined} />;
}

