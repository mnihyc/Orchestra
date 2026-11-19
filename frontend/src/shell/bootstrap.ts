/** One shell read primes the existing caches before any of its descendants can request them. */
import { CancelledError, type QueryClient, useQuery, useQueryClient } from '@tanstack/react-query';

import { adminModeKey } from '@/api/adminMode';
import { meKey, metaKey } from '@/api/auth';
import { outsideQuery } from '@/api/away';
import { chatStatesKey } from '@/api/chats';
import { applyChanges } from '@/api/acts';
import { api, client, ok } from '@/api/client';
import { capturePeopleHint, repairPeople } from '@/api/peopleChanges';
import { scheduledReads } from '@/api/readScheduler';
import { ensurePersonalSource } from '@/realtime/live';
import { holdPersonalEvents, personalBaselineReader } from '@/realtime/personalSource';
import { queryClient } from '@/api/query';
import { nodeKey, orgBySlugKey, orgsKey } from '@/api/nodes';
import { favoritesKey, meStateKey } from '@/api/personal';
import type { MeBootstrapReply } from '@/generated/api-types';
import { statesWriter } from '@/realtime/stores/chat';
import { useUserStore, userWriter } from '@/realtime/stores/user';
import { landingOrg, savedOrg } from '@/features/onboarding/state';
import { rememberedOrg, useSelectedOrg } from '@/shell/org';
import { attentionWriter } from '@/features/navigation/attention';

export const shellBootstrapKey = (userId: string) => ['shell-bootstrap', userId] as const;

export function hydrateShell(cache: QueryClient, data: MeBootstrapReply): void {
  // These small session-held records come from the aggregate and native acts/events. Opening a
  // panel later must not age them into independent startup reads; sign-out clears the client.
  for (const key of [meKey, meStateKey, orgsKey, adminModeKey, favoritesKey, chatStatesKey, outsideQuery().queryKey]) {
    cache.setQueryDefaults(key, { staleTime: Infinity, gcTime: Infinity });
  }
  cache.setQueryData(meKey, data.person);
  cache.setQueryData(metaKey, data.meta);
  cache.setQueryData(meStateKey, data.state);
  cache.setQueryData(orgsKey, data.orgs);
  cache.setQueryData(adminModeKey, data.admin_mode);
  cache.setQueryData(favoritesKey, data.favorites);
  cache.setQueryData(chatStatesKey, data.chat_states);
  const heldSummary = cache.getQueryData(outsideQuery().queryKey);
  cache.setQueryData(outsideQuery().queryKey, heldSummary !== undefined && heldSummary.seq > data.notification_summary.seq ? heldSummary : data.notification_summary);
  for (const org of data.orgs) {
    cache.setQueryData(orgBySlugKey(org.address), org);
    cache.setQueryData(nodeKey(org.id), org);
  }
  statesWriter.loaded(data.chat_states);
  attentionWriter.loadedAll(data.read_states, data.subscriptions);
  // A summary is not an empty, fully loaded Inbox. Preserve its paging state and rows.
  userWriter.apply([], data.notification_summary.seq < useUserStore.getState().lastSeq ? null : { unread: data.notification_summary.unread, approvalsPending: data.notification_summary.approvals_pending }, data.notification_summary.seq);
  const selected = useSelectedOrg.getState().orgId;
  const remembered = rememberedOrg(data.person.id);
  const org = data.orgs.find((org) => org.id === selected && org.access.permissions.includes('org.view'))
    ?? landingOrg(data.orgs, data.person.personal_org_id, savedOrg(data.state) ?? remembered);
  useSelectedOrg.setState({ orgId: org?.id ?? null });
  scheduledReads.supply(shellBootstrapKey(data.person.id), data.orgs.flatMap((org) => [orgBySlugKey(org.address), nodeKey(org.id)]));
}

/** Imperative loaders and React observers share this same admitted read owner. */
export function shellBootstrapQuery(cache: QueryClient, userId: string) {
  return { queryKey: shellBootstrapKey(userId), staleTime: Infinity, retry: false as const, retryOnMount: false,
    queryFn: async (): Promise<MeBootstrapReply> => {
      const readerVersion = client.session.getState().readerVersion;
      for (;;) {
        await ensurePersonalSource(true);
        const data = await scheduledReads.run(shellBootstrapKey(userId), async (current) => {
          const release = holdPersonalEvents();
          try {
            const data = await ok(api.GET('/me/bootstrap'));
            if (data.person.id !== userId || client.session.getState().userId !== userId || client.session.getState().readerVersion !== readerVersion) throw new CancelledError({ silent: true });
            if (!current()) return null;
            hydrateShell(cache, data);
            return data;
          } finally { release(); }
        });
        if (data !== null) return data;
      }
    },
  };
}

export function ensureShellBootstrap(cache: QueryClient, userId: string): Promise<MeBootstrapReply> {
  return cache.query(shellBootstrapQuery(cache, userId));
}

personalBaselineReader(async () => {
  const userId = client.session.getState().userId;
  if (userId === null) return;
  const cut = capturePeopleHint(queryClient);
  const before = queryClient.getQueryData<MeBootstrapReply>(shellBootstrapKey(userId));
  await queryClient.cancelQueries({ queryKey: shellBootstrapKey(userId), exact: true });
  const data = await queryClient.query({ ...shellBootstrapQuery(queryClient, userId), staleTime: 0 });
  if (before !== undefined && (before.person.is_system_admin !== data.person.is_system_admin || before.admin_mode.as_admin !== data.admin_mode.as_admin || before.admin_mode.available !== data.admin_mode.available)) {
    await applyChanges([{ session: true, person: data.person, mode: data.admin_mode }]);
    return;
  }
  await repairPeople(queryClient, [meKey, metaKey, meStateKey, orgsKey, adminModeKey, favoritesKey, chatStatesKey,
    ...data.orgs.flatMap((org) => [orgBySlugKey(org.address), nodeKey(org.id)])], cut);
});

export function useShellBootstrap(userId: string | null) {
  const cache = useQueryClient();
  return useQuery({ ...shellBootstrapQuery(cache, userId ?? ''), enabled: userId !== null });
}
