/** The dialog that makes an environment from a template on a host: its name, the template, the host or the one with the most room, and who may use it; the job that makes it is followed from the page it opens on.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { createEnvironment, environmentKey } from '@/api/environments';
import { ApiError } from '@/api/client';
import { hostsQuery, templatesQuery } from '@/api/resources';
import { settingsQuery } from '@/api/settings';
import { borrowsData, DataSourcesEditor, madeDataSources } from '@/features/resources/environments/DataSources';
import { NewAccess } from '@/features/resources/NewAccess';
import { HomePicker } from '@/features/resources/HomePicker';
import { Hint } from '@/features/help/Hint';
import type { Environment, Host, NewEnvironmentAccess, NewResourceAccess } from '@/generated/api-types';
import { useActKey, useForm } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { RuntimeChoice, defaultRuntime, runtimeOffer } from '@/features/resources/isolation/RuntimeChoice';
import { classNamed, useIsolationWords } from '@/features/resources/isolation/runtimes';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';
import { TemporaryChoice } from './Temporary';

/** The choice of a host that stands for "the one with the most room": a menu cannot hold an empty value. */
const MOST_ROOM = '\u0000most-room';

/** Whether a host takes new environments from this template: it is open to them, and its driver is the template's and makes environments. */
function takesFrom(host: Host, driver: string | undefined): boolean {
  return host.schedulable && host.drivers.some((entry) => entry.name === driver && entry.creates.includes('environment'));
}

export interface MakeEnvironmentProps {
  onClose: () => void;
  /** The place the environment is made for, which gives it its settings; left out, it is made in the person's own place and its page is opened. */
  home?: { id: string; name: string };
  /** Presets from the template and host pages never silently fall back to another resource. */
  templateId?: string;
  hostId?: string;
  /** A newly made thread becomes this temporary environment's creating thread. */
  forThread?: boolean;
  /** Told which environment was made, where the person is to stay where they are: the dialog then does not open the new environment's page. */
  onMade?: (environment: Environment) => void;
}

