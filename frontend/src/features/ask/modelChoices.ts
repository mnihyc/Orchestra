/** Runtime rows are the native configured entries, never a second model/Use/policy evaluator. */
import type { TFunction } from 'i18next';
import type { AskPreference, AskTarget, AskModelsReply, BackendRef } from '@/generated/api-types';
import type { ModelEffortEntry } from '@/features/composer/ModelEffort';
import type { ModelSelection } from '@/features/composer/modelSelection';
import { backendIdentity } from '@/features/settings/capabilityBackendValue';
import { resourceTargetIdentity } from '@/lib/resourceTargets';
import { modelPurpose } from '@/features/models/purpose';

export function askTargetIdentity(value: AskTarget): string {
  if ('backend' in value) return backendIdentity(value.backend);
  const identity = resourceTargetIdentity(value.target);
  return value.model ? JSON.stringify([identity, value.model]) : identity;
}

/** The model chip displays the offering while keeping the whole route for its writer. */
export function askSelection(value: AskPreference | null | undefined): (ModelSelection & { backend: BackendRef }) | null {
  if (!value || !('backend' in value)) return null;
  const offering = value.backend.offering;
  return offering === null ? null :
    { provider_id: offering.provider, model: offering.offering, backend: value.backend, params: value.params };
}

export function configuredAskEntries(reply: AskModelsReply | undefined, selected?: AskPreference | null): ModelEffortEntry[] {
  return (reply?.models ?? []).flatMap((entry, index) => {
    const value = askSelection(entry.selection);
    const current = selected !== undefined && selected !== null && askTargetIdentity(entry.selection) === askTargetIdentity(selected);
    return (current || entry.allowed && entry.model !== null && modelPurpose(entry.model, 'generation') !== 'unsupported') && entry.provider !== null && entry.model !== null && value !== null
      ? [{ id: String(index), value, option: { ...entry.model, allowed: entry.allowed }, providerName: entry.provider.name }] : [];
  });
}

export function askModelExclusionText(reason: string, t: TFunction): string {
  switch (reason) {
    case 'disabled': return t('ask.modelExclusion_disabled');
    case 'provider_unavailable': return t('ask.modelExclusion_provider_unavailable');
    case 'generation_unsupported': return t('ask.modelExclusion_generation_unsupported');
    case 'shared_for_ask_only':return t('ask.modelExclusion_shared_for_ask_only');
    case 'shared_for_other_work_only':return t('ask.modelExclusion_shared_for_other_work_only');
    case 'no_use': return t('ask.modelExclusion_no_use');
    case 'parameters_unavailable': return t('ask.modelExclusion_parameters_unavailable');
    case 'excluded_by_policy': return t('ask.modelExclusion_excluded_by_policy');
    default: return t([`capabilityRoutes.reasons.${reason}`, `errors.ask.${reason}`, 'ask.modelUnavailable']);
  }
}
