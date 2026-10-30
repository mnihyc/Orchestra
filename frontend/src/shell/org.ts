/** The root org whose navigation is in view, which is also where a person's next visit begins.
 *
 * The left bar shows one root org at a time. Which one is in view follows the screen: an org's own
 * screens select it, and so does anything that says which org it lies in. The device remembers the
 * last one for each person, so that opening the application leads back to where they were.
 */
import { create } from 'zustand';

import { meStateQuery, setMeState } from '@/api/personal';
import { orgBySlugKey } from '@/api/nodes';
import { queryClient } from '@/api/query';
import { record } from '@/features/onboarding/state';
import type { Me, Org } from '@/generated/api-types';

/** Administrative reach does not make somebody else's space a person's own navigation. */
export function railOrgs(orgs: readonly Org[], person: Pick<Me, 'personal_org_id'> | undefined): Org[] {
  return orgs.filter((org) => org.org_kind === 'personal' ? org.id === person?.personal_org_id : org.access.participating);
}

const storageKey = (userId: string) => `orch.${userId}.org`;

interface SelectedOrg {
  /** The root org in view, or `null` while none was chosen. */
  orgId: string | null;
}

export const useSelectedOrg = create<SelectedOrg>(() => ({ orgId: null }));

/** The root org this device remembers for a person. */
export function rememberedOrg(userId: string): string | null {
  try {
    return localStorage.getItem(storageKey(userId));
  } catch {
    return null;
  }
}

/** Brings a root org into view and remembers it for the person signed in. */
export function selectOrg(userId: string | null, orgId: string, explicit = false): void {
  const changed = useSelectedOrg.getState().orgId !== orgId;
  if (changed) useSelectedOrg.setState({ orgId });
  if (userId === null) return;
  if (changed && explicit) void keepPortableOrg(userId, orgId);
  try {
    localStorage.setItem(storageKey(userId), orgId);
  } catch {
    // A device that keeps nothing still shows the org.
  }
}

/** Only the chosen org is written; folded sections and Show choices keep their own values. */
async function keepPortableOrg(userId: string, orgId: string): Promise<void> {
  try {
    const state = queryClient.getQueryData<Record<string, unknown>>(meStateQuery.queryKey) ?? await queryClient.query(meStateQuery);
    // A delayed read from a previous navigation must not replace the current selection.
    if (useSelectedOrg.getState().orgId !== orgId) return;
    // The sidebar may guess while a route is loading; only the resolved screen remembers a visit.
    const pathname = window.location.pathname;
    if (pathname === '/' || ['/login', '/setup', '/register'].includes(pathname)) return;
    const match = /^\/org\/([^/]+)/.exec(pathname);
    let lastOrgId = orgId;
    if (match?.[1] !== undefined) {
      const screen = queryClient.getQueryData<Org>(orgBySlugKey(decodeURIComponent(match[1])));
      if (screen?.org_id !== orgId) return;
      lastOrgId = screen.id;
    }
    const sidebar = { ...record(state.sidebar), last_org_id: lastOrgId };
    if (record(state.sidebar).last_org_id === lastOrgId) return;
    await setMeState('sidebar', sidebar);
    if (useSelectedOrg.getState().orgId === orgId && rememberedOrg(userId) === orgId) {
      queryClient.setQueryData(meStateQuery.queryKey, { ...state, sidebar });
    }
  } catch {
    // The screen remains usable; its next explicit navigation retries portable persistence.
  }
}

/** Nobody is signed in any more: no org is in view. What the device remembers stays for their return. */
export function clearSelectedOrg(): void {
  useSelectedOrg.setState({ orgId: null });
}
