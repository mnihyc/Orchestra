/** Mount, MCP, template, profile and skill facts use the same bounded inspection card and one native detail read. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { api, ok } from '@/api/client';
import { Facts, type Fact } from '@/features/resources/Facts';
import { PathValue } from '@/features/resources/TechnicalValue';
import { record } from '@/features/resources/UnitCommands';
import { reportedHarnessName } from '@/features/resources/agents/descriptor';
import { programNameIn } from '@/features/resources/programNames';
import { imageSource } from '@/features/resources/templates/spec';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { ErrorAlert } from '@/ui/ErrorState';
import { Button } from '@/ui/Button';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusDot } from '@/ui/StatusDot';
import { ResourceChip } from './ResourceFacts';
import type { ResourceSubject } from './facts';

type Subject = Extract<ResourceSubject, { kind: 'mount' | 'mcp_server' | 'template' | 'agent' | 'skill' }>;
const HEAD = { mount: 'mount', mcp_server: 'mcp-server', template: 'template', agent: 'agent', skill: 'skill' };

function identity(subject: Subject) {
  switch (subject.kind) {
    case 'mount': return subject.mount;
    case 'mcp_server': return subject.server;
    case 'template': return subject.template;
    case 'agent': return subject.agent;
    case 'skill': return subject.skill;
  }
}

export default function ExtraResourceCard({ subject }: { subject: Subject }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const supplied = identity(subject);
  // Pinning transfers the body between the hover and kept layers. Let this one read finish
  // into the ordinary cache during that transfer rather than aborting and issuing it again.
  const query = useQuery<Record<string, unknown>>({ queryKey: [HEAD[subject.kind], supplied.id], queryFn: async () => {
    const params = { path: { id: supplied.id } };
    switch (subject.kind) {
      case 'mount': return { ...await ok(api.GET('/mounts/{id}', { params })) };
      case 'mcp_server': return { ...await ok(api.GET('/mcp-servers/{id}', { params })) };
      case 'template': return { ...await ok(api.GET('/templates/{id}', { params })) };
      case 'agent': return { ...await ok(api.GET('/agents/{id}', { params })) };
      case 'skill': return { ...await ok(api.GET('/skills/{id}', { params })) };
    }
  } });
  const count = (value: number | null | undefined) => value === null || value === undefined ? t('usagePerf.dash') : formatNumber(value, locale);
  let rows: Fact[] = [];
  switch (subject.kind) {
    case 'mount': {
      const mount = { ...subject.mount, ...query.data };
      rows = [
        { name: t('resourceFacts.status'), value: <StatusDot tone={mount.enabled === false ? 'off' : 'success'} label={t(mount.enabled === false ? 'resources.disabled' : 'resourceFacts.capState.ok')} showLabel /> },
        { name: t('resourceFacts.locations'), value: mount.locations === undefined ? t('usagePerf.dash') : <div className="flex min-w-0 flex-col gap-2">{mount.locations.map((location) => <div key={location.machine.id} className="flex min-w-0 flex-col gap-0.5"><span>{location.machine.name}</span><span className="text-text-muted">{t(location.kind === 'volume' ? 'resourceDetails.mount.volume' : 'resourceDetails.mount.path')}</span>{location.source !== null && <PathValue path={location.source} />}{location.storage !== null && <span>{location.storage}</span>}{location.measured_bytes !== undefined && location.measured_bytes !== null && <span>{formatBytes(location.measured_bytes, locale)}</span>}</div>)}</div> },
        { name: t('resourceFacts.attaching'), value: count(mount.attached_environments) },
        { name: t('resourceFacts.defaultFate'), value: mount.default_fate === undefined ? t('usagePerf.dash') : t(`resourceFacts.fate.${mount.default_fate}`) },
      ]; break;
    }
    case 'mcp_server': {
      const server = { ...subject.server, ...query.data };
      const spec = record(server.spec);
      const transport = typeof spec.transport === 'string' ? spec.transport : typeof spec.type === 'string' ? spec.type : null;
      const machines = server.offering_machines ?? (server.machine === null || server.machine === undefined ? [] : [server.machine]);
      rows = [
        { name: t('resourceFacts.transport'), value: transport ?? t('usagePerf.dash') },
        { name: t('resourceFacts.machines'), value: machines.length === 0 ? t('resourceFacts.noMachine') : machines.map((machine) => machine.name).join(t('resourceFacts.separator')) },
        { name: t('resourceFacts.enabledAt'), value: count(server.enabled_places) },
        { name: t('resourceFacts.status'), value: <StatusDot tone={subject.state === 'failed' || subject.state === 'unavailable' ? 'warning' : server.enabled === false || subject.state === 'stopped' ? 'off' : 'success'} label={subject.state === undefined ? t(server.enabled === false ? 'resources.disabled' : 'resourceFacts.capState.ok') : t(`mcpServers.states.${subject.state}`)} showLabel /> },
      ]; break;
    }
    case 'template': {
      const template = { ...subject.template, ...query.data };
      const source = imageSource(template.spec);
      const digest = record(template.spec).image_digest;
      rows = [
        { name: t('resourceFacts.kind'), value: template.kind === undefined ? t('usagePerf.dash') : t(`templates.kind.${template.kind}`) },
        { name: t('resourceFacts.image'), value: source.kind === 'image' ? source.reference : t(`templates.source.${source.kind === 'none' ? 'none' : source.kind === 'build' ? 'build' : 'archive'}`) },
        ...(source.kind === 'image' && typeof digest === 'string' && digest !== '' ? [{ name: t('resourceFacts.imageDigest'), value: digest }] : []),
        { name: t('resourceFacts.usedByLabel'), value: count(template.units) },
      ]; break;
    }
    case 'agent': {
      const agent = { ...subject.agent, ...query.data };
      const harness = programNameIn(agent, locale) ?? reportedHarnessName(agent.descriptor, locale);
      const providers = subject.provider === undefined ? agent.providers ?? [] : [subject.provider];
      const models = subject.models ?? agent.models ?? [];
      rows = [
        { name: t('resourceFacts.program'), value: harness ?? (agent.harness_kind === undefined ? t('usagePerf.dash') : t('resources.harnessNameUnknown')) },
        { name: t('resourceFacts.provider'), value: providers.length === 0 ? t('usagePerf.dash') : <div className="flex flex-wrap gap-1">{providers.map((provider) => <ResourceChip key={provider.id} subject={{ kind: 'provider', provider }} hint={false} />)}</div> },
        { name: t('resourceFacts.models'), value: models.length === 0 ? t('usagePerf.dash') : <div className="flex flex-wrap gap-1">{models.map((model) => <ResourceChip key={`${model.provider_id ?? providers[0]?.id}:${model.model_name}`} subject={{ kind: 'model', model, providerId: model.provider_id ?? providers[0]?.id ?? '' }} hint={false} />)}</div> },
      ]; break;
    }
    case 'skill': {
      const skill = { ...subject.skill, ...query.data };
      const origin = skill.latest?.origin ?? skill.origin;
      rows = [
        { name: t('resourceFacts.versionLabel'), value: skill.latest === null || skill.latest === undefined ? t('resourceFacts.noVersion') : count(skill.latest.number) },
        { name: t('resourceFacts.source'), value: origin === null || origin === undefined ? t('usagePerf.dash') : <span>{t(`resourceFacts.origin.${origin.kind}`)}{' · '}{origin.kind === 'git' ? origin.repository : origin.kind === 'url' ? origin.url : origin.kind === 'environment' ? origin.path : origin.kind === 'upload' ? origin.name : ''}</span> },
        { name: t('resourceFacts.enabledAt'), value: count(skill.enabled_places) },
      ]; break;
    }
  }
  const known = query.data !== undefined || 'access' in supplied;
  return <>{known ? <Facts compact rows={rows} /> : query.isError ? null : <SkeletonRows rows={3} />}{query.isError && <ErrorAlert error={query.error} action={<Button size="sm" onClick={() => void query.refetch()}>{t('common.tryAgain')}</Button>} />}</>;
}
