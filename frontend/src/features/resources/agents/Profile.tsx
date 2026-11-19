/** Harness version, capabilities, parameters and configuration.
 *
 * An agent profile says which harness to run and how. Its page has four parts: the overview, which
 * says what the harness is and how well this build is supported, which bindings it can work in and what
 * it can do, drawn from what its adapter declares; the parameters it publishes, with where each may be
 * set and when a change takes effect; the profile itself, as a name, a version pin and the JSON of the
 * rest; and who may use or manage it. A profile whose harness has not reported yet declares nothing, and
 * says so. Switching a profile off and deleting it ask first; deleting is refused while a thread is bound.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { AGENT_TABS, type AgentTab } from '@/features/resources/tabs';
import { useNavigate } from '@tanstack/react-router';
import { Bot } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { StateLine } from '@/ui/StateLine';
import { choiceOption } from '@/i18n/choices';

import { ApiError } from '@/api/client';
import { agentKey, agentQuery, harnessKindsQuery, updateAgent } from '@/api/resources';
import { adminSpec } from '@/ui/AdminAct';
import { useAgentActs } from '@/features/resources/agents/acts';
import { DescriptorOverrides } from '@/features/resources/agents/Overrides';
import { type DescriptorView, harnessSupportWords, harnessVersionWords, humanize, readDescriptor, reportedHarnessName, versionPin } from '@/features/resources/agents/descriptor';
import { Facts } from '@/features/resources/Facts';
import { DetailPages, ResourceDetails, ResourceTabs as Tabs, ResourceHeadingActs as HeadingActs } from '@/features/resources/DetailSections';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { useSay } from '@/features/settings/SettingRow';
import { useHarnessTitle } from '@/features/resources/harnesses';
import { programNameIn } from '@/features/resources/programNames';
import { Bindings } from '@/features/resources/agents/Bindings';
import { ProgramBuild } from '@/features/resources/agents/ProgramBuild';
import { ResourceAccess } from '@/features/resources/ResourceAccess';
import { ReadOnlySettingsNotice } from '@/features/settings/ReadOnlyNotice';
import { useResourceHeading } from '@/features/resources/trail';
import type { Agent } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Page, PageHeader, PlaceMark, Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { SettingRow as SettingRowLayout } from '@/ui/SettingRow';
import { SettingEditor } from '@/ui/SettingEditor';
import { SettingChanges } from '@/ui/SettingChanges';
import { ValueSummary } from '@/ui/ValueSummary';
import { Skeleton, SkeletonGroup, SkeletonRows } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';
import { Textarea } from '@/ui/Textarea';
import { toast } from '@/ui/Toast';

export { AGENT_TABS, type AgentTab };

/** The sentinel of the version menu for "the default build": a menu cannot hold an empty value. */
const DEFAULT_BUILD = '\u0000default';

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

/** Only an opened technical report reads the program catalogue used by its details. */
function OverviewDetails({ agent, descriptor }: { agent: Agent; descriptor: DescriptorView }) {
  const { t } = useTranslation();
  const kinds = useQuery(harnessKindsQuery);
  const registered = kinds.data?.find((entry) => entry.kind === agent.harness_kind);
  const builds = registered?.builds ?? [];
  const program = registered !== undefined && 'program' in registered ? registered.program : null;
  const knownRange = descriptor.harness?.range ?? null;
  return <>
      <Facts compact rows={[
        { name: t('agents.overview.address'), value: agent.address },
        ...(knownRange === null ? [] : [{ name: t('agents.overview.range'), value: knownRange }]),
        ...(builds.length === 0 ? [] : [{ name: t('agents.overview.builds'), value: builds.map((build) => harnessVersionWords(t, build.version)).join(', ') }]),
      ]} />
      {descriptor.arrangements.length > 0 && <Bindings arrangements={descriptor.arrangements} program={program} />}
      {descriptor.capabilities.length > 0 && (
        <Section id="agent-capabilities" title={t('agents.overview.capabilities')} lead={t('agents.overview.capabilitiesLead')}>
          <Facts label={t('agents.overview.capabilities')} rows={descriptor.capabilities.map((entry) => ({ name: humanize(entry.key), value: entry.value === 'true' ? t('agents.overview.yes') : entry.value === 'false' ? t('agents.overview.no') : entry.value }))} />
        </Section>
      )}
      {(descriptor.sandboxModes.length > 0 || descriptor.approvalModes.length > 0) && (
        <Section id="agent-modes" title={t('agents.overview.modes')}>
          <Facts
            label={t('agents.overview.modes')}
            rows={[
              ...(descriptor.sandboxModes.length === 0 ? [] : [{ name: t('agents.overview.sandbox'), value: descriptor.sandboxModes.join(', ') }]),
              ...(descriptor.approvalModes.length === 0 ? [] : [{ name: t('agents.overview.approval'), value: descriptor.approvalModes.join(', ') }]),
            ]}
          />
        </Section>
      )}
  </>;
}

