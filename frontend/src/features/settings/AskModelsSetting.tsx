/** The system curates API identities; an org can only remove from its served effective intersection. */
import { useQueries, useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { providerModelsQuery, providerQuery, providersQuery } from '@/api/resources';
import type { SettingsTarget } from '@/api/settings';
import type { ModelEffortEntry } from '@/features/composer/ModelEffort';
import { sameSelectionModel, type ModelIdentity } from '@/features/composer/modelSelection';
import { askSelection } from '@/features/ask/modelChoices';
import { record } from '@/features/resources/UnitCommands';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { ModelOption, Provider, ResolvedSetting } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { AskModelsEditor } from './AskModelsEditor';
import type { ValueControlProps } from './ValueControl';

export function askModelIdentities(value: unknown): ModelIdentity[] {
  return Array.isArray(value) ? value.flatMap((item) => {
    const backend = record(record(item).backend);
    const offering = record(backend.offering);
    if (typeof backend.route !== 'string' || typeof offering.provider !== 'string' || typeof offering.offering !== 'string') return [];
    const selected = askSelection({ backend: { ...backend, route: backend.route, config: backend.config ?? null, offering: { provider: offering.provider, offering: offering.offering } } });
    return selected === null ? [] : [selected];
  }) : [];
}

export function AskModelsSetting({ target, setting, value, label, onChange, onEdited, disabled = false }: ValueControlProps & { target: SettingsTarget; setting: ResolvedSetting }) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const system = target.kind === 'system';
  const effective = askModelIdentities(setting.value);
  const requested = askModelIdentities(value ?? setting.value);
  const rows = system ? requested : effective.filter((entry) => requested.some((wanted) => sameSelectionModel(wanted, entry)));
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
      const backend = { route: 'provider.generate', config: null, offering: { provider: provider.id, offering: model.model_name } };
      return [{ id: `${provider.id}:${model.id}`, value: { provider_id: provider.id, model: model.model_name, backend }, option, providerName: provider.name }];
    });
  });
  const reads = system && !disabled ? [list, ...models] : [...details, ...models];
  const failure = reads.find((read) => read.isError)?.error;
  const privateRead = failure instanceof ApiError && [401, 403, 404, 410].includes(failure.status);
  const pending = shown && reads.some((read) => read.isPending);
  return <div className="flex min-w-0 flex-col gap-2">
    {pending && <SkeletonRows rows={2} />}
    {privateRead ? <StateLine compact message={t('implicitAvailability.detailsUnavailable')} /> : failure !== undefined && <ErrorState compact error={failure} onRetry={() => { for (const read of reads) if (read.isError) void read.refetch(); }} />}
    <AskModelsEditor label={label} value={rows} entries={entries} disabled={disabled} mode={system ? 'system' : 'narrow'}
      onChange={(next) => { if (!disabled && next.every((entry) => entry.backend !== undefined)) { onChange(next.map(({ backend }) => ({ backend }))); onEdited?.(); } }} />
  </div>;
}
