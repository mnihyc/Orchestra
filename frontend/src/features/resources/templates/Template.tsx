/** Driver-schema forms by kind (for environments, or for providers' runtimes), where its image comes from (a reference, a Dockerfile, an archive), its limits and what an environment made from it declares it needs, how its exec node is launched and what it offers, and sharing.
 *
 * The driver's schema supplies the form, defaults and examples. The JSON view preserves the same values,
 * with line errors from the server's save-time checker. Saves use the revision the writer read.
 *
 * Its tabs are the overview, the spec, who may use or manage it, and what is dangerous. A spec that was
 * changed by someone else since it was read is not overwritten: the page says so and reads it again. A
 * template that units were made from cannot be deleted; it can be switched off, which only stops new
 * ones from being made.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { Ban, CirclePlay, Cpu, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { driversQuery, hostsQuery, templateKey, templateQuery, updateTemplate } from '@/api/resources';
import { LimitLines, RuntimeValue } from '@/features/resources/isolation/RuntimeInfo';
import { classNamed, classesOfDriver, limitsTouching, templateRuntime, templateSubjects } from '@/features/resources/isolation/runtimes';
import { ResourceDetails, ResourceHeadingActs, ResourceTabs } from '@/features/resources/DetailSections';
import { TemplateGuideLink } from '@/features/help/TemplateGuideLink';
import { adminSpec } from '@/ui/AdminAct';
import { useAmountWords } from '@/features/resources/amounts';
import { Facts } from '@/features/resources/Facts';
import { ResourceAccess } from '@/features/resources/ResourceAccess';
import { ReadOnlySettingsNotice } from '@/features/settings/ReadOnlyNotice';
import { MakeEnvironmentDialog } from '@/features/resources/environments/MakeEnvironment';
import { TemplateEditor } from '@/features/resources/templates/TemplateEditor';
import { useTemplateActs } from '@/features/resources/templates/acts';
import { useResourceHeading } from '@/features/resources/trail';
import { useConfinementWords } from '@/features/resources/templates/TemplateList';
import { amountsOf, confinementOf, execOffers, hasAmounts, imageSource, readSpec, specSchema, specText } from '@/features/resources/templates/spec';
import type { Template } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Page, PageHeader, PlaceMark, Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { Tooltip } from '@/ui/Tooltip';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { SettingRow as SettingRowLayout } from '@/ui/SettingRow';
import { SettingChanges } from '@/ui/SettingChanges';
import { ValueSummary } from '@/ui/ValueSummary';
import { Skeleton, SkeletonGroup, SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

import { TEMPLATE_TABS, type TemplateTab } from '@/features/resources/tabs';

export { TEMPLATE_TABS, type TemplateTab };

function Skeletons() {
  return (
    <Page>
      <SkeletonGroup className="flex flex-col gap-6">
        <div className="flex items-center gap-4">
          <Skeleton className="size-10 rounded-lg" />
          <div className="flex flex-1 flex-col gap-2">
            <Skeleton className="h-6 w-1/3" />
            <Skeleton className="h-4 w-1/2" />
          </div>
        </div>
        <Skeleton className="h-8 w-full" />
        <SkeletonRows rows={5} />
      </SkeletonGroup>
    </Page>
  );
}

function Overview({ template }: { template: Template }) {
  const { t } = useTranslation();
  const drivers = useQuery(driversQuery);
  const confinementWords = useConfinementWords();
  const words = useAmountWords();
  const driverTitle = drivers.data?.find((driver) => driver.name === template.driver)?.title;
  const hosts = useQuery(hostsQuery(null));
  // The runtime the template starts under, as the hosts that report its driver declare it.
  const runtimeName = templateRuntime(template.spec);
  const runtime = classNamed(classesOfDriver(hosts.data ?? [], template.driver), runtimeName);
  const limits = runtime === undefined ? [] : limitsTouching(runtime.limits, templateSubjects(template.spec, true));
  const unavailable = Object.fromEntries(Object.entries(runtime?.unavailable ?? {}).filter(([key]) => templateSubjects(template.spec).includes(key)));
  const source = imageSource(template.spec);
  const confinement = confinementOf(template.spec);
  const resourceLimits = amountsOf(template.spec, 'resources');
  const most = amountsOf(template.spec, 'resources_max');
  const needs = amountsOf(template.spec, 'needs');
  const exec = execOffers(template.spec);
  const amountRows = (amounts: typeof resourceLimits) => [
    ...(amounts.cpuMillis === null ? [] : [{ name: t('templates.amount.cpu'), value: words.cpu(amounts.cpuMillis) }]),
    ...(amounts.memoryMb === null ? [] : [{ name: t('templates.amount.memory'), value: words.memory(amounts.memoryMb) }]),
    ...(amounts.pids === null ? [] : [{ name: t('templates.amount.pids'), value: words.count(amounts.pids) }]),
    ...(amounts.diskGb === null ? [] : [{ name: t('templates.amount.disk'), value: words.disk(amounts.diskGb) }]),
  ];
  return (
    <div className="flex flex-col gap-6">
      <Section id="template-about" title={t('templates.overview.about')}>
        <Facts compact
          label={t('templates.overview.about')}
          rows={[
            { name: t('templates.overview.driver'), value: driverTitle ?? t('resourceDetails.layout.notReported') },
            ...(runtimeName === null ? [] : [{ name: t('isolation.runtime'), value: runtime === undefined ? runtimeName : <RuntimeValue driverTitle={driverTitle ?? template.driver} runtimeTitle={runtime.title} boundary={runtime.boundary} /> }]),
            ...(runtime === undefined ? [] : [{ name: t('isolation.limitsHere'), value: <LimitLines limits={limits} unavailable={unavailable} boundary={runtime.boundary} /> }]),
            { name: t('templates.overview.home'), value: template.home?.name ?? t('templates.list.system') },
            { name: t('templates.overview.units'), value: t('templates.overview.unitCount', { count: template.units }) },
            ...(template.description === '' ? [] : [{ name: t('templates.overview.description'), value: template.description }]),
            { name: t('templates.overview.imageFrom'), value: source.kind === 'none' ? t('templates.source.none') : t(`templates.sourceChoices.${source.kind}.name`) },
            ...(source.kind === 'image' ? [{ name: t('templates.overview.reference'), value: <span className="wrap-anywhere">{source.reference}</span> }] : []),
            ...amountRows(resourceLimits),
          ]}
        />
      </Section>
      <ResourceDetails><div className="flex min-w-0 flex-col gap-4">
      <Facts compact label={t('common.details')} rows={[{ name: t('templates.overview.address'), value: template.address }]} />
      <Section id="template-confinement" title={t('templates.overview.confinement')} lead={t('templates.overview.confinementLead')}>
        {confinementWords(template.spec) === '–' && !confinement.noNewPrivileges ? (
          <p className="text-sm text-text-muted">{t('templates.overview.confinementNone')}</p>
        ) : (
          <Facts
            label={t('templates.overview.confinement')}
            rows={[
              ...(confinement.runtime === null ? [] : [{ name: t('templates.overview.runtime'), value: confinement.runtime }]),
              ...(confinement.network === null ? [] : [{ name: t('templates.overview.network'), value: confinement.network }]),
              { name: t('templates.overview.readOnly'), value: confinement.readOnlyRoot ? t('templates.overview.yes') : t('templates.overview.no') },
              { name: t('templates.overview.noNewPrivileges'), value: confinement.noNewPrivileges ? t('templates.overview.yes') : t('templates.overview.no') },
            ]}
          />
        )}
      </Section>
      {(hasAmounts(most) || hasAmounts(needs)) && (
        <Section id="template-amounts" title={t('templates.overview.uses')} lead={t('templates.overview.usesLead')}>
          <div className="grid gap-6 sm:grid-cols-2">
            {hasAmounts(most) && (
              <div className="flex flex-col gap-2">
                <h3 className="text-sm font-medium text-text">{t('templates.overview.most')}</h3>
                <Facts label={t('templates.overview.most')} rows={amountRows(most)} />
              </div>
            )}
            {hasAmounts(needs) && (
              <div className="flex flex-col gap-2">
                <h3 className="text-sm font-medium text-text">{t('templates.overview.needs')}</h3>
                <Facts label={t('templates.overview.needs')} rows={amountRows(needs)} />
              </div>
            )}
          </div>
        </Section>
      )}
      {exec.offers.length > 0 && (
        <Section id="template-exec" title={t('templates.overview.exec')} lead={t('templates.overview.execLead')}>
          <Facts
            label={t('templates.overview.exec')}
            rows={[
              { name: t('templates.overview.offers'), value: exec.offers.join(', ') },
              ...(exec.orchTools === null ? [] : [{ name: t('templates.overview.orchTools'), value: exec.orchTools ? t('templates.overview.yes') : t('templates.overview.no') }]),
            ]}
          />
        </Section>
      )}
      </div></ResourceDetails>
    </div>
  );
}

/** The name, the description and the spec, under the revision that was read. */
function SpecForm({ template, manage }: { template: Template; manage: boolean }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const drivers = useQuery(driversQuery);
  const driver = drivers.data?.find((entry) => entry.name === template.driver);
  const schema = driver === undefined ? null : specSchema(driver.descriptor, template.kind);
  const [name, setName] = useState(template.name);
  const [description, setDescription] = useState(template.description);
  const [spec, setSpec] = useState(() => specText(template.spec));
  const [fault, setFault] = useState<string | undefined>(undefined);
  const [validDraft, setValidDraft] = useState(true);
  const [attempt, setAttempt] = useState(0);
  const [base, setBase] = useState(template);
  const [observed, setObserved] = useState(template.revision);
  const parsed = readSpec(spec);
  const nameEdited = name.trim() !== base.name;
  const descriptionEdited = description.trim() !== base.description;
  const specEdited = JSON.stringify(parsed) !== JSON.stringify(base.spec);
  const changed = nameEdited || descriptionEdited || specEdited || !validDraft;
  if (observed !== template.revision) {
    setObserved(template.revision);
    if (!changed) {
      setBase(template);
      setName(template.name);
      setDescription(template.description);
      setSpec(specText(template.spec));
      setAttempt((value) => value + 1);
    }
  }

  const save = useMutation({
    mutationFn: () => {
      if (parsed === null) throw new Error('the spec is not a JSON object');
      return updateTemplate(template.id, {
        expected_revision: base.revision,
        ...(nameEdited ? { name: name.trim() } : {}),
        ...(descriptionEdited ? { description: description.trim() } : {}),
        ...(specEdited ? { spec: parsed } : {}),
      });
    },
    onSuccess: (updated) => {
      setBase(updated);
      setName(updated.name);
      setDescription(updated.description);
      setSpec(specText(updated.spec));
      setFault(undefined);
      setValidDraft(true);
      setAttempt((value) => value + 1);
      queryClient.setQueryData(templateKey(updated.id), updated);
      toast({ tone: 'success', title: t('templates.spec.saved', { name: updated.name }) });
    },
    // Somebody else changed it first: it is read again, and what was typed is kept for the person to look at.
    onError: (error) => {
      if (error instanceof ApiError && error.is('conflict.stale_revision')) void queryClient.invalidateQueries({ queryKey: templateKey(template.id), exact: true });
    },
  });
  const stale = save.error instanceof ApiError && save.error.is('conflict.stale_revision') || changed && template.revision !== base.revision;
  const faults = save.error instanceof ApiError ? save.error.faults() : [];

  const submit = () => {
    if (parsed === null) {
      setFault(t('templates.spec.invalid'));
      return;
    }
    setFault(undefined);
    save.mutate();
  };
  const metadata = (query: string) => {
    const matches = (label: string, value: string) => query.trim().toLocaleLowerCase().split(/\s+/).every((part) => `${label} ${value}`.toLocaleLowerCase().includes(part));
    const matchesName = manage && matches(t('templates.spec.name'), name);
    if (!matchesName && !matches(t('templates.spec.description'), description)) return null;
    return <ul className="divide-y divide-border">
      {manage && <li hidden={!matchesName}><SettingRowLayout as="div" name={t('templates.spec.name')} reason={<Badge>{t('templateSettings.source.template')}</Badge>}
        value={<Input label={t('templates.spec.name')} hideLabel value={name} error={faults.find((fault) => fault.path === 'name')?.message} disabled={save.isPending} onChange={(event) => setName(event.target.value)} autoComplete="off" />} /></li>}
      <li hidden={!matches(t('templates.spec.description'), description)}><SettingRowLayout as="div" name={t('templates.spec.description')} readOnly={!manage} reason={<Badge>{t('templateSettings.source.template')}</Badge>}
        value={manage ? <Input label={t('templates.spec.description')} hideLabel value={description} error={faults.find((fault) => fault.path === 'description')?.message} disabled={save.isPending} onChange={(event) => setDescription(event.target.value)} autoComplete="off" /> : <ValueSummary value={template.description} />} /></li>
    </ul>;
  };

  return (
    <section id="template-spec" aria-label={t('templates.spec.title')} className="flex min-w-0 flex-col gap-3">
      <TemplateGuideLink />
      <div className="flex min-w-0 flex-col gap-3">
        {!manage && <ReadOnlySettingsNotice owner={template.owner} />}
        {drivers.isPending ? <SkeletonRows rows={3} /> : drivers.isError ? <ErrorState compact error={drivers.error} onRetry={() => void drivers.refetch()} /> : schema === null ? <>{metadata('')}<InlineAlert tone="warning">{t('templates.editor.schemaMissing')}</InlineAlert>{!manage && <ValueSummary value={template.spec} />}</>
          : <TemplateEditor key={attempt} compact schema={schema} driver={template.driver} kind={template.kind} leading={metadata} text={spec}
              savedProblems={faults.filter((fault) => fault.path.startsWith('spec')).map((fault) => ({ pointer: fault.path.slice(4), message: fault.message }))}
              onChange={(next) => { setSpec(next); if (!stale) save.reset(); }} onValidityChange={setValidDraft} disabled={save.isPending} readOnly={!manage} />}
        {fault && <p role="status" className="text-sm text-danger-text">{fault}</p>}
        {manage && (
          <div className="flex flex-wrap items-center gap-3">
            <Button variant="primary" pending={save.isPending} blocked={!changed || !validDraft || stale} onClick={submit}>
              {t('templates.spec.save')}
            </Button>
            <Button
              disabled={save.isPending || !changed}
              onClick={() => {
                setName(template.name);
                setDescription(template.description);
                setSpec(specText(template.spec));
                setBase(template);
                setFault(undefined);
                setValidDraft(true);
                setAttempt((value) => value + 1);
                save.reset();
              }}
            >
              {t('templates.spec.revert')}
            </Button>
          </div>
        )}
        {stale && (
          <InlineAlert tone="warning" title={t('templates.spec.changed')}>
            <p>{t('settings.compact.changedReason')}</p>
            {manage && <SettingChanges rows={[
              { name: t('templates.spec.name'), current: template.name, draft: name },
              { name: t('templates.spec.description'), current: template.description, draft: description },
              { name: t('templates.spec.title'), current: template.spec, draft: parsed ?? spec },
            ]} disabled={save.isPending || template.revision === base.revision} reason={!validDraft || parsed === null ? t('templates.editor.invalidJson') : undefined} onKeep={() => {
              if (!nameEdited) setName(template.name);
              if (!descriptionEdited) setDescription(template.description);
              if (!specEdited && validDraft) setSpec(specText(template.spec));
              setBase(template);
              save.reset();
            }} />}
          </InlineAlert>
        )}
        {save.error !== null && !stale && <ErrorAlert error={save.error} />}
      </div>
    </section>
  );
}

