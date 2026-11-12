/** Runtime rows are the native configured entries, never a second model/Use/policy evaluator. */
import type { TFunction } from 'i18next';
import type { AskModelsReply } from '@/generated/api-types';
import type { ModelEffortEntry } from '@/features/composer/ModelEffort';

export function configuredAskEntries(reply: AskModelsReply | undefined): ModelEffortEntry[] {
  return (reply?.models ?? []).flatMap((entry, index) => entry.allowed && entry.provider !== null && entry.model !== null ? [{
    id: String(index), value: entry.selection, option: entry.model, providerName: entry.provider.name,
  }] : []);
}

export function askModelExclusionText(reason: string, t: TFunction): string {
  switch (reason) {
    case 'disabled': return t('ask.modelExclusion_disabled');
    case 'provider_unavailable': return t('ask.modelExclusion_provider_unavailable');
    case 'generation_unsupported': return t('ask.modelExclusion_generation_unsupported');
    case 'no_use': return t('ask.modelExclusion_no_use');
    case 'parameters_unavailable': return t('ask.modelExclusion_parameters_unavailable');
    case 'excluded_by_policy': return t('ask.modelExclusion_excluded_by_policy');
    default: return t('ask.modelUnavailable');
  }
}
