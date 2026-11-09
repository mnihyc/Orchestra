/** The system curates API identities; an org can only remove from its served effective intersection. */
import { useQueries, useQuery } from '@tanstack/react-query';
import { providerModelsQuery, providerQuery, providersQuery } from '@/api/resources';
import type { SettingsTarget } from '@/api/settings';
import type { ModelEffortEntry } from '@/features/composer/ModelEffort';
import type { ModelIdentity } from '@/features/composer/modelSelection';
import { record } from '@/features/resources/UnitCommands';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { ModelOption, Provider, ResolvedSetting } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { AskModelsEditor } from './AskModelsEditor';
import type { ValueControlProps } from './ValueControl';

export function askModelIdentities(value: unknown): ModelIdentity[] {
  return Array.isArray(value) ? value.flatMap((item) => {
    const entry = record(item);
    return typeof entry.provider_id === 'string' && typeof entry.model === 'string'
      ? [{ provider_id: entry.provider_id, model: entry.model }] : [];
  }) : [];
}

export function AskModelsSetting({ target, setting, value, label, onChange, onEdited, disabled = false }: ValueControlProps & { target: SettingsTarget; setting: ResolvedSetting }) {
  const shown = useViewShown();
  const system = target.kind === 'system';
  const effective = askModelIdentities(setting.value);
  const requested = askModelIdentities(value ?? setting.value);
  const rows = system ? requested : effective.filter((entry) => requested.some((wanted) => wanted.provider_id === entry.provider_id && wanted.model === entry.model));
  const list = useQuery({ ...providersQuery('usable', target.kind === 'node' ? target.id : null), enabled: shown && system && !disabled });
  const ids = [...new Set(rows.map((entry) => entry.provider_id))];
  const details = useQueries({ queries: ids.map((id) => ({ ...providerQuery(id), enabled: shown && (!system || disabled), retry: false })) });
  const providers: Provider[] = system && !disabled ? list.isError ? [] : (list.data ?? []).filter((provider) => provider.kind === 'api')
    : details.flatMap((read) => !read.isError && read.data?.kind === 'api' ? [read.data] : []);
  const models = useQueries({ queries: providers.map((provider) => ({ ...providerModelsQuery(provider.id), enabled: shown, retry: false })) });
  const entries: ModelEffortEntry[] = providers.flatMap((provider, index) => {
    const read = models[index];
    if (read === undefined || read.isError) return [];
    return (read.data ?? []).flatMap((model) => {
      if (system ? !model.enabled : !rows.some((entry) => entry.provider_id === provider.id && entry.model === model.model_name)) return [];
      const option: ModelOption = { ...model, allowed: model.enabled, reason: model.enabled ? null : 'disabled' };
      return [{ id: `${provider.id}:${model.id}`, value: { provider_id: provider.id, model: model.model_name }, option, providerName: provider.name }];
    });
  });
  const failure = system && !disabled ? list.error ?? models.find((read) => read.isError)?.error : null;
  const pending = system && !disabled && (list.isPending || models.some((read) => read.isPending));
  return <div className="flex min-w-0 flex-col gap-2">
    {pending && <SkeletonRows rows={2} />}
    {failure !== null && failure !== undefined && <ErrorState compact error={failure} onRetry={() => { void list.refetch(); for (const read of models) void read.refetch(); }} />}
    <AskModelsEditor label={label} value={rows} entries={entries} disabled={disabled} mode={system ? 'system' : 'narrow'}
      onChange={(next) => { if (!disabled) { onChange(next); onEdited?.(); } }} />
  </div>;
}
