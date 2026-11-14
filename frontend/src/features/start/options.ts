/** The server's answer to what a thread of a place could be bound to, shared by the dialog and inline starter.
 *
 * The dialog and the inline starter read one answer through this, so that they say the same of the same place. A
 * profile whose harness has never said what it is cannot be placed, and the server lists it as such.
 * The confirmed option set remains while resources change; Retry reads one current placement plan.
 */
import { skipToken, useQuery, type UseQueryResult } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { useStore } from 'zustand';

import { ApiError, client } from '@/api/client';
import { nodeKey } from '@/api/nodes';
import { startOptionsQuery } from '@/api/start';
import type { Node } from '@/generated/api-types';

import { canUseEnvironment, CHOOSE_EXISTING_ENVIRONMENT, type Mode, NEW_ENVIRONMENT, NO_PICKS, type Picks, resolve, type Resolved } from './choices';
import { useLastSetup } from './lastSetup';
import type { CreationAccessContext, StartOptions } from '@/api/start';

export interface Started {
  /** Native loading/retry state for callers using the shared read skeleton. */
  read: UseQueryResult<StartOptions>;
  /** The answer, or `null` before the first one. */
  options: StartOptions | null;
  resolved: Resolved | null;
  /** Retained choices for display after a refusal; never evidence that submission is ready. */
  preview: Resolved | null;
  loading: boolean;
  failed: unknown;
  /** The answer shown is the one for another environment, and the one for this is on its way. */
  refreshing: boolean;
  retry: () => void;
}

interface BindingState {
  implicit: { scope: string; environment: string | null };
  held: { scope: string; options: StartOptions } | null;
  refused: { scope: string; environment: string; error: unknown } | null;
}

/** What a thread made in a topic, or in a topic made in a project, could be bound to, and what the person's picks make of it. */
export function useStartOptions(nodeId: string, picks: Picks, mode: Mode, enabled = true, context?: CreationAccessContext, knownEnvironment?: string | null): Started {
  const reader = useStore(client.session, (state) => `${state.userId}:${state.readerVersion}:${state.status}`);
  const last = useLastSetup();
  const previous = last === null ? NO_PICKS : { ...last.picks, environment: last.place === nodeId ? last.picks.environment : null };
  // Metadata is owned by the addressed page. Observe its flight without issuing a
  // competing plain-node request before that page can use its native opening.
  const nodeRead = useQuery<Node>({ queryKey: nodeKey(nodeId), queryFn: skipToken });
  const node = nodeRead.data;
  const seed = knownEnvironment ?? (node?.kind === 'topic' ? node.default_environment_id : null) ?? previous.environment;
  const scope = JSON.stringify([reader, startOptionsQuery(nodeId, null, context).queryKey]);
  const [committed, setCommitted] = useState<string | null>(null);
  useEffect(() => {
    if (!enabled) return;
    let current = true;
    // Parent query observers subscribe in this same commit. Let those subscriptions
    // establish their metadata flight before an automatic unqualified binding read.
    queueMicrotask(() => { if (current) setCommitted(scope); });
    return () => { current = false; };
  }, [enabled, scope]);
  const seedScope = JSON.stringify([scope, seed]);
  const [binding, setBinding] = useState<BindingState>(() => ({
    implicit: { scope: seedScope, environment: seed }, held: null, refused: null,
  }));
  const { implicit, held, refused } = binding;
  const environment = picks.environment === CHOOSE_EXISTING_ENVIRONMENT ? null : picks.environment
    ?? (implicit.scope === seedScope ? implicit.environment : seed);
  const waitingForNode = picks.environment === null && knownEnvironment === undefined && node === undefined && nodeRead.isFetching;
  const settling = picks.environment === null && knownEnvironment === undefined && node === undefined && committed !== scope;
  const paused = waitingForNode || settling;
  const query = useQuery({ ...startOptionsQuery(nodeId, environment, context, picks.newEnvironment), enabled: enabled && !paused });
  // Preserve the loaded form while its newly chosen environment is being checked.
  // A different place, audience or reader never inherits that display snapshot.
  const options = query.data ?? (held?.scope === scope ? held.options : null);
  const judged = options === null ? null : resolve(options, picks, mode, previous);
  const desired = judged?.fresh ? NEW_ENVIRONMENT : judged?.environment?.environment.id ?? null;
  const nativeDefault = options?.environment_default === 'shared'
    ? options.environments.find((row) => row.is_default && canUseEnvironment(row))?.environment.id : undefined;
  const actual = environment ?? nativeDefault ?? NEW_ENVIRONMENT;
  const automatic = picks.environment === null;
  const rejectedChoice = automatic && refused?.scope === scope && refused.environment === desired ? refused.error : null;
  const redirect = automatic && query.data !== undefined && desired !== null && desired !== actual && rejectedChoice === null;
  // A loaded default or remembered id is a hint, never proof of current Use.
  // If that hint was removed or is no longer permitted, read the ordinary default once.
  const fallback = automatic && environment !== null && query.error instanceof ApiError
    && (query.error.is('environment.none_usable') || [403, 404, 410].includes(query.error.status));
  // Reconcile stable query outcomes before commit, together with their scoped display facts.
  let next = binding;
  if (enabled && !paused && !query.isFetching) {
    if (fallback) {
      next = { ...binding, refused: { scope, environment, error: query.error },
        implicit: { scope: seedScope, environment: null } };
    } else {
      const nextHeld = query.data !== undefined && (held?.scope !== scope || held.options !== query.data)
        ? { scope, options: query.data } : held;
      const nextRefused = query.data !== undefined && refused?.scope === scope && refused.environment === environment ? null : refused;
      const nextImplicit = redirect ? { scope: seedScope, environment: desired } : implicit;
      if (nextHeld !== held || nextRefused !== refused || nextImplicit !== implicit) {
        next = { implicit: nextImplicit, held: nextHeld, refused: nextRefused };
      }
    }
  }
  if (next !== binding) setBinding(next);
  const failed = paused && query.data === undefined ? null : query.isError ? query.error : rejectedChoice;
  return {
    read: query,
    options,
    resolved: failed === null ? judged : null,
    preview: judged,
    loading: (paused || query.isPending) && options === null,
    failed,
    refreshing: waitingForNode || (settling && options === null) || fallback || redirect || (query.isPending && options !== null),
    retry: () => { if (enabled && !paused) {
      setBinding((state) => state.refused === null ? state : { ...state, refused: null });
      void query.refetch();
    } },
  };
}
