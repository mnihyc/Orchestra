/** The dialog that registers a directory on a machine as an environment: its name, the directory, its org, whether files may be written and shells opened from outside, and who may use it.
 */
import { useMutation } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { registerEnvironment } from '@/api/environments';
import { readMachineFacts } from '@/features/resources/machines/facts';
import { DataSourcesEditor, madeDataSources } from '@/features/resources/environments/DataSources';
import { HomePicker, useHomeChoices } from '@/features/resources/HomePicker';
import { NewAccess } from '@/features/resources/NewAccess';
import { Hint } from '@/features/help/Hint';
import type { Machine, NewEnvironmentAccess, NewResourceAccess } from '@/generated/api-types';
import { useActKey, useForm } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Switch } from '@/ui/Switch';
import { toast } from '@/ui/Toast';

export function RegisterEnvironmentDialog({ machine, open, onClose }: { machine: Machine; open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const facts = readMachineFacts(machine.facts);
  const policy = facts.policy;
  // What the machine switched off is off for everybody: the switch is there to read, and cannot be turned on.
  const writeAllowed = policy === null || policy['environment.remote_write'] === true;
  const shellAllowed = policy === null || policy['environment.terminal'] === true;

  const [home, setHome] = useState('');
  const [remoteWrite, setRemoteWrite] = useState<boolean | undefined>();
  const [terminal, setTerminal] = useState<boolean | undefined>();
  const [dataSources, setDataSources] = useState(madeDataSources);
  const [sourcesReady, setSourcesReady] = useState(false);
  const [access, setAccess] = useState<NewResourceAccess>('defaults');
  const key = useActKey();
  // Whether the form took the server's complaint to one of its fields; if not, it is said as a whole.
  const [placed, setPlaced] = useState(false);
  const choices = useHomeChoices(home).choices;
  const homeName = choices.find((choice) => choice.id === home)?.label ?? '';

  const form = useForm(
    { name: '' },
    {
      name: (value) => (value.trim() === '' ? t('machines.environment.nameEmpty') : null),
    },
  );

  const register = useMutation({
    mutationFn: (values: { name: string }) => {
      if (!sourcesReady) throw new Error(t('environmentData.chooseAllowed'));
      const asked: NewEnvironmentAccess = access === 'only_me' ? 'only_me' : 'defaults';
      const body = {
        home_id: home,
        name: values.name.trim(),
        root:dataSources.workspace.kind==='existing_directory'?dataSources.workspace.path:'',
        data_sources: dataSources,
        ...(remoteWrite === undefined ? {} : {remote_write:remoteWrite}),
        ...(terminal === undefined ? {} : {terminal}),
        access: asked,
      };
      return registerEnvironment(machine.id, body, key(body));
    },
    onSuccess: (environment) => {
      toast({ tone: 'success', title: t('machines.environment.done', { name: environment.name }) });
      onClose();
      void navigate({ to: '/resources/environments/$id', params: { id: environment.id } });
    },
    onError: (error) => setPlaced(form.refuse(error)),
  });

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !register.isPending && onClose()}
      size="lg"
      title={t('machines.environment.title', { machine: machine.name })}
      description={t('machines.environment.lead')}
      dismissible={!register.isPending}
      footer={
        <>
          <Button disabled={register.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" type="submit" form="register-environment" pending={register.isPending} blocked={home === '' || !sourcesReady}>
            {t('machines.environment.submit')}
          </Button>
        </>
      }
    >
      <form
        id="register-environment"
        className="flex flex-col gap-4 py-2"
        noValidate
        onSubmit={form.submit((values) => home !== '' && register.mutate(values))}
      >
        <Input label={t('machines.environment.name')} {...form.field('name')} autoComplete="off" />
        <DataSourcesEditor machine={machine.id} value={dataSources} onChange={setDataSources} onReady={setSourcesReady} disabled={register.isPending} />
        <HomePicker value={home} onChange={setHome} />
        <Switch
          label={t('machines.environment.remoteWrite')}
          help={<Hint id="env-remote-write" />}
          {...(writeAllowed ? {} : { hint: t('machines.environment.switchedOff') })}
          checked={writeAllowed && (remoteWrite ?? true)}
          disabled={!writeAllowed || register.isPending}
          onCheckedChange={setRemoteWrite}
        />
        <Switch
          label={t('machines.environment.terminal')}
          help={<Hint id="env-terminal" />}
          {...(shellAllowed ? {} : { hint: t('machines.environment.switchedOff') })}
          checked={shellAllowed && (terminal ?? true)}
          disabled={!shellAllowed || register.isPending}
          onCheckedChange={setTerminal}
        />
        <NewAccess value={access} onChange={setAccess} home={homeName} allowHome={false} disabled={register.isPending} />
        <p className="text-sm text-text-muted">{t('machines.environment.reads')}</p>
        {register.error !== null && !placed && <ErrorAlert error={register.error} />}
      </form>
    </Dialog>
  );
}
