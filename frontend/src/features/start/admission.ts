/** Check the exact displayed setup against its native options; the atomic write rechecks it at admission. */
import { ENVIRONMENT_POOL_PREFIX } from '@/features/resources/pools/environmentTarget';
import { ApiError } from '@/api/client';
import type { StartOptions } from '@/api/start';
import type { ArrangementType } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { type BindingPool, NEW_ENVIRONMENT, resolve, startModelName } from './choices';

export interface NamedStart {
  provider: string;
  agent: string;
  model: string | null;
  bindingPool?: BindingPool | undefined;
  effort: string | null;
  arrangement: ArrangementType;
  environmentPool?: string | undefined;
  environment: { id: string; name: string } | null;
}

export function checkStart(offered: StartOptions, chosen: NamedStart): void {
  const reviewed = resolve(offered, { provider: chosen.provider, agent: chosen.agent, model: chosen.model, bindingPool: chosen.bindingPool ?? null,
    effort: chosen.effort, arrangement: chosen.arrangement, environment: chosen.environmentPool === undefined ? chosen.environment?.id ?? NEW_ENVIRONMENT : ENVIRONMENT_POOL_PREFIX + chosen.environmentPool }, { suggest: false, respectDefaults: true });
  const same = reviewed.provider?.id === chosen.provider && reviewed.agent?.id === chosen.agent
    && reviewed.bindingPool?.id === chosen.bindingPool?.id && startModelName(reviewed) === chosen.model && reviewed.effort === chosen.effort && reviewed.arrangement === chosen.arrangement
    && (chosen.environmentPool !== undefined ? reviewed.environmentPool === chosen.environmentPool : chosen.environment === null ? reviewed.fresh : reviewed.environment?.environment.id === chosen.environment.id);
  if (same && reviewed.missing.length === 0 && !reviewed.settling) return;
  const provider = offered.providers.find((row) => row.provider.id === chosen.provider);
  const agent = provider?.agents.find((row) => row.agent.id === chosen.agent);
  const arrangement = agent?.arrangements.find((row) => row.type === chosen.arrangement);
  const reason = provider?.reason ?? agent?.reason ?? arrangement?.reason ?? 'setup_changed';
  throw new ApiError({ type: 'orch:error:binding.incompatible', code: 'binding.incompatible', status: 409,
    title: i18n.t('errors.binding.incompatible'), detail: i18n.t('startPreflight.changed'), request_id: '', reason }, null);
}