function DangerZone({ template, onDisable }: { template: Template; onDisable: () => void }) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-6">
      <Section id="template-switch" title={template.enabled ? t('templates.danger.disableTitle') : t('templates.danger.enableTitle')} lead={template.enabled ? t('templates.danger.disableLead') : t('templates.danger.enableLead')}>
        <div>
          <Button icon={template.enabled ? <Ban aria-hidden /> : <CirclePlay aria-hidden />} onClick={onDisable}>
            {template.enabled ? t('templates.danger.disable') : t('templates.danger.enable')}
          </Button>
        </div>
      </Section>
    </div>
  );
}

export interface TemplateScreenProps {
  templateId: string;
  tab: TemplateTab;
  onTab: (tab: TemplateTab) => void;
}

export function TemplateScreen({ templateId, tab, onTab }: TemplateScreenProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const query = useQuery(templateQuery(templateId));
  const [making, setMaking] = useState(false);
  const back = useResourceHeading('templates', Cpu, query.data?.name ?? t('common.loading'), query.data?.id);
  const acts = useTemplateActs({ onTab: (_template, next) => onTab(next), afterDelete: () => void navigate({ to: '/resources/templates' }) });

  const refused = query.error instanceof ApiError && [401, 403, 404].includes(query.error.status);
  if (query.data === undefined || refused) return query.isError ? <ErrorState className="h-full" error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} /> : <Skeletons />;
  const template = query.data;
  const manage = holds(template.access, 'template.manage');
  const { edit, primary, menu } = acts.page(template);

  const tabs = [
    { value: 'overview' as const, label: t('templates.tabs.overview'), panel: <Overview template={template} /> },
    { value: 'spec' as const, label: t('resourceDetails.layout.settings'), panel: <SpecForm key={template.id} template={template} manage={manage} /> },
    { value: 'access' as const, label: t('templates.tabs.access'), panel: <ResourceAccess access={template.access} target={{ kind: 'template', id: template.id, name: template.name }} /> },
    ...(manage
      ? [{ value: 'danger' as const, label: t('resourceDetails.layout.lifecycle'), panel: <DangerZone template={template} onDisable={() => (template.enabled ? acts.ask('disable', template) : acts.enable(template))} /> }]
      : []),
  ];
  const shown = tabs.some((candidate) => candidate.value === tab) ? tab : 'overview';

  return (
    <Page>
      {query.isError && <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />}
      <PageHeader
        adminAccess={template.access.admin_access} ownership={template} opening={{ kind: 'template', id: template.id }}
        back={back}
        mark={
          <PlaceMark>
            <Cpu aria-hidden />
          </PlaceMark>
        }
        title={template.name}
        facts={[
          template.enabled ? <Badge key="state" tone="accent">{t('templates.list.on')}</Badge> : <Badge key="state" tone="warning">{t('resources.disabled')}</Badge>,
          <Badge key="kind">{t(`templates.kind.${template.kind}`)}</Badge>,
        ]}
        actions={<>{template.kind === 'environment' && holds(template.access, 'template.use') && <Tooltip content={template.enabled ? undefined : t('templates.overview.offLead')}><Button icon={<Plus aria-hidden />} blocked={!template.enabled} onClick={() => setMaking(true)}>{t('environments.make.here')}</Button></Tooltip>}<ResourceHeadingActs omit={shown === 'danger' ? ['enable', 'disable'] : shown === 'spec' ? ['edit', 'rename'] : []} admin={adminSpec(template, 'template.manage')} name={template.name} primary={primary} edit={edit} menu={menu} /></>}
      />
      <ResourceTabs label={t('templates.tabs.label')} value={shown} onValueChange={onTab} tabs={tabs} panelClassName="pt-6" />
      {acts.dialogs}
      {making && <MakeEnvironmentDialog templateId={template.id} onClose={() => setMaking(false)} />}
    </Page>
  );
}
