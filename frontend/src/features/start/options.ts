/** The server's answer to what a thread of a place could be bound to, shared by the dialog and inline starter.
 *
 * The dialog and the inline starter read one answer through this, so that they say the same of the same place. A
 * profile whose harness has never said what it is cannot be placed, and the server lists it as such.
 * The confirmed option set remains while resources change; Retry reads one current placement plan.
 */
import { keepPreviousData, useQuery } from '@tanstack/react-query';

import { startOptionsQuery } from '@/api/start';

import { canUseEnvironment, type Mode, NO_PICKS, type Picks, resolve, type Resolved } from './choices';
import { useLastSetup } from './lastSetup';
import type { StartOptions } from '@/api/start';

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
export function useStartOptions(nodeId: string, picks: Picks, mode: Mode): Started {
  const last = useLastSetup();
  const environmentId = picks.environment;
  const base = useQuery({ ...startOptionsQuery(nodeId, environmentId), placeholderData: keepPreviousData });
  // A place's environment rule wins. Reuse a location only in that same place, while it is still usable.
  const rememberedEnvironment = !base.isError && !base.isPlaceholderData && environmentId === null && picks.environment === null && last?.place === nodeId && base.data?.environment_default === 'shared' && !base.data.environments.some((row) => row.is_default && canUseEnvironment(row))
    ? base.data.environments.find((row) => canUseEnvironment(row) && row.environment.id === last.picks.environment)?.environment.id ?? null : null;
  const remembered = useQuery({ ...startOptionsQuery(nodeId, rememberedEnvironment), enabled: rememberedEnvironment !== null, placeholderData: keepPreviousData });
  const query = rememberedEnvironment === null ? base : remembered;
  const options = base.isError || query.isError ? null : query.data ?? null;
  const previous = last === null ? NO_PICKS : { ...last.picks, environment: last.place === nodeId ? last.picks.environment : null };
  const resolved = options === null ? null : resolve(options, picks, mode, previous);
  const retry = async () => {
    const fresh = await base.refetch();
    if (!fresh.isError && rememberedEnvironment !== null && fresh.data?.environments.some((row) => canUseEnvironment(row) && row.environment.id === rememberedEnvironment)) await remembered.refetch();
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
