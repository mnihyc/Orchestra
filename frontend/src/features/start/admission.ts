/** Check the exact displayed setup against its native options; the atomic write rechecks it at admission. */
import { ApiError } from '@/api/client';
import type { StartOptions } from '@/api/start';
import type { ArrangementType } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { NEW_ENVIRONMENT, resolve } from './choices';

export interface NamedStart {
  provider: string;
  agent: string;
  model: string;
  effort: string | null;
  arrangement: ArrangementType;
  environment: { id: string; name: string } | null;
}

export function checkStart(offered: StartOptions, chosen: NamedStart): void {
  const reviewed = resolve(offered, { provider: chosen.provider, agent: chosen.agent, model: chosen.model,
    effort: chosen.effort, arrangement: chosen.arrangement, environment: chosen.environment?.id ?? NEW_ENVIRONMENT }, { suggest: false, respectDefaults: true });
  const same = reviewed.provider?.id === chosen.provider && reviewed.agent?.id === chosen.agent
    && reviewed.model?.model_name === chosen.model && reviewed.effort === chosen.effort && reviewed.arrangement === chosen.arrangement
    && (chosen.environment === null ? reviewed.fresh : reviewed.environment?.environment.id === chosen.environment.id);
  if (same && reviewed.missing.length === 0 && !reviewed.settling) return;
  const provider = offered.providers.find((row) => row.provider.id === chosen.provider);
  const agent = provider?.agents.find((row) => row.agent.id === chosen.agent);
  const arrangement = agent?.arrangements.find((row) => row.type === chosen.arrangement);
  const reason = provider?.reason ?? agent?.reason ?? arrangement?.reason ?? 'setup_changed';
  throw new ApiError({ type: 'orch:error:binding.incompatible', code: 'binding.incompatible', status: 409,
    title: i18n.t('errors.binding.incompatible'), detail: i18n.t('startPreflight.changed'), request_id: '', reason }, null);
}
