/** Last place, last org or personal org redirect.
 *
 * The address of the application itself leads to the place this person was last in on any device, when
 * it can still be read; else to the org they last had in view, the one team org they belong to when
 * there is exactly one, or their own org. Without a readable home, Shared is the normal entrance.
 */
import { createFileRoute, redirect } from '@tanstack/react-router';

import { nodeQuery } from '@/api/nodes';
import { ApiError, client } from '@/api/client';
import { CancelledError } from '@tanstack/react-query';
import { resolveLandingPlaces } from '@/api/orgOverview';
import { queryClient } from '@/api/query';
import { ensureShellBootstrap } from '@/shell/bootstrap';
import { devicePlace, savedPlace } from '@/features/navigation/lastPlace';
import { landingOrg, savedOrg } from '@/features/onboarding/state';
import type { Node } from '@/generated/api-types';
import { rememberedOrg, selectOrg } from '@/shell/org';
import { SharedScreen } from '@/features/shared/Shared';

export const Route = createFileRoute('/')({
  staticData: { screen: 'page' },
  validateSearch: () => ({}),
  loader: async ({ abortController, preload }) => {
    const href = await landingAddress((node) => {
      if (!preload && !abortController.signal.aborted) selectOrg(client.session.getState().userId, node.org_id);
    });
    if (href !== null) throw redirect({ href, replace: true });
  },
  component: SharedScreen,
});

/** The same authorized entrance resolves inside an independent content tab. */
export async function landingAddress(prepare?: (node: Node) => void): Promise<string | null> {
  const userId = client.session.getState().userId;
  if (userId === null) return null;
  const reader = client.session.getState().readerVersion;
  const { orgs, state, person } = await ensureShellBootstrap(queryClient, userId);
  const place = devicePlace() ?? savedPlace(state);
  const last = savedOrg(state) ?? rememberedOrg(userId);
  let org = landingOrg(orgs, person.personal_org_id, last);
  const known = new Map<string, Node>(orgs.filter((node) => node.access.permissions.includes('org.view')).map((node) => [node.id, node]));
  const wanted = [...new Set([place?.node_id, last].filter((id): id is string => id !== undefined && id !== null && !known.has(id)))];
  const context = org ?? orgs.find((node) => node.access.permissions.includes('org.view'));
  let nodes: Node[];
  if (context !== undefined) nodes = await resolveLandingPlaces(queryClient, context.id, wanted);
  else {
    // Without any readable org context, retain the native node reads. There
    // is no hidden org overview/navigation to preload merely for this check.
    nodes = (await Promise.all(wanted.map(async (id) => {
      try { return await queryClient.query({ ...nodeQuery(id), staleTime: 0 }); }
      catch (error) {
        if (!(error instanceof ApiError) || ![403, 404, 410].includes(error.status)) throw error;
        return null;
      }
    }))).filter((node): node is Node => node !== null);
  }
  if (client.session.getState().userId !== userId || client.session.getState().readerVersion !== reader) throw new CancelledError({ silent: true });
  for (const node of nodes) known.set(node.id, node);
  const finish = (node: Node, route: string) => { prepare?.(node); return route; };
  if (place !== null) {
    const saved = known.get(place.node_id);
    if (saved !== undefined) return finish(saved, place.route);
  }
  if (last !== null) {
    const saved = known.get(last);
    if (saved?.kind === 'org' && saved.access.permissions.includes('org.view')) org = saved;
  }
  return org === undefined ? null : finish(org, `/org/${encodeURIComponent(org.address)}`);
}