export function MakeEnvironmentDialog({ onClose, home, templateId, hostId, forThread = false, onMade }: MakeEnvironmentProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const key = useActKey();
  const templates = useQuery(templatesQuery('environment', null));
  const hosts = useQuery(hostsQuery(null));
  const [dataSources, setDataSources] = useState(madeDataSources);
  const [sourcesReady, setSourcesReady] = useState(true);
  const [access, setAccess] = useState<NewResourceAccess>('only_me');
  const [placed, setPlaced] = useState(false);
  const [temporary, setTemporary] = useState(false);
  // The runtime chosen for this environment; empty takes the template's own.
  const [runtime, setRuntime] = useState('');
  const isolation = useIsolationWords();
  const form = useForm({ home_node_id: home?.id ?? '', name: '', template_id: templateId ?? '', host_id: hostId ?? MOST_ROOM }, {
    home_node_id: (value) => value === '' ? t('environments.make.homeEmpty') : null,
  });
  // What the place allows of runtimes (`environments.runtimes`), where the environment will be made.
  const placeSettings = useQuery({ ...settingsQuery(form.values.home_node_id), enabled: form.values.home_node_id !== '', retry: false });
  const runtimePolicy = placeSettings.data?.find((setting) => setting.key === 'environments.runtimes')?.value;
  const formElement = useRef<HTMLFormElement>(null);
  const errorElement = useRef<HTMLDivElement>(null);

  const usable = (templates.data ?? []).filter((template) => template.enabled);
  const template = form.values.template_id === '' ? usable[0] ?? null : usable.find((entry) => entry.id === form.values.template_id) ?? null;
  const open = (hosts.data ?? []).filter((host) => takesFrom(host, template?.driver));
  const closed = (hosts.data ?? []).filter((host) => !takesFrom(host, template?.driver));
  const host = (hosts.data ?? []).find((entry) => entry.id === form.values.host_id) ?? null;
  const hostAvailable = form.values.host_id === MOST_ROOM || host !== null && open.some((entry) => entry.id === host.id);

  const make = useMutation({
    mutationFn: (name: string) => {
      if (template === null) throw new Error('there is no template to make it from');
      // An environment is shared as its place decides, or with nobody: the third choice of other resources is not one of its.
      const asked: NewEnvironmentAccess = access === 'only_me' ? 'only_me' : 'defaults';
      if (!sourcesReady) throw new Error(t('environmentData.chooseAllowed'));
      const config = { ...(temporary ? { 'env.lifetime': forThread ? 'thread' : 'temporary' } : {}), ...(runtime === '' ? {} : { 'env.runtime': runtime }) };
      const body = { ...(name.trim() === '' ? {} : { name: name.trim() }), home_node_id: form.values.home_node_id, template_id: template.id, host_id: host === null ? null : host.id, access: asked, ...(Object.keys(config).length === 0 ? {} : { config }), ...(borrowsData(dataSources) ? { data_sources: dataSources } : {}) };
      return createEnvironment(body, key(body));
    },
    onSuccess: (environment) => {
      queryClient.setQueryData(environmentKey(environment.id), environment);
      toast({ tone: 'success', title: t('environments.make.done', { name: environment.name }) });
      onClose();
      if (onMade === undefined) void navigate({ to: '/resources/environments/$id', params: { id: environment.id } });
      else onMade(environment);
    },
    onError: (error) => setPlaced(form.refuse(error)),
  });

  useEffect(() => {
    if (make.error === null) return;
    const element = formElement.current?.querySelector<HTMLElement>('[aria-invalid="true"]') ?? errorElement.current;
    element?.scrollIntoView({ block: 'center' });
    element?.focus();
  }, [make.error, placed]);
  const unplaced = make.error instanceof ApiError && make.error.faults().some((fault) => !Object.hasOwn(form.values, fault.path));

  const loading = templates.isPending || hosts.isPending;
  const failed = templates.isError ? templates.error : hosts.isError ? hosts.error : null;

  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !make.isPending && onClose()}
      title={t('environments.make.title')}
      dismissible={!make.isPending}
      footer={
        <>
          <Button disabled={make.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" type="submit" form="make-environment" pending={make.isPending} blocked={template === null || form.values.home_node_id === '' || !hostAvailable || !sourcesReady}>
            {t('environments.make.submit')}
          </Button>
        </>
      }
    >
      <form ref={formElement} id="make-environment" className="flex flex-col gap-4 py-2" noValidate onSubmit={form.submit((values) => template !== null && hostAvailable && make.mutate(values.name))}>
        {make.error !== null && (!placed || unplaced) && <div ref={errorElement} tabIndex={-1}><ErrorAlert error={make.error} /></div>}
        {loading ? (
          <SkeletonRows rows={3} />
        ) : failed !== null ? (
          <ErrorState compact error={failed} retrying={templates.isRefetching || hosts.isRefetching} onRetry={() => void Promise.all([templates.refetch(), hosts.refetch()])} />
        ) : usable.length === 0 ? (
          <InlineAlert tone="warning" title={t('environments.make.noTemplate')}>
            {t('environments.make.noTemplateLead')}
          </InlineAlert>
        ) : (
          <>
            <HomePicker value={form.values.home_node_id} onChange={(value) => form.set('home_node_id', value)} error={form.field('home_node_id').error}
              places={home === undefined ? [] : [home]} disabled={make.isPending} />
            <Input label={t('environments.make.name')} {...form.field('name')} autoComplete="off" />
            <Select
              label={t('environments.make.template')}
              help={<Hint id="env-template" />}
              value={template?.id ?? form.values.template_id}
              error={form.field('template_id').error ?? (template === null ? t('environments.make.templateUnavailable') : undefined)}
              onValueChange={(next) => {
                form.set('template_id', next);
                form.set('host_id', hostId ?? MOST_ROOM);
                setDataSources(madeDataSources());
                setRuntime('');
              }}
              options={usable.map((entry) => {
                // Each template says what it runs on and under, from what the hosts report of it.
                const choice = resourceChoice({ kind: 'template', template: entry });
                const reporters = (hosts.data ?? []).filter((candidate) => takesFrom(candidate, entry.driver));
                const offer = runtimeOffer(entry, reporters);
                const runtimeName = defaultRuntime(offer);
                const driverTitle = (hosts.data ?? []).flatMap((candidate) => candidate.drivers).find((driver) => driver.name === entry.driver)?.title ?? entry.driver;
                const title = classNamed(offer.classes, runtimeName)?.title ?? runtimeName;
                return { value: entry.id, ...choiceOption(t, 'dynamic.resource', { name: entry.name }), ...choice, description: <>{choice.description}<span className="text-text-muted"> · {isolation.pair(driverTitle, title)}</span></> };
              })}
            />
            <Select
              label={t('environments.make.host')}
              help={<Hint id="env-host" />}
              hint={open.length === 0 ? t('environments.make.noHost') : undefined}
              value={form.values.host_id}
              error={form.field('host_id').error ?? (!hostAvailable ? t('environments.make.hostUnavailable') : undefined)}
              disabled={open.length === 0}
              onValueChange={(id) => { form.set('host_id', id); setDataSources(madeDataSources()); setRuntime(''); }}
              options={[
                { value: MOST_ROOM, ...choiceOption(t, 'placement.automatic'), description: t('environments.make.mostRoom') },
                ...open.map((entry) => ({ value: entry.id, ...choiceOption(t, 'dynamic.resource', { name: entry.name }), ...resourceChoice({ kind: 'host', host: entry }) })),
                // A host that takes none from this template is shown and cannot be chosen, with why, and not left out as if it were not there.
                ...closed.map((entry) => ({ value: entry.id, ...choiceOption(t, 'dynamic.resource', { name: entry.name }), ...resourceChoice({ kind: 'host', host: entry }), description: entry.schedulable ? t('environments.make.hostNoDriver') : t('environments.make.hostClosed'), disabled: true })),
              ]}
            />
            {template !== null && <RuntimeChoice template={template} hosts={host !== null && hostAvailable ? [host] : open} value={runtime} onChange={setRuntime} policy={runtimePolicy} disabled={make.isPending} />}
            {template !== null && <DataSourcesEditor machine={host?.machine.id} driver={template.driver} value={dataSources} onChange={setDataSources} onReady={setSourcesReady} disabled={make.isPending} />}
            <NewAccess value={access} onChange={setAccess} home={t('environments.make.yourPlace')} allowHome={false} />
            <TemporaryChoice checked={temporary} onChange={setTemporary} disabled={make.isPending} />
          </>
        )}
      </form>
    </Dialog>
  );
}
