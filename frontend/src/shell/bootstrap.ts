/** One shell read primes the existing caches before any of its descendants can request them. */
import { type QueryClient, useQuery, useQueryClient } from '@tanstack/react-query';

import { adminModeKey } from '@/api/adminMode';
import { meKey, metaKey } from '@/api/auth';
import { outsideQuery } from '@/api/away';
import { chatStatesKey } from '@/api/chats';
import { api, client, ok } from '@/api/client';
import { nodeKey, orgBySlugKey, orgsKey } from '@/api/nodes';
import { favoritesKey, meStateKey } from '@/api/personal';
import type { MeBootstrapReply } from '@/generated/api-types';
import { statesWriter } from '@/realtime/stores/chat';
import { userWriter } from '@/realtime/stores/user';
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
  cache.setQueryData(outsideQuery().queryKey, data.notification_summary);
  for (const org of data.orgs) {
    cache.setQueryData(orgBySlugKey(org.address), org);
    cache.setQueryData(nodeKey(org.id), org);
  }
  statesWriter.loaded(data.chat_states);
  attentionWriter.loadedAll(data.read_states, data.subscriptions);
  // A summary is not an empty, fully loaded Inbox. Preserve its paging state and rows.
  userWriter.apply([], { unread: data.notification_summary.unread, approvalsPending: data.notification_summary.approvals_pending }, data.notification_summary.seq);
  const selected = useSelectedOrg.getState().orgId;
  const remembered = rememberedOrg(data.person.id);
  const saved = data.state.sidebar;
  const remote = typeof saved === 'object' && saved !== null && 'last_org_id' in saved ? saved.last_org_id : null;
  const org = data.orgs.find((org) => org.id === selected) ?? data.orgs.find((org) => org.id === remembered)
    ?? data.orgs.find((org) => org.id === remote) ?? data.orgs.find((org) => org.id === data.person.personal_org_id) ?? data.orgs[0];
  useSelectedOrg.setState({ orgId: org?.id ?? null });
}

export function useShellBootstrap(userId: string | null) {
  const cache = useQueryClient();
  return useQuery({ queryKey: shellBootstrapKey(userId ?? ''), enabled: userId !== null, staleTime: Infinity, retry: false,
    queryFn: async (): Promise<MeBootstrapReply> => {
      const readerVersion = client.session.getState().readerVersion;
      // This one session aggregate belongs to the cache, not a StrictMode mount's abort signal.
      // Removing the session's query still detaches its result; the reader check fences hydration.
      const data = await ok(api.GET('/me/bootstrap'));
      if (data.person.id === userId && client.session.getState().userId === userId && client.session.getState().readerVersion === readerVersion) hydrateShell(cache, data);
      return data;
    },
  });
}
