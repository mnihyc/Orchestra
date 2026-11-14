/** A model chosen by name from the models its provider has enabled: it follows the provider, and says beside itself when the provider does not offer the one saved.
 *
 * Where a name that the provider does not list has to stay possible (a model a provider's endpoint takes without listing it), what is typed can be used as it is. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { providerModelsQuery } from '@/api/resources';
import { Combobox } from '@/ui/Combobox';

export function ProviderModelChoice({ providerId, value, label, onChange, disabled = false, error, placeholder, allowTyped = false }: {
  providerId: string; value: string; label: string; onChange: (model: string) => void; disabled?: boolean; error?: string | undefined;
  /** What the field says while no model is chosen. */
  placeholder?: string | undefined;
  /** Whether a name typed in full can be used though the provider does not list it. */
  allowTyped?: boolean;
}) {
  const { t } = useTranslation();
  const models = useQuery({ ...providerModelsQuery(providerId), enabled: providerId !== '', retry: false });
  const [typed, setTyped] = useState('');
  const enabled = (models.data ?? []).filter((model) => model.enabled);
  const match = typed.trim().toLocaleLowerCase();
  const options = enabled.filter((model) => match === '' || `${model.display_name} ${model.model_name}`.toLocaleLowerCase().includes(match))
    .map((model) => { const name = model.display_name === '' ? model.model_name : model.display_name; return { value: model.model_name, label: name, text: name }; });
  const entered = typed.trim();
  if (allowTyped && entered !== '' && !enabled.some((model) => model.model_name === entered)) options.push({ value: entered, label: t('settingsEditors.model.use', { name: entered }), text: entered });
  const offered = models.data === undefined || value === '' || enabled.some((model) => model.model_name === value);
  return <Combobox label={label} query={typed} onQueryChange={setTyped} displayValue={value} disabled={disabled || providerId === '' && !allowTyped}
    placeholder={placeholder ?? t(providerId === '' ? 'settingsEditors.summarizer.providerFirst' : 'settingsEditors.summarizer.chooseModel')} loading={models.isFetching}
    error={error ?? (offered ? undefined : t('settingsEditors.summarizer.modelNotOffered', { model: value }))}
    groups={[{ id: 'models', label: '', options }]} empty={t('ui.combobox.empty')} onOpenChange={(open) => { if (!open) setTyped(''); }}
    onSelect={(option) => { setTyped(''); onChange(option.value); }} />;
}
