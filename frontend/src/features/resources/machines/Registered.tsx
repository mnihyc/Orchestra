/** What is registered from a machine: its agent profiles, its providers and its environments, each with where it leads, and the three acts that register one more.
 */
import { Link } from '@tanstack/react-router';
import { Plus } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { allowedRoots, readMachineFacts, registrableHarnesses } from '@/features/resources/machines/facts';
import { DetailPages, DetailSections } from '@/features/resources/DetailSections';
import { RegisterAgentDialog } from '@/features/resources/machines/RegisterAgent';
import { RegisterEnvironmentDialog } from '@/features/resources/machines/RegisterEnvironment';
import { AddProviderDialog } from '@/features/resources/providers/AddProvider';
import type { Machine } from '@/generated/api-types';
import { Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { EnvironmentDot } from '@/ui/EnvironmentDot';
import { PermissionGate } from '@/ui/PermissionGate';
import { StatusDot } from '@/ui/StatusDot';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';

type Registering = 'agent' | 'provider' | 'environment' | null;

function Entries({ label, empty, children }: { label: string; empty: string; children: ReactNode[] }) {
  if (children.length === 0) return <EmptyState compact title={empty} />;
  return (
    <DetailPages items={children}>{(page) => <ul aria-label={label} className="flex flex-col divide-y divide-border rounded-lg border border-border bg-surface">{page}</ul>}</DetailPages>
  );
}

const rowClasses = 'flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2';
const nameClasses = 'min-w-0 truncate font-medium text-text hover:underline';

export function RegisteredHere({ machine }: { machine: Machine }) {
  const { t } = useTranslation();
  const [registering, setRegistering] = useState<Registering>(null);
  const facts = readMachineFacts(machine.facts);
  const usable = machine.status === 'active' && machine.online;
  const why = machine.status === 'revoked' ? t('machines.registered.revoked') : !machine.online ? t('machines.registered.offline') : null;
  const harnesses = registrableHarnesses(facts);
  const noHarness = usable && harnesses.length === 0 ? t('machines.registered.noHarness') : null;
  const noRoot = usable && facts.policy !== null && allowedRoots(facts).length === 0 ? t('machines.registered.noRoot') : null;

  const act = (kind: Exclude<Registering, null>, label: string, reason: string | null) => (
    <PermissionGate allowed={reason === null} mode="block" {...(reason === null ? {} : { reason })}>
      <Button size="sm" icon={<Plus aria-hidden />} onClick={() => setRegistering(kind)}>
        {label}
      </Button>
    </PermissionGate>
  );

  return (
    <div className="flex flex-col gap-6">
      <DetailSections label={t('machines.tabs.registered')} sections={[{ value: 'agents', label: t('machines.registered.agents'), panel: <Section
        id="registered-agents"
        title={t('machines.registered.agents')}
        lead={t('machines.registered.agentsLead')}
        action={act('agent', t('machines.registered.registerAgent'), why ?? noHarness)}
      >
        <Entries label={t('machines.registered.agents')} empty={t('machines.registered.noAgents')}>
          {machine.agents.map((agent) => (
            <li key={agent.id} className={rowClasses}>
              <ResourceChip subject={{ kind: 'agent', agent }} asChild hint={false}>
              <Link to="/resources/agents/$id" params={{ id: agent.id }} className={nameClasses}>
                {agent.name}
              </Link>
              </ResourceChip>
              <span className="text-sm text-text-muted">{agent.harness_kind}</span>
              {agent.disabled && <Badge tone="warning">{t('resources.disabled')}</Badge>}
            </li>
          ))}
        </Entries>
      </Section> },

      { value: 'providers', label: t('machines.registered.providers'), panel: <Section
        id="registered-providers"
        title={t('machines.registered.providers')}
        lead={t('machines.registered.providersLead')}
        action={act('provider', t('machines.registered.registerProvider'), why)}
      >
        <Entries label={t('machines.registered.providers')} empty={t('machines.registered.noProviders')}>
          {machine.providers.map((provider) => (
            <li key={provider.id} className={rowClasses}>
              <ResourceChip subject={{ kind: 'provider', provider }} asChild>
              <Link to="/resources/providers/$id" params={{ id: provider.id }} className={nameClasses}>
                {provider.name}
              </Link>
              </ResourceChip>
              <span className="text-sm text-text-muted">{provider.kind === 'api' ? t('providers.kind.api') : t('providers.kind.subscription')}</span>
              <StatusDot
                tone={provider.status === 'ready' ? 'success' : provider.status === 'needs_setup' || provider.status === 'exhausted' ? 'warning' : provider.status === 'error' ? 'danger' : 'idle'}
                label={t(`providers.status.${provider.status}`)}
                showLabel
              />
            </li>
          ))}
        </Entries>
      </Section> },

      { value: 'environments', label: t('machines.registered.environments'), panel: <Section
        id="registered-environments"
        title={t('machines.registered.environments')}
        lead={t('machines.registered.environmentsLead')}
        action={act('environment', t('machines.registered.registerEnvironment'), why ?? noRoot)}
      >
        <Entries label={t('machines.registered.environments')} empty={t('machines.registered.noEnvironments')}>
          {machine.environments.map((environment) => (
            <li key={environment.id} className={rowClasses}>
              <ResourceChip subject={{ kind: 'environment', environment }} asChild>
              <Link to="/resources/environments/$id" params={{ id: environment.id }} className={nameClasses}>
                {environment.name}
              </Link>
              </ResourceChip>
              <span className="text-sm text-text-muted">{environment.address}</span>
              <EnvironmentDot status={environment.status} showLabel />
            </li>
          ))}
        </Entries>
      </Section> }]} />

      {registering === 'agent' && <RegisterAgentDialog machine={machine} open onClose={() => setRegistering(null)} />}
      {registering === 'provider' && <AddProviderDialog initialRuntimeMachine={machine.id} onClose={() => setRegistering(null)} />}
      {registering === 'environment' && <RegisterEnvironmentDialog machine={machine} open onClose={() => setRegistering(null)} />}
    </div>
  );
}
