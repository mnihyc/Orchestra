/** One shell read primes the existing caches before any of its descendants can request them. */
import { supersededRead } from '@/api/reads';
import { READ_NOW } from '@/api/freshness';
import { type QueryClient, useQuery, useQueryClient } from '@tanstack/react-query';

import { adminModeKey } from '@/api/adminMode';
import { permissionsKey } from '@/api/access';
import { notificationPrefsKey, notifiersKey } from '@/api/notifications';
import { meKey, metaKey } from '@/api/auth';
import { outsideQuery } from '@/api/away';
import { chatStatesKey } from '@/api/chats';
import { applyChanges } from '@/api/acts';
import { api, client, ok } from '@/api/client';
import { capturePeopleHint, repairPeople } from '@/api/peopleChanges';
import { scheduledReads } from '@/api/readScheduler';
import { ensurePersonalSource, personalSourceJoined } from '@/realtime/live';
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
import { mark } from '@/lib/marks';
import { setSystemName } from '@/lib/title';

export const shellBootstrapKey = (userId: string) => ['shell-bootstrap', userId] as const;

/**
 * How long after the person's own live feed has joined, and the browser has been idle, the first read of the shell is checked
 * once against the server (see `checkAfterJoin`). A default of the build: a later measurement may move it.
 */
export const BASELINE_CHECK_AFTER_MS = 1500;

export function hydrateShell(cache: QueryClient, data: MeBootstrapReply): void {
  // These small session-held records come from the aggregate and native acts/events. Opening a
  // panel later must not age them into independent startup reads; sign-out clears the client.
  for (const key of [meKey, meStateKey, orgsKey, adminModeKey, favoritesKey, chatStatesKey, outsideQuery().queryKey]) {
    cache.setQueryDefaults(key, { staleTime: Infinity, gcTime: Infinity });
  }
  setSystemName(data.meta.name);
  cache.setQueryData(meKey, data.person);
  cache.setQueryData(metaKey, data.meta);
  cache.setQueryData(meStateKey, data.state);
  cache.setQueryData(orgsKey, data.orgs);
  cache.setQueryData(adminModeKey, data.admin_mode);
  cache.setQueryData(favoritesKey, data.favorites);
  cache.setQueryData(chatStatesKey, data.chat_states);
  if (data.permission_catalogue !== undefined && data.permission_catalogue !== null) {
    cache.setQueryDefaults(permissionsKey, { staleTime: Infinity, gcTime: Infinity });
    cache.setQueryData(permissionsKey, data.permission_catalogue);
  }
  if (data.notifier_catalogue !== undefined && data.notifier_catalogue !== null) cache.setQueryData(notifiersKey, data.notifier_catalogue);
  if (data.notification_preferences !== undefined && data.notification_preferences !== null) cache.setQueryData(notificationPrefsKey, data.notification_preferences);
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

/**
 * Whether what is held of the person already says what the later read says: nothing happened that was not told. The members
 * compared are the ones that only the person's own feed reports; chats and the Inbox have cursors of their own and are not.
 */
function matchesHeld(cache: QueryClient, data: MeBootstrapReply): boolean {
  return sameValue(cache.getQueryData(meKey), data.person) && sameValue(cache.getQueryData(meStateKey), data.state)
    && sameValue(cache.getQueryData(orgsKey), data.orgs) && sameValue(cache.getQueryData(adminModeKey), data.admin_mode)
    && sameValue(cache.getQueryData(favoritesKey), data.favorites);
}

function sameValue(one: unknown, other: unknown): boolean {
  if (one === other) return true;
  if (typeof one !== 'object' || typeof other !== 'object' || one === null || other === null) return false;
  if (Array.isArray(one) !== Array.isArray(other)) return false;
  const left = one as Record<string, unknown>, right = other as Record<string, unknown>;
  const keys = Object.keys(left).filter((key) => left[key] !== undefined);
  return keys.length === Object.keys(right).filter((key) => right[key] !== undefined).length && keys.every((key) => sameValue(left[key], right[key]));
}

/** After a retaken or checked baseline: a changed person or mode resets the session; otherwise every view of reach is read again once. */
async function repairAfterBaseline(before: MeBootstrapReply | undefined, data: MeBootstrapReply, cut: number): Promise<void> {
  if (before !== undefined && (before.person.is_system_admin !== data.person.is_system_admin || before.admin_mode.as_admin !== data.admin_mode.as_admin || before.admin_mode.available !== data.admin_mode.available)) {
    await applyChanges([{ session: true, person: data.person, mode: data.admin_mode }]);
    return;
  }
  await repairPeople(queryClient, [meKey, metaKey, meStateKey, orgsKey, adminModeKey, favoritesKey, chatStatesKey,
    ...data.orgs.flatMap((org) => [orgBySlugKey(org.address), nodeKey(org.id)])], cut);
}

/** Waits `ms`, and then for the browser to be idle: a check made in the background never stands in front of what is on screen. */
function afterIdle(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(() => {
      if (typeof requestIdleCallback === 'function') requestIdleCallback(() => resolve(), { timeout: ms });
      else resolve();
    }, ms);
  });
}