function Overview({ agent, descriptor }: { agent: Agent; descriptor: DescriptorView }) {
  const { t, i18n } = useTranslation();
  const support = descriptor.harness;
  const seenVersion = support?.version ?? null;
  const pin = versionPin(agent.profile);
  return (
    <div className="flex flex-col gap-6">
      {!descriptor.declared && (
        <InlineAlert tone="info" title={t('agents.overview.notReported')}>
          {t('agents.overview.notReportedLead')}
        </InlineAlert>
      )}
      <Section id="agent-about" title={t('agents.overview.about')}>
        <Facts compact
          label={t('agents.overview.about')}
          rows={[
            ...(support === null || support.status === ''
              ? []
              : [
                  {
                    name: t('agents.overview.support'),
                    value: <Badge tone={support.status === 'supported' ? 'accent' : 'warning'}>{harnessSupportWords(t, support.status)}</Badge>,
                  },
                ]),
            ...(seenVersion === null ? [] : [{ name: t('agents.overview.version'), value: harnessVersionWords(t, seenVersion) }]),
            { name: t('agents.overview.pin'), value: pin === null ? t('agents.list.pinDefault') : harnessVersionWords(t, pin) },
            { name: t('agents.overview.usable'), value: agent.machine === null ? t('agents.list.anyRuntime') : t('agents.overview.onMachine', { machine: agent.machine.name }) },
          ]}
        />
      </Section>
      {seenVersion !== null && <ProgramBuild program={{ kind: agent.harness_kind, version: seenVersion, build: null }} name={programNameIn(agent, i18n.resolvedLanguage ?? i18n.language) ?? reportedHarnessName(agent.descriptor, i18n.resolvedLanguage ?? i18n.language) ?? undefined} />}
      <ResourceDetails><OverviewDetails agent={agent} descriptor={descriptor} /></ResourceDetails>
    </div>
  );
}

function ParameterDefault({ value }: { value: string }) {
  const { t } = useTranslation();
  const say = useSay();
  if (value.trim().startsWith('←')) return <>{t('resourceDetails.layout.modelDefault')}</>;
  if (value === '') return <>{t('resourceDetails.layout.notReported')}</>;
  let parsed: unknown = value;
  try { const next: unknown = JSON.parse(value); parsed = next; } catch { /* An adapter may supply ordinary prose. */ }
  return <ValueSummary value={parsed} word={say} />;
}

