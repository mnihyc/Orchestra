/** Safe native requirements, expressed as alternatives without exposing configuration expressions. */
import type { TFunction } from 'i18next';
import type { ParameterRequirement } from '@/generated/api-types';
import { accessProgramDisplayName, effortDisplayName } from '@/features/models/names';

export function requirementWords(requirement: ParameterRequirement, t: TFunction, locale: string, negated = false): string {
  const join = (parts: ParameterRequirement[], any: boolean) => new Intl.ListFormat(locale, { type: any !== negated ? 'disjunction' : 'conjunction' })
    .format(parts.map((part) => requirementWords(part, t, locale, negated)));
  const leaf = (words: string) => negated ? t('composer.requirements.without', { requirement: words }) : words;
  switch (requirement.kind) {
    case 'all': return join(requirement.requirements, false);
    case 'any': return join(requirement.alternatives, true);
    case 'not': return requirementWords(requirement.requirement, t, locale, !negated);
    case 'placement': return leaf(t(requirement.placement === 'managed' ? 'composer.requirements.managed' : 'composer.requirements.connected'));
    case 'provider_kind': return leaf(t(requirement.provider_kind === 'api' ? 'composer.requirements.api' : 'composer.requirements.subscription'));
    case 'credential_path': {
      const words = {
        native: 'composer.requirements.native', forwarded: 'composer.requirements.forwarded',
        shared: 'composer.requirements.shared', handover: 'composer.requirements.handover',
      } as const;
      return leaf(t(words[requirement.path]));
    }
    case 'model_feature': {
      const words = {
        web_search: 'composer.requirements.webSearch', efforts: 'composer.requirements.efforts', service_tiers: 'composer.requirements.speeds',
        access_programs: 'composer.requirements.programs', input_images: 'composer.requirements.images', context_max: 'composer.requirements.context',
      } as const;
      return leaf(t(words[requirement.feature]));
    }
    case 'model_value': {
      const label = requirement.list === 'efforts' ? effortDisplayName(requirement.value, t)
        : requirement.list === 'access_programs' ? accessProgramDisplayName(requirement.value, t) : requirement.value;
      return leaf(t('composer.requirements.modelValue', { value: label }));
    }
    case 'program_version': return leaf(t('composer.requirements.version', { range: requirement.range }));
    case 'agent': return leaf(t('composer.requirements.agent'));
    case 'feature': return leaf(t('composer.requirements.feature'));
    case 'provider': return leaf(t('composer.requirements.provider'));
  }
}

/** Only safe observed categories, never the private expression or configuration used to decide. */
export function requirementActualWords(requirement: ParameterRequirement, t: TFunction, locale: string): string | undefined {
  const parts = requirement.kind === 'all' ? requirement.requirements : requirement.kind === 'any' ? requirement.alternatives : requirement.kind === 'not' ? [requirement.requirement] : null;
  if (parts !== null) {
    const words = [...new Set(parts.map((part) => requirementActualWords(part, t, locale)).filter((part): part is string => part !== undefined))];
    return words.length === 0 ? undefined : new Intl.ListFormat(locale).format(words);
  }
  if (requirement.kind === 'credential_path' && requirement.actual !== undefined && requirement.actual !== null) return requirementWords({ ...requirement, path: requirement.actual }, t, locale);
  if (requirement.kind === 'provider_kind' && requirement.actual !== undefined && requirement.actual !== null) return requirementWords({ ...requirement, provider_kind: requirement.actual }, t, locale);
  if (requirement.kind === 'placement' && requirement.actual !== undefined && requirement.actual !== null) return requirementWords({ ...requirement, placement: requirement.actual }, t, locale);
  if (requirement.kind === 'program_version') return requirement.actual ?? undefined;
  return undefined;
}