/**
 * The person's own feed carries no replay, so a read made beside its join can predate it: a change made in the moments between
 * the server's read and the server's acceptance of the feed was told to nobody. Once the feed has joined, the read is made
 * again once, in the background. It costs one request and changes nothing when nothing was missed; if what the person holds is
 * not what the later read says, the later read is taken, and the reads of reach on screen are read again as after a reconnect.
 * Events that arrive while the check is out are held and applied after it, as for any baseline.
 */
async function checkAfterJoin(cache: QueryClient, userId: string, readerVersion: number | undefined, joining: Promise<void>): Promise<void> {
  await joining;
  // Never accepted (the live connection is down): no cut was missed, and the first join that happens retakes the baseline.
  if (!personalSourceJoined()) return;
  await afterIdle(BASELINE_CHECK_AFTER_MS);
  const same = () => client.session.getState().userId === userId && client.session.getState().readerVersion === readerVersion;
  if (!same()) return;
  const release = holdPersonalEvents();
  let taken: { before: MeBootstrapReply | undefined; later: MeBootstrapReply; cut: number } | null = null;
  try {
    const later = await ok(api.GET('/me/bootstrap'));
    if (same() && later.person.id === userId && !matchesHeld(cache, later)) {
      const cut = capturePeopleHint(cache);
      const before = cache.getQueryData<MeBootstrapReply>(shellBootstrapKey(userId));
      hydrateShell(cache, later);
      cache.setQueryData(shellBootstrapKey(userId), later);
      taken = { before, later, cut };
    }
  } catch {
    // A check that could not be made leaves what was read; the next reconnect retakes the baseline anyway.
  } finally { release(); }
  if (taken !== null) await repairAfterBaseline(taken.before, taken.later, taken.cut);
}

/**
 * The first read of a page: made at once, beside the join of the person's own feed and not after it. The feed joins now; the
 * read does not wait for it. Events that arrive while the read is out are held and applied after it is published.
 */
async function readBesideJoin(cache: QueryClient, userId: string, readerVersion: number | undefined): Promise<MeBootstrapReply> {
  const joining = ensurePersonalSource(true).catch(() => undefined);
  const release = holdPersonalEvents();
  let data: MeBootstrapReply;
  try {
    data = await ok(api.GET('/me/bootstrap'));
    if (data.person.id !== userId || client.session.getState().userId !== userId || client.session.getState().readerVersion !== readerVersion) throw supersededRead();
    hydrateShell(cache, data);
    mark('start/shell-answered');
  } finally { release(); }
  void checkAfterJoin(cache, userId, readerVersion, joining);
  return data;
}

/** Imperative loaders and React observers share this same admitted read owner. */
export function shellBootstrapQuery(cache: QueryClient, userId: string) {
  return { queryKey: shellBootstrapKey(userId), staleTime: Infinity, retry: false as const, retryOnMount: false,
    queryFn: async (): Promise<MeBootstrapReply> => {
      const readerVersion = client.session.getState().readerVersion;
      // Before the feed has ever joined on this page the read is made beside the join; a retake after a reconnect follows it.
      if (!personalSourceJoined()) return readBesideJoin(cache, userId, readerVersion);
      for (;;) {
        await ensurePersonalSource(true);
        const data = await scheduledReads.run(shellBootstrapKey(userId), async (current) => {
          const release = holdPersonalEvents();
          try {
            const data = await ok(api.GET('/me/bootstrap'));
            if (data.person.id !== userId || client.session.getState().userId !== userId || client.session.getState().readerVersion !== readerVersion) throw supersededRead();
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
  const data = await queryClient.query({ ...shellBootstrapQuery(queryClient, userId), ...READ_NOW });
  await repairAfterBaseline(before, data, cut);
});

export function useShellBootstrap(userId: string | null) {
  const cache = useQueryClient();
  return useQuery({ ...shellBootstrapQuery(cache, userId ?? ''), enabled: userId !== null });
}
