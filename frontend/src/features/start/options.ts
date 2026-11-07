/** The server's answer to what a thread of a place could be bound to, shared by the dialog and inline starter.
 *
 * The dialog and the inline starter read one answer through this, so that they say the same of the same place. A
 * profile whose harness has never said what it is cannot be placed, and the server lists it as such.
 * The confirmed option set remains while resources change; Retry reads one current placement plan.
 */
import { keepPreviousData, useQuery } from '@tanstack/react-query';

import { startOptionsQuery } from '@/api/start';

import { canUseEnvironment, CHOOSE_EXISTING_ENVIRONMENT, noHostEnvironment, type Mode, NO_PICKS, type Picks, resolve, type Resolved } from './choices';
import { useLastSetup } from './lastSetup';
import type { CreationAccessContext, StartOptions } from '@/api/start';

export interface Started {
  /** The answer, or `null` before the first one. */
  options: StartOptions | null;
  resolved: Resolved | null;
  loading: boolean;
  failed: unknown;
  /** The answer shown is the one for another environment, and the one for this is on its way. */
  refreshing: boolean;
  retry: () => void;
}

/** What a thread made in a topic, or in a topic made in a project, could be bound to, and what the person's picks make of it. */
export function useStartOptions(nodeId: string, picks: Picks, mode: Mode, enabled = true, context?: CreationAccessContext): Started {
  const last = useLastSetup();
  const environmentId = picks.environment === CHOOSE_EXISTING_ENVIRONMENT ? null : picks.environment;
  const base = useQuery({ ...startOptionsQuery(nodeId, environmentId, context), enabled, placeholderData: keepPreviousData });
  // A place's environment rule wins. Reuse a location only in that same place, while it is still usable.
  const rememberedEnvironment = !base.isError && !base.isPlaceholderData && environmentId === null && picks.environment === null && last?.place === nodeId && base.data?.environment_default === 'shared' && !base.data.environments.some((row) => row.is_default && canUseEnvironment(row))
    ? base.data.environments.find((row) => canUseEnvironment(row) && row.environment.id === last.picks.environment)?.environment.id ?? null : null;
  const hostFallback = !base.isError && !base.isPlaceholderData && picks.environment === null && base.data !== undefined
    && !(base.data.environment_default === 'shared' && base.data.environments.some((row) => row.is_default && canUseEnvironment(row)))
    ? noHostEnvironment(base.data)?.environment.id ?? null : null;
  const redirectedEnvironment = rememberedEnvironment ?? hostFallback;
  const redirected = useQuery({ ...startOptionsQuery(nodeId, redirectedEnvironment, context), enabled: enabled && redirectedEnvironment !== null, placeholderData: keepPreviousData });
  const query = redirectedEnvironment === null ? base : redirected;
  const options = base.isError || query.isError ? null : query.data ?? null;
  const previous = last === null ? NO_PICKS : { ...last.picks, environment: last.place === nodeId ? last.picks.environment : null };
  // Freeze the implicit fallback into the context actually read, without making it a manual pick.
  const effective = rememberedEnvironment === null && hostFallback !== null ? { ...picks, environment: hostFallback } : picks;
  const resolved = options === null ? null : resolve(options, effective, mode, previous);
  const retry = async () => {
    if (!enabled) return;
    const fresh = await base.refetch();
    if (!fresh.isError && redirectedEnvironment !== null && fresh.data?.environments.some((row) => canUseEnvironment(row) && row.environment.id === redirectedEnvironment)
      && (rememberedEnvironment !== null || noHostEnvironment(fresh.data) !== null)) await redirected.refetch();
  };
  return {
    options,
    resolved,
    loading: !base.isError && query.isPending,
    failed: base.isError ? base.error : query.isError ? query.error : null,
    refreshing: base.isPlaceholderData || query.isPlaceholderData,
    retry: () => void retry(),
  };
}
