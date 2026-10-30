/** Last place, last org or personal org redirect.
 *
 * The address of the application itself leads to the place this person was last in on any device, when
 * it can still be read; else to the org they last had in view, the one team org they belong to when
 * there is exactly one, or their own org. Without a readable home, Shared is the normal entrance.
 */
import { createFileRoute, redirect } from '@tanstack/react-router';

import { meKey } from '@/api/auth';
import { nodeQuery, orgsQuery } from '@/api/nodes';
import { ApiError } from '@/api/client';
import { meStateQuery } from '@/api/personal';
import { queryClient } from '@/api/query';
import { devicePlace, savedPlace } from '@/features/navigation/lastPlace';
import { landingOrg, savedOrg } from '@/features/onboarding/state';
import type { Me } from '@/generated/api-types';
import { rememberedOrg } from '@/shell/org';
import { SharedScreen } from '@/features/shared/Shared';

export const Route = createFileRoute('/')({
  staticData: { screen: 'page' },
  validateSearch: () => ({}),
  loader: async () => { const href = await landingAddress(); if (href !== null) throw redirect({ href, replace: true }); },
  component: SharedScreen,
});

/** The same authorized entrance resolves inside an independent content tab. */
export async function landingAddress(): Promise<string | null> {
  const [orgs, state] = await Promise.all([
    queryClient.query({ ...orgsQuery, staleTime: 0 }),
    queryClient.query({ ...meStateQuery, staleTime: 0 }),
  ]);
  // The place the person was last in, on any device, when it can still be read; nothing of one that cannot is shown.
  const place = devicePlace() ?? savedPlace(state);
  if (place !== null) {
    let readable = false;
    try {
      await queryClient.query({ ...nodeQuery(place.node_id), staleTime: 0 });
      readable = true;
    } catch (error) {
      if (!(error instanceof ApiError) || ![403, 404, 410].includes(error.status)) throw error;
    }
    if (readable) return place.route;
  }
  const me = queryClient.getQueryData<Me>(meKey);
  const last = savedOrg(state) ?? (me === undefined ? null : rememberedOrg(me.id));
  let org = landingOrg(orgs, me?.personal_org_id, last);
  if (last !== null && !orgs.some((candidate) => candidate.id === last)) {
    try {
      const saved = await queryClient.query({ ...nodeQuery(last), staleTime: 0 });
      if (saved.kind === 'org' && saved.access.permissions.includes('org.view')) org = saved;
    } catch (error) {
      if (!(error instanceof ApiError) || ![403, 404, 410].includes(error.status)) throw error;
    }
  }
  if (org !== undefined) return `/org/${encodeURIComponent(org.address)}`;
  return null;
}
