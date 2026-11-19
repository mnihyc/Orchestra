/** A model chosen by name from the models its provider has enabled: it follows the provider, and says beside itself when the provider does not offer the one saved.
 *
 * Where a name that the provider does not list has to stay possible (a model a provider's endpoint takes without listing it), what is typed can be used as it is. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { usePoolTargetChoices } from '@/features/resources/pools/targetChoices';
import { providerModelsQuery } from '@/api/resources';
import { Combobox } from '@/ui/Combobox';
import type { ChoiceInheritance } from '@/ui/InheritedChoice';
import type { ModelPurpose } from '@/generated/api-types';
import { modelPurpose, modelsForPurpose } from '@/features/models/purpose';
import { splitRecent, useModelHistory } from '@/features/models/history';
import { resourceChoice } from '@/features/resource-facts/ResourceFacts';
import type { ProviderChoiceUsage } from '@/generated/api-types';
import { ErrorAlert } from '@/ui/ErrorState';
import { Button } from '@/ui/Button';
import { ReadFeedback } from '@/ui/ReadFeedback';

export function ProviderModelChoice({ providerId, value, label, onChange, disabled = false, error, placeholder, allowTyped = false, inheritance, purpose = 'generation', noneLabel, inspect = false, usage, effort, poolChoices }: {
  providerId: string; value: string; label: string; onChange: (model: string) => void; disabled?: boolean; error?: string | undefined;
  /** What the field says while no model is chosen. */
  placeholder?: string | undefined;
  /** Whether a name typed in full can be used though the provider does not list it. */
  allowTyped?: boolean;
  inheritance?: ChoiceInheritance | undefined;
  /** Null is a matching rule over any kind of call, rather than work to perform. */
  purpose?: ModelPurpose | null;
  noneLabel?: string;
  inspect?: boolean;
  usage?: ProviderChoiceUsage | null | undefined;
  effort?: string | null | undefined;
  poolChoices?: {
    id: string | null;
    label: string | null;
    choices: ReturnType<typeof usePoolTargetChoices>;
    onSearch: (query: string) => void;
    onOpen: (open: boolean) => void;
    onSelect: (id: string) => void;
  };
}) {
  const { t } = useTranslation();
  const models = useQuery({ ...providerModelsQuery(providerId), enabled: providerId !== '', retry: false });
  const [typed, setTyped] = useState('');
  const [open, setOpen] = useState(false);
  const history = useModelHistory(open);
  const search = (query: string) => {
    setTyped(query);
    poolChoices?.onSearch(query);
  };
  const catalogue = (models.data ?? []).filter((model) => model.enabled);
  const enabled = purpose === null ? catalogue : modelsForPurpose(catalogue, purpose);
  const match = typed.trim().toLocaleLowerCase();
  const options = enabled.map((model) => { const name = model.display_name.trim() === '' ? model.model_name : model.display_name; return { value: model.model_name, label: name, text: name,
      ...(inspect ? { preview: resourceChoice({ kind: 'model', model, providerId, usage, effort }).preview } : {}),
      description: purpose !== null && modelPurpose(model, purpose) === 'unknown' ? t('modelPicker.purposeUnknown') : undefined }; });
  const entered = typed.trim();
  const promoted = splitRecent(options, history.history, (row) => ({ provider_id: providerId, model_name: row.value }));
  const matching = <T extends { value: string; text: string }>(rows: readonly T[]) => rows.filter((row) => match === '' || `${row.text} ${row.value}`.toLocaleLowerCase().includes(match));
  const groups = [
    ...(noneLabel === undefined || match !== '' && !noneLabel.toLocaleLowerCase().includes(match) ? [] : [{ id: 'unchanged', label: '', options: [{ value: '', label: noneLabel, text: noneLabel }] }]),
    ...(promoted.recent.length === 0 ? [] : [{ id: 'recent', label: t('modelPicker.recent'), options: matching(promoted.recent) }]),
    { id: 'models', label: '', options: matching(promoted.rest) },
    ...(poolChoices === undefined ? [] : [{ id: 'model-pools', label: t('screens.pools.title'), options: poolChoices.choices.options }]),
    ...(allowTyped && entered !== '' && !(models.data ?? []).some((model) => model.model_name === entered)
      ? [{ id: 'typed', label: '', options: [{ value: entered, label: t('settingsEditors.model.use', { name: entered }), text: entered, description: t('modelPicker.purposeUnknown') }] }] : []),
  ];
  const offered = models.data === undefined || value === '' || enabled.some((model) => model.model_name === value);
  const current = catalogue.find((model) => model.model_name === value);
  const wrongPurpose = current !== undefined && purpose !== null && modelPurpose(current, purpose) === 'unsupported';
  const display = current?.display_name;
  return <div className="flex min-w-0 flex-col gap-1"><Combobox inheritance={inheritance} label={label} query={typed} onQueryChange={search} displayValue={poolChoices?.label ?? (value === '' ? noneLabel ?? '' : display === undefined || display.trim() === '' ? value : display)} initialActiveValue={poolChoices?.id ? `\u0000pool:${poolChoices.id}` : value} disabled={disabled || providerId === '' && !allowTyped}
    placeholder={placeholder ?? t(providerId === '' ? 'settingsEditors.summarizer.providerFirst' : 'settingsEditors.summarizer.chooseModel')} loading={models.isFetching || poolChoices?.choices.loading === true} footer={poolChoices?.choices.footer}
    error={error ?? (offered ? undefined : wrongPurpose ? t('modelPicker.notForWork') : t('settingsEditors.summarizer.modelNotOffered', { model: value }))}
    groups={groups} empty={t(purpose !== null && catalogue.length > 0 && enabled.length === 0 ? 'modelPicker.noSuitableModels' : 'ui.combobox.empty')} onOpenChange={(next) => { setOpen(next); poolChoices?.onOpen(next); if (!next) search(''); }}
    onSelect={(option) => { search('');
      const pool = poolChoices?.choices.rows.find((row) => option.value === `\u0000pool:${row.id}`);
      if (pool && poolChoices) { poolChoices.onSelect(pool.id); return; }
      if (option.value !== '') history.remember({ provider_id: providerId, model_name: option.value }); onChange(option.value); }} />
    <ReadFeedback reads={[history.read]} pending={history.read.isFetching && history.history === undefined} />
    {models.isError && <ErrorAlert error={models.error} action={<Button size="sm" pending={models.isFetching} onClick={() => void models.refetch()}>{t('common.tryAgain')}</Button>} />}
  </div>;
}