function Parameters({ agent, descriptor }: { agent: Agent; descriptor: DescriptorView }) {
  const { t } = useTranslation();
  if (descriptor.parameters.length === 0) {
    return <EmptyState compact icon={<Bot aria-hidden />} title={t('agents.parameters.none')} description={t('agents.parameters.noneLead')} />;
  }
  const columns: Column<DescriptorView['parameters'][number]>[] = [
    {
      id: 'title',
      header: t('agents.parameters.parameter'),
      cell: (parameter, shown) => {
        const withoutWhere = !shown.has('where');
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <span className="font-medium text-text">{parameter.title}</span>
            <code className="text-xs text-text-subtle">{parameter.key}</code>
            {parameter.description !== '' && <span className="text-sm text-text-muted">{parameter.description}</span>}
            {withoutWhere && parameter.scopes.length > 0 && <span className="text-sm text-text-muted">{t('agents.parameters.setAt', { list: parameter.scopes.join(', ') })}</span>}
          </div>
        );
      },
    },
    { id: 'type', header: t('agents.parameters.type'), from: 'sm', className: 'text-text-muted', cell: (parameter) => parameter.type },
    { id: 'values', header: t('agents.parameters.values'), from: 'md', cell: (parameter) => <span className="wrap-anywhere">{parameter.options === '' ? '–' : parameter.options}</span> },
    { id: 'default', header: t('agents.parameters.default'), from: 'md', className: 'text-text-muted', cell: (parameter) => (parameter.default === '' ? '–' : parameter.default) },
    {
      id: 'where',
      header: t('agents.parameters.where'),
      from: 'lg',
      cell: (parameter) => (
        <span className="flex flex-col gap-1">
          <span className="flex flex-wrap gap-1">
            {parameter.scopes.map((scope) => (
              <Badge key={scope}>{scope}</Badge>
            ))}
          </span>
          {parameter.apply !== '' && <span className="text-sm text-text-muted">{t('agents.parameters.apply', { when: parameter.apply.replaceAll('_', ' ') })}</span>}
        </span>
      ),
    },
  ];
  return (
    <Section id="agent-parameters" title={t('agents.parameters.title')}>
      <p className="text-sm text-text-muted">{t('resourceDetails.layout.parametersHere')}</p>
      <DetailPages items={descriptor.parameters}>{(parameters) => <ul aria-label={t('agents.parameters.title')} className="divide-y divide-border rounded-lg border border-border">
        {parameters.map((parameter) => <SettingRowLayout key={parameter.key} name={parameter.title} description={parameter.description} readOnly
          value={<ParameterDefault value={parameter.default} />}
          reason={<SourceSettingLink source={{ scope: agent.home?.kind ?? 'system', node: agent.home }} def={{ key: parameter.key, class: 'parameter' }}
            fallback={agent.home?.name ?? t('resourceDetails.system')} text={t('resourceDetails.layout.openDefaults')} />} />)}
      </ul>}</DetailPages>
      <ResourceDetails><Table label={t('agents.parameters.title')} columns={columns} rows={descriptor.parameters} rowKey={(parameter) => parameter.key} /></ResourceDetails>
    </Section>
  );
}

