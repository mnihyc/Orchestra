/** A provider harness uses the common home choice and the runtime machine's declared roots. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { providerKey } from '@/api/resources';
import { putSetting, targetSettingsKey } from '@/api/settings';
import { DataSourcesEditor, madeDataSources } from '@/features/resources/environments/DataSources';
import { SettingsTable } from '@/features/resources/SettingsTable';
import type { EnvironmentDataSources, Provider, ResolvedSetting } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';

const homeRoles = ['home'] as const;
function homeOf(value: unknown): EnvironmentDataSources['home'] {
  if (typeof value === 'object' && value !== null && 'kind' in value && value.kind === 'existing_directory' && 'path' in value && typeof value.path === 'string') return { kind: 'existing_directory', path: value.path };
  return { kind: 'made_by_orch' };
}

export function ProviderHome({ provider, setting }: { provider: Provider; setting: ResolvedSetting }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const target = { kind: 'provider' as const, id: provider.id };
  const [value, setValue] = useState(() => ({ ...madeDataSources(), home: provider.runtime_support?.home ?? homeOf(setting.value) }));
  const [ready, setReady] = useState(false);
  const [revision, setRevision] = useState(setting.own?.revision ?? null);
  const save = useMutation({ mutationFn: () => putSetting(target, 'runtime.home', { value: value.home, expectedRevision: revision }),
    onSuccess: async (next) => {
      setRevision(next.own?.revision ?? null);
      await Promise.all([client.invalidateQueries({ queryKey: targetSettingsKey(target) }), client.invalidateQueries({ queryKey: providerKey(provider.id) })]);
    } });
  return <section id="provider-harness-home" className="flex flex-col gap-3" aria-label={t('environmentData.harnessHome')}>
    <h2 className="font-semibold">{t('environmentData.harnessHome')}</h2>
    <SettingsTable settings={[setting]} label={t('environmentData.harnessHome')} target={target} />
    <DataSourcesEditor machine={provider.runtime_support?.machine_id ?? undefined} driver={provider.runtime_support?.driver ?? undefined}
      dataStorage={provider.runtime_support?.data_storage}
      roles={homeRoles} value={value} onChange={(next) => { setValue(next); save.reset(); }} onReady={setReady} disabled={!setting.writable || save.isPending} />
    <p className="text-sm text-text-muted">{t(provider.runtime_support?.lifecycle === 'unit' ? 'environmentData.update' : 'environmentData.harnessAttach')}</p>
    {setting.writable && <Button pending={save.isPending} disabled={!ready} onClick={() => save.mutate()}>{t('resources.edit.save')}</Button>}
    {save.error && <ErrorAlert error={save.error} />}
    {save.isSuccess && <InlineAlert tone="info">{t('unitConfiguration.saved')}</InlineAlert>}
  </section>;
}
