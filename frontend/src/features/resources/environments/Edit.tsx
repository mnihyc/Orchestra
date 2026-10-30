/** The dialog that edits an environment: its name and the two switches that decide what people may do in it from outside; where it runs and what it belongs to, shown as fixed; and its own settings, changed on their tab.
 *
 * What an environment lets be changed after it was made is what the request takes: a name, its own values for
 * the settings of its scope, whether files may be written from outside, whether shells may be opened in it, and
 * another home. The name and the switches are here and are saved together under the revision that was read. The
 * settings are a long table that says where each value comes from, and are changed on their own tab. A move to
 * another home is not offered yet, and the form says so. Whoever opens it as Rename sees the name alone.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { adminSpec } from '@/ui/AdminAct';

import { environmentKey, updateEnvironment } from '@/api/environments';
import { hostQuery } from '@/api/resources';
import { DataSourcesEditor, madeDataSources } from '@/features/resources/environments/DataSources';
import type { EnvironmentTab } from '@/features/resources/environments/Environment';
import { howItRuns } from '@/features/resources/environments/words';
import { ChangedOn, EditDialog, Fixed } from '@/features/resources/EditDialog';
import { Hint } from '@/features/help/Hint';
import { PlacePicker, type PickedPlace } from '@/features/settings/PlacePicker';
import type { Environment } from '@/generated/api-types';
import { Input } from '@/ui/Input';
import { InlineAlert } from '@/ui/InlineAlert';
import { Switch } from '@/ui/Switch';
import { toast } from '@/ui/Toast';

export interface EditEnvironmentProps {
  environment: Environment;
  /** `name` shows the name alone, as Rename does. */
  only?: 'name';
  onTab: (tab: EnvironmentTab) => void;
  onClose: () => void;
}

export function EditEnvironment({ environment, only, onTab, onClose }: EditEnvironmentProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [name, setName] = useState(environment.name);
  const [write, setWrite] = useState(environment.remote_write);
  const [shell, setShell] = useState(environment.terminal);
  const initialSources = environment.data_sources ?? madeDataSources();
  const [dataSources, setDataSources] = useState(initialSources);
  const [sourcesReady, setSourcesReady] = useState(true);
  const selectedHost = useQuery({ ...hostQuery(environment.host?.id ?? ''), enabled: only === undefined && environment.host !== null });
  const sourceMachine = environment.host !== null ? selectedHost.data?.machine.id : environment.machine?.id;
  const sourcesChanged = only === undefined && JSON.stringify(dataSources) !== JSON.stringify(initialSources);
  const [home, setHome] = useState<PickedPlace | null>({ id: environment.home.id, kind: environment.home.kind, color: null, folder: false, path: environment.home.name ?? environment.address });
  const nameChanged = name.trim() !== environment.name;
  const writeChanged = only === undefined && write !== environment.remote_write;
  const shellChanged = only === undefined && shell !== environment.terminal;
  const homeChanged = only === undefined && home !== null && home.id !== environment.home.id;
  const save = useMutation({
    mutationFn: () =>
      updateEnvironment(environment.id, {
        expected_revision: environment.revision,
        ...(nameChanged ? { name: name.trim() } : {}),
        ...(writeChanged ? { remote_write: write } : {}),
        ...(shellChanged ? { terminal: shell } : {}),
        ...(homeChanged ? { home_node_id: home.id } : {}),
        ...(sourcesChanged ? { data_sources: dataSources } : {}),
      }),
    onSuccess: (updated) => {
      toast({ tone: 'success', title: t('resources.edit.saved', { name: updated.name }) });
      onClose();
    },
    // Somebody else changed it first: it is read again before the next try.
    onError: () => void queryClient.invalidateQueries({ queryKey: environmentKey(environment.id) }),
  });

  return (
    <EditDialog
      admin={adminSpec(environment, 'environment.manage')}
      title={only === 'name' ? t('resources.edit.renameTitle', { name: environment.name }) : t('resources.edit.title', { name: environment.name })}
      changed={nameChanged || writeChanged || shellChanged || homeChanged || sourcesChanged}
      valid={name.trim() !== '' && (only === 'name' || home !== null) && (!sourcesChanged || sourcesReady)}
      pending={save.isPending}
      error={save.error}
      onSave={() => save.mutate()}
      onClose={onClose}
      {...(only === 'name'
        ? {}
        : {
            rest: (
              <>
                <Fixed label={t('environments.overview.runs')} value={howItRuns(environment, t, true)} why={t('environments.edit.runsWhy')} />
                <ChangedOn
                  label={t('environments.tabs.settings')}
                  value={t('environments.edit.settingsValue')}
                  tab={t('environments.tabs.settings')}
                  onOpen={() => {
                    onClose();
                    onTab('settings');
                  }}
                />
              </>
            ),
          })}
    >
      <Input label={t('resources.edit.name')} value={name} disabled={save.isPending} onChange={(event) => setName(event.target.value)} autoComplete="off" />
      {only === undefined && (
        <>
          <Switch label={t('environments.overview.remoteWrite')} help={<Hint id="env-remote-write" />} checked={write} disabled={save.isPending} onCheckedChange={setWrite} />
          <Switch label={t('environments.overview.terminal')} help={<Hint id="env-terminal" />} checked={shell} disabled={save.isPending} onCheckedChange={setShell} />
          <PlacePicker label={t('environments.overview.home')} value={home} onChange={setHome} rootOrgId={environment.org_id} disabled={save.isPending} />
          <DataSourcesEditor machine={sourceMachine} driver={environment.template?.driver} value={dataSources} onChange={setDataSources} onReady={setSourcesReady} disabled={save.isPending} />
          {sourcesChanged && <InlineAlert tone="warning">{t('environmentData.update')}</InlineAlert>}
          {homeChanged && <InlineAlert tone="warning">{t('resourceDetails.environment.moveHelp', { place: home.path })}</InlineAlert>}
        </>
      )}
    </EditDialog>
  );
}
