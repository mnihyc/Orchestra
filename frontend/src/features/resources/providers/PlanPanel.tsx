/** One native observation card with a permitted refresh and optional provider-owned reset control. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { providerQuery, refreshProvider } from '@/api/resources';
import type { PlanProvider } from './plan';
import { ProviderUsageCard } from './ProviderUsageCard';
import { ProviderReset } from './Reset';

export function useProviderRefresh(provider:PlanProvider) {
  const client = useQueryClient();
  return useMutation({ mutationFn: () => refreshProvider(provider.id), onSuccess: (fresh) => {
    client.setQueryData(providerQuery(provider.id).queryKey, fresh);
  } });
}

export function PlanPanel({ provider, resets = false, onHistory }: { provider: PlanProvider; resets?: boolean; onHistory?: () => void }) {
  const refresh=useProviderRefresh(provider);
  return <ProviderUsageCard provider={provider} refreshing={refresh.isPending} error={refresh.error} onRefresh={() => refresh.mutate()} actions={resets ? <ProviderReset provider={provider} onHistory={onHistory} /> : undefined} />;
}

