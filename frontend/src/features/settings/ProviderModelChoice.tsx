/** A model chosen by name from the models its provider has enabled: it follows the provider, and says beside itself when the provider does not offer the one saved. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { providerModelsQuery } from '@/api/resources';
import { Combobox } from '@/ui/Combobox';

export function ProviderModelChoice({ providerId, value, label, onChange, disabled = false, error }: {
  providerId: string; value: string; label: string; onChange: (model: string) => void; disabled?: boolean; error?: string | undefined;
}) {
  const { t } = useTranslation();
  const models = useQuery({ ...providerModelsQuery(providerId), enabled: providerId !== '', retry: false });
  const [typed, setTyped] = useState('');
  const enabled = (models.data ?? []).filter((model) => model.enabled);
  const match = typed.trim().toLocaleLowerCase();
  const options = enabled.filter((model) => match === '' || `${model.display_name} ${model.model_name}`.toLocaleLowerCase().includes(match))
    .map((model) => { const name = model.display_name === '' ? model.model_name : model.display_name; return { value: model.model_name, label: name, text: name }; });
  const offered = models.data === undefined || value === '' || enabled.some((model) => model.model_name === value);
  return <Combobox label={label} query={typed} onQueryChange={setTyped} displayValue={value} disabled={disabled || providerId === ''}
    placeholder={t(providerId === '' ? 'settingsEditors.summarizer.providerFirst' : 'settingsEditors.summarizer.chooseModel')} loading={models.isFetching}
    error={error ?? (offered ? undefined : t('settingsEditors.summarizer.modelNotOffered', { model: value }))}
    groups={[{ id: 'models', label: '', options }]} empty={t('ui.combobox.empty')} onOpenChange={(open) => { if (!open) setTyped(''); }}
    onSelect={(option) => { setTyped(''); onChange(option.value); }} />;
}
