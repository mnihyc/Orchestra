/** The model that threads run on, chosen by name from the models of the provider in force at the place.
 *
 * The provider in force is the one the place's own settings resolve for `agent.default_provider_id`, so the list follows what is chosen
 * beside it; a model that provider does not list can still be used by typing its name, and where no provider is in force the name is typed
 * (the thread's own model menu is the product's picker among everything a thread can run on).
 */
import { useState } from 'react';
import { poolQuery } from '@/api/pools';
import { usePoolTargetChoices } from '@/features/resources/pools/targetChoices';
import { defaultModelName, defaultProviderId, defaultPoolId, namedModel } from './threadDefaults';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { targetSettingsQuery, type SettingsTarget } from '@/api/settings';
import { providerModelsQuery } from '@/api/resources';
import { ParameterInput } from '@/features/models/ParameterInput';
import { effortDisplayName } from '@/features/models/names';
import { ProviderModelChoice } from '@/features/settings/ProviderModelChoice';
import type { ValueControlProps } from '@/features/settings/ValueControl';

export const AGENT_MODEL_SETTING = 'agent.model';

/** The provider that threads here use, as the place's settings resolve it; none while the place's settings are not read or name none. */
function useProviderInForce(target: SettingsTarget): string {
  // What the page already holds is enough: the choice of a provider beside this one is written through the same list.
  const settings = useQuery({ ...targetSettingsQuery(target), staleTime: Infinity });
  const row = settings.data?.find((setting) => setting.key === 'agent.default_provider_id');
  return defaultProviderId(row?.value) ?? '';
}

export function AgentModelChoice({ target, value, label, onChange, onEdited, disabled = false, error, inheritance }: ValueControlProps & { target: SettingsTarget }) {
  const { t } = useTranslation();
  const provider = useProviderInForce(target);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const pool = defaultPoolId(value, 'model');
  const cache = useQueryClient();
  const choices = usePoolTargetChoices('model', open, query);
  const selected = useQuery({ ...poolQuery(pool ?? ''), enabled: open && pool !== null, retry: false });
  return <ProviderModelChoice inheritance={inheritance} allowTyped providerId={provider} label={label} value={defaultModelName(value) ?? ''} disabled={disabled} error={error}
    poolChoices={{ id: pool, label: pool === null ? null : selected.data?.name ?? t('pools.selectedModel'), choices,
      onSearch: setQuery, onOpen: setOpen,
      onSelect: (id) => {
        const row = choices.rows.find((choice) => choice.id === id);
        if (row) cache.setQueryData(poolQuery(id).queryKey, row);
        onEdited?.();
        onChange({type:'pool',kind:'model',id});
      } }}
    placeholder={t(provider === '' ? 'settingsEditors.model.name' : 'settingsEditors.model.choose')} onChange={(model) => { onEdited?.(); onChange(namedModel(model)); }} />;
}

/** Effort remains its own setting, chosen among the selected model's declared efforts. */
export function AgentEffortChoice({ target, value, label, onChange, onEdited, disabled = false, error, inheritance }: ValueControlProps & { target: SettingsTarget }) {
  const { t } = useTranslation();
  const settings = useQuery({ ...targetSettingsQuery(target), staleTime: Infinity });
  const provider = defaultProviderId(settings.data?.find((row) => row.key === 'agent.default_provider_id')?.value);
  const model = defaultModelName(settings.data?.find((row) => row.key === 'agent.model')?.value);
  const models = useQuery({ ...providerModelsQuery(typeof provider === 'string' ? provider : ''), enabled: typeof provider === 'string' && typeof model === 'string', retry: false });
  const chosen = models.data?.find((entry) => entry.model_name === model && entry.enabled);
  return <div className="flex min-w-0 flex-col gap-1">
    <ParameterInput inheritance={inheritance} parameterKey="agent.effort" value={value} constraint={null} disabled={disabled || inheritance === undefined && (chosen === undefined || (chosen.parameters.efforts?.length ?? 0) === 0)}
      info={{ title: label, description: '', group: 'model', type: 'enum', options: (chosen?.parameters.efforts ?? []).map((effort) => ({ value: effort, label: effortDisplayName(effort, t) })) }}
      onChange={(next) => { onEdited?.(); onChange(next); }} />
    {error !== undefined && <p role="alert" className="text-sm text-danger-text">{error}</p>}
  </div>;
}