/** The profile as a name, a version pin and the JSON of the rest. */
function ProfileForm({ agent, manage }: { agent: Agent; manage: boolean }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const kinds = useQuery(harnessKindsQuery);
  const builds = kinds.data?.find((entry) => entry.kind === agent.harness_kind)?.builds ?? [];
  const [name, setName] = useState(agent.name);
  const [json, setJson] = useState(() => JSON.stringify(agent.profile ?? {}, null, 2));
  const [fault, setFault] = useState<string | undefined>(undefined);
  const [jsonOpen, setJsonOpen] = useState(false);
  const [base, setBase] = useState(agent);
  const [observed, setObserved] = useState(agent.revision);
  const pin = versionPin(agent.profile) ?? DEFAULT_BUILD;

  const parsed = (): Record<string, unknown> | null => {
    try {
      const value: unknown = JSON.parse(json);
      return typeof value === 'object' && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
    } catch {
      return null;
    }
  };
  const current = parsed();
  const shownPin = manage ? typeof current?.harness_version === 'string' ? current.harness_version : DEFAULT_BUILD : pin;
  const nameEdited = name.trim() !== base.name;
  const profileEdited = JSON.stringify(current) !== JSON.stringify(base.profile ?? {});
  const changed = nameEdited || profileEdited;
  if (observed !== agent.revision) {
    setObserved(agent.revision);
    if (!changed) {
      setBase(agent);
      setName(agent.name);
      setJson(JSON.stringify(agent.profile ?? {}, null, 2));
    }
  }

  const save = useMutation({
    mutationFn: () => {
      const profile = parsed();
      if (profile === null) throw new Error('the profile is not a JSON object');
      return updateAgent(agent.id, {
        expected_revision: base.revision,
        ...(nameEdited ? { name: name.trim() } : {}),
        ...(profileEdited ? { profile } : {}),
      });
    },
    onSuccess: (updated) => {
      setBase(updated);
      setName(updated.name);
      setJson(JSON.stringify(updated.profile ?? {}, null, 2));
      setFault(undefined);
      setJsonOpen(false);
      queryClient.setQueryData(agentKey(updated.id), updated);
      toast({ tone: 'success', title: t('agents.profile.saved', { name: updated.name }) });
    },
    // Somebody else changed it first: it is read again before the next try.
    onError: (error) => {
      if (error instanceof ApiError && error.is('conflict.stale_revision')) void queryClient.invalidateQueries({ queryKey: agentKey(agent.id), exact: true });
    },
  });

  const stale = save.error instanceof ApiError && save.error.is('conflict.stale_revision') || changed && agent.revision !== base.revision;

  const submit = () => {
    if (parsed() === null) {
      setFault(t('agents.profile.jsonInvalid'));
      return;
    }
    setFault(undefined);
    save.mutate();
  };

  const pick = (next: string) => {
    const profile = { ...(current ?? {}) };
    if (next === DEFAULT_BUILD) profile.harness_version = null;
    else profile.harness_version = next;
    setJson(JSON.stringify(profile, null, 2));
  };

  return (
    <div className="flex flex-col gap-6">
      <section id="agent-profile" aria-label={t('agents.profile.title')} className="flex min-w-0 flex-col gap-3">
        <ul aria-label={t('agents.profile.title')} className="min-w-0 divide-y divide-border rounded-lg border border-border bg-surface">
          {manage && <SettingRowLayout name={t('agents.profile.name')}
            value={<Input label={t('agents.profile.name')} hideLabel value={name} disabled={save.isPending} onChange={(event) => setName(event.target.value)} autoComplete="off" />} />}
          <SettingRowLayout name={t('agents.profile.pin')} description={t('agents.profile.pinHint')} readOnly={!manage}
            reason={manage && current === null ? t('settings.compact.finishDraft') : undefined} value={manage ? <ChoiceLocator
            label={t('agents.profile.pin')}
            hideLabel
            value={shownPin}
            disabled={save.isPending || current === null}
            onValueChange={pick}
            options={[
              { value: DEFAULT_BUILD, ...choiceOption(t, 'sourceKind.default'), description: t('agents.list.pinDefault') },
              ...builds.map((build) => ({ value: build.version, ...choiceOption(t, 'dynamic.value', { name: harnessVersionWords(t, build.version), context: t('agents.profile.pin') }), description: build.is_default ? t('agents.overview.buildDefault', { version: harnessVersionWords(t, build.version) }) : t('agents.profile.pinHint') })),
              ...(shownPin !== DEFAULT_BUILD && !builds.some((build) => build.version === shownPin) ? [{ value: shownPin, ...choiceOption(t, 'dynamic.value', { name: harnessVersionWords(t, shownPin), context: t('agents.profile.pin') }) }] : []),
            ]}
          /> : shownPin === DEFAULT_BUILD ? t('agents.list.pinDefault') : harnessVersionWords(t, shownPin)} />
          {!manage && <SettingRowLayout name={t('agents.profile.json')} readOnly value={<ValueSummary value={agent.profile} />} />}
          <li><ResourceDetails keepMounted pauseEffects={false}><ul>
          {manage && <SettingEditor name={t('agents.profile.json')} description={`${t('agents.profile.lead')} ${t('agents.profile.jsonHint')}`}
            status={jsonOpen ? undefined : current === null ? <StateLine role="alert" tone="danger" compact message={t('agents.profile.jsonInvalid')} /> : fault === undefined ? undefined : <StateLine role="alert" tone="danger" compact message={fault} />}
            label={t('settings.compact.editValue', { name: t('agents.profile.json') })} open={jsonOpen} onOpenChange={setJsonOpen}
            summary={<ValueSummary inControl value={agent.profile} />} disabled={save.isPending}><Textarea
            label={t('agents.profile.json')}
            hideLabel
            error={current === null ? t('agents.profile.jsonInvalid') : fault}
            rows={10}
            value={json}
            disabled={save.isPending}
            onChange={(event) => setJson(event.target.value)}
            className="font-mono"
            spellCheck={false}
          /></SettingEditor>}
          </ul><DescriptorOverrides key={agent.id} agent={agent} manage={manage} /></ResourceDetails></li>
        </ul>
          {manage && (
            <div className="flex flex-wrap items-center gap-3">
              <Button admin={adminSpec(agent, 'agent.manage')} variant="primary" pending={save.isPending} blocked={!changed || current === null || stale} onClick={submit}>
                {t('agents.profile.save')}
              </Button>
              <Button
                disabled={save.isPending || !changed}
                onClick={() => {
                  setName(agent.name);
                  setJson(JSON.stringify(agent.profile ?? {}, null, 2));
                  setBase(agent);
                  setFault(undefined);
                  setJsonOpen(false);
                  save.reset();
                }}
              >
                {t('agents.profile.revert')}
              </Button>
            </div>
          )}
          {stale && (
            <InlineAlert tone="warning" title={t('agents.profile.changed')}>
              <p>{t('settings.compact.changedReason')}</p>
              {manage && <SettingChanges rows={[
                { name: t('agents.profile.name'), current: agent.name, draft: name },
                { name: t('agents.profile.json'), current: agent.profile, draft: current ?? json },
              ]} disabled={save.isPending || agent.revision === base.revision} reason={current === null ? t('agents.profile.jsonInvalid') : undefined} onKeep={() => {
                if (!nameEdited) setName(agent.name);
                if (!profileEdited) setJson(JSON.stringify(agent.profile ?? {}, null, 2));
                setBase(agent);
                save.reset();
              }} />}
            </InlineAlert>
          )}
          {save.error !== null && !stale && <ErrorAlert error={save.error} />}
      </section>
    </div>
  );
}

