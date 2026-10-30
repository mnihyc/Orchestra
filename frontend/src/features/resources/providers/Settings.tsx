/** The values a provider's runtime is made with, each with where it comes from: its template and host, its resources, when it stops by itself, whether it is updated when it starts and how often it is started to renew.
 *
 * Read from the settings the provider resolves, in the order the system gives them. A value says which
 * place it comes from (the system's default, or a place that set it) and whether this reader may change
 * it. Changing them is done where they are set, on the settings pages; this tab is the list of what is in force.
 */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { providerSettingsQuery } from '@/api/resources';
import { SettingsTable } from '@/features/resources/SettingsTable';
import type { Provider } from '@/generated/api-types';
import { Section } from '@/shell/Page';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { ProviderSources } from './Sources';

export function ProviderSettings({ provider }: { provider: Provider }) {
  const { t } = useTranslation();
  const settings = useQuery(providerSettingsQuery(provider.id));
  return (
    <div className="flex min-w-0 flex-col gap-6">
    <ProviderSources provider={provider} />
    <Section id="provider-settings" title={t('providerRecovery.runtimeSettings')}>
      {settings.isPending ? (
        <SkeletonRows rows={4} />
      ) : settings.isError ? (
        <ErrorState compact error={settings.error} retrying={settings.isRefetching} onRetry={() => void settings.refetch()} />
      ) : (
        <>
          <SettingsTable settings={settings.data} label={t('providers.settings.title')} target={{ kind: 'provider', id: provider.id }} place={{ name: provider.name, owner: provider.owner, access: provider.access, kind: null }} runtime={provider.runtime_changes} />
        </>
      )}
    </Section>
    </div>
  );
}
