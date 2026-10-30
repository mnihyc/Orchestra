/** The dialog that edits a provider: its name; what it was made as, shown as fixed; and its endpoints, how agents run on it and its runtime's values, each changed on its own tab.
 *
 * What a provider lets be changed after it was made is what the request takes: a name, its endpoints,
 * its adapter's options, the arrangement types it allows and its runtime's own values. The name is the one
 * thing changed in this form; the others are lists and tables that are read in full and changed where they
 * are drawn, and each stands here with the way to its tab. What it is, the agent whose harness holds its
 * sign-in and where its runtime lies are chosen when it is made, and say so.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { adminSpec } from '@/ui/AdminAct';

import { providerKey, updateProvider } from '@/api/resources';
import { ChangedOn, EditDialog, Fixed } from '@/features/resources/EditDialog';
import { useHarnessTitle } from '@/features/resources/harnesses';
import type { ProviderTab } from '@/features/resources/providers/Provider';
import { providerWhat } from '@/features/resources/providers/state';
import type { Provider } from '@/generated/api-types';
import { Input } from '@/ui/Input';
import { toast } from '@/ui/Toast';

export interface EditProviderProps {
  provider: Provider;
  /** `name` shows the name alone, as Rename does. */
  only?: 'name';
  onTab: (tab: ProviderTab) => void;
  onClose: () => void;
}

export function EditProvider({ provider, only, onTab, onClose }: EditProviderProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const harnessTitle = useHarnessTitle();
  const [name, setName] = useState(provider.name);
  const changed = name.trim() !== provider.name;
  const save = useMutation({
    mutationFn: () => updateProvider(provider.id, { expected_revision: provider.revision, name: name.trim() }),
    onSuccess: (reply) => {
      toast({ tone: 'success', title: t('resources.edit.saved', { name: reply.name }) });
      onClose();
    },
    // Somebody else changed it first: it is read again before the next try.
    onError: () => void queryClient.invalidateQueries({ queryKey: providerKey(provider.id) }),
  });
  const go = (tab: ProviderTab) => () => {
    onClose();
    onTab(tab);
  };

  return (
    <EditDialog
      admin={adminSpec(provider, 'provider.manage')}
      title={only === 'name' ? t('resources.edit.renameTitle', { name: provider.name }) : t('resources.edit.title', { name: provider.name })}
      changed={changed}
      valid={name.trim() !== ''}
      pending={save.isPending}
      error={save.error}
      onSave={() => save.mutate()}
      onClose={onClose}
      {...(only === 'name'
        ? {}
        : {
            rest: (
              <>
                <Fixed label={t('providers.overview.what')} value={providerWhat(provider, harnessTitle, t)} why={t(provider.kind === 'subscription' ? 'providers.edit.kindSubscriptionWhy' : 'providers.edit.kindApiWhy')} />
                <Fixed
                  label={t('providers.overview.runs')}
                  value={provider.machine?.name ?? provider.runtime?.host?.name ?? t('providers.list.noRuntime')}
                  why={provider.machine === null ? t('providers.edit.runsWhy') : t('providers.edit.runsMachineWhy', { machine: provider.machine.name })}
                />
                {(provider.harness_kind === null || provider.protocols.length > 0) && (
                  <ChangedOn label={t('providers.edit.endpoints')} value={t('providers.what.api', { count: provider.upstreams.length })} tab={t('providers.tabs.key')} onOpen={go('key')} />
                )}
                <ChangedOn
                  label={t('providers.tabs.arrangements')}
                  value={t('providers.edit.arrangementsValue', { count: provider.arrangements.filter((consent) => consent.allowed).length })}
                  tab={t('providers.tabs.arrangements')}
                  onOpen={go('arrangements')}
                />
                <ChangedOn label={t('providers.edit.runtime')} value={t('providers.edit.runtimeValue')} tab={t('providers.tabs.settings')} onOpen={go('settings')} />
              </>
            ),
          })}
    >
      <Input label={t('resources.edit.name')} value={name} disabled={save.isPending} onChange={(event) => setName(event.target.value)} autoComplete="off" />
    </EditDialog>
  );
}