export interface AgentScreenProps {
  agentId: string;
  tab: AgentTab;
  onTab: (tab: AgentTab) => void;
}

export function AgentScreen({ agentId, tab, onTab }: AgentScreenProps) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const query = useQuery(agentQuery(agentId));
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const declaredHarness = programNameIn(query.data, locale) ?? reportedHarnessName(query.data?.descriptor, locale);
  const harnessTitle = useHarnessTitle(query.data !== undefined && declaredHarness === null);
  const back = useResourceHeading('agents', Bot, query.data?.name ?? t('common.loading'), query.data?.id);
  const acts = useAgentActs({ onProfile: () => onTab('profile'), afterDelete: () => void navigate({ to: '/resources/agents' }) });

  const refused = query.error instanceof ApiError && [401, 403, 404].includes(query.error.status);
  if (query.data === undefined || refused) return query.isError ? <ErrorState className="h-full" error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} /> : <Skeletons />;
  const agent = query.data;
  const manage = holds(agent.access, 'agent.manage');
  const descriptor = readDescriptor(agent.descriptor);
  const { edit, primary, menu } = acts.page(agent);
  const harness = declaredHarness ?? harnessTitle(agent.harness_kind);

  return (
    <Page>
      {query.isError && <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />}
      <PageHeader
        adminAccess={agent.access.admin_access} ownership={agent} opening={{ kind: 'agent', id: agent.id }}
        back={back}
        mark={
          <PlaceMark>
            <Bot aria-hidden />
          </PlaceMark>
        }
        title={agent.name}
        // Repeat the program under the profile only when its name differs from the profile's.
        {...(harness === agent.name ? {} : { lead: harness })}
        facts={[
          agent.disabled ? <Badge key="state" tone="warning">{t('resources.disabled')}</Badge> : <Badge key="state" tone="accent">{t('agents.list.enabled')}</Badge>,
        ]}
        actions={manage ? <HeadingActs omit={tab === 'profile' ? ['edit', 'rename'] : []} admin={adminSpec(agent, 'agent.manage')} name={agent.name} primary={primary} edit={edit} menu={menu} /> : <HeadingActs name={agent.name} primary={null} edit={null} menu={menu} />}
      />
      <Tabs
        label={t('agents.tabs.label')}
        value={tab === 'parameters' && descriptor.declared && descriptor.parameters.length === 0 ? 'overview' : tab}
        onValueChange={onTab}
        tabs={[
          { value: 'overview', label: t('agents.tabs.overview'), panel: <Overview agent={agent} descriptor={descriptor} /> },
          ...(!descriptor.declared || descriptor.parameters.length > 0 ? [{ value: 'parameters' as const, label: t('agents.tabs.parameters'), count: descriptor.parameters.length, panel: <Parameters agent={agent} descriptor={descriptor} /> }] : []),
          { value: 'profile', label: t('resourceDetails.layout.settings'), panel: <>{!manage && <ReadOnlySettingsNotice owner={agent.owner} />}<ProfileForm key={agent.id} agent={agent} manage={manage} /></> },
          { value: 'access', label: t('agents.tabs.access'), panel: <ResourceAccess access={agent.access} target={{ kind: 'agent', id: agent.id, name: agent.name }} /> },
        ]}
        panelClassName="pt-4"
      />
      {acts.dialogs}
    </Page>
  );
}
