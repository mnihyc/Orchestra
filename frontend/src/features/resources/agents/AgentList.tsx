/** The agent profiles a person may use or manage: name, harness, version pin, where each is usable (any managed runtime, or one machine) and whether it is switched on, with the means to add one.
 */
import { useQuery } from '@tanstack/react-query';
import { Link, useNavigate } from '@tanstack/react-router';
import { Bot, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { harnessKindsQuery } from '@/api/resources';
import { resourceReaders } from '@/features/resources/inventory';
import { useKnownPlaces } from '@/features/navigation/placeLabels';
import { resourceLabels } from '@/features/resources/labels';
import { AddAgentDialog } from '@/features/resources/agents/AddAgent';
import { useAgentActs } from '@/features/resources/agents/acts';
import { versionPin } from '@/features/resources/agents/descriptor';
import { useHarnessTitle } from '@/features/resources/harnesses';
import { ResourceList, useSearchText } from '@/features/resources/ResourceList';
import { oneOf } from '@/features/resources/search';
import type { Agent } from '@/generated/api-types';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import type { Column } from '@/ui/Table';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';

export type AgentState = 'enabled' | 'disabled';

export const AGENT_STATES: readonly AgentState[] = ['enabled', 'disabled'];

/** A registered adapter may itself be named `any`; this is only the local clear choice. */
const ANY_HARNESS = '\u0000any';

export interface AgentsSearch {
  q?: string;
  state?: AgentState;
  harness?: string;
}

export function AgentLink({ agent, label = agent.name }: { agent: Pick<Agent, 'id' | 'name'> & Partial<Agent>; label?: string }) {
  return (
    <ResourceChip subject={{ kind: 'agent', agent }} asChild hint={false}><Link to="/resources/agents/$id" params={{ id: agent.id }} className="truncate font-medium text-text hover:underline">
      {label}
    </Link></ResourceChip>
  );
}

export function AgentList({ search, onSearch }: { search: AgentsSearch; onSearch: (patch: Partial<AgentsSearch>) => void }) {
  const { t } = useTranslation();
  const knownHarnesses = useQuery(harnessKindsQuery);
  const harnessTitle = useHarnessTitle();
  const places = useKnownPlaces({ trees: false });
  const navigate = useNavigate();
  const acts = useAgentActs({ onProfile: (agent) => void navigate({ to: '/resources/agents/$id', params: { id: agent.id }, search: { tab: 'profile' } }) });
  const [adding, setAdding] = useState(false);
  const [text, setText] = useSearchText(search.q ?? '', (q) => onSearch({ q: q === '' ? undefined : q }));
  const harnesses = (knownHarnesses.data ?? []).map((harness) => harness.kind).sort();
  const retainedHarness = search.harness !== undefined && !harnesses.includes(search.harness) ? search.harness : null;
  const registryReady = knownHarnesses.isSuccess && !knownHarnesses.isFetching;

  const columns = (rows: readonly Agent[]): Column<Agent>[] => {
    const labels = resourceLabels(rows, places);
    return [
    {
      id: 'name',
      header: t('agents.list.name'),
      cell: (agent, shown) => {
        const withoutHarness = !shown.has('harness');
        const withoutState = !shown.has('state');
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <AgentLink agent={agent} label={labels.get(agent.id) ?? agent.name} />
            {withoutHarness && agent.name !== harnessTitle(agent.harness_kind) && <span className="text-sm text-text-muted">{harnessTitle(agent.harness_kind)}</span>}
            {withoutState && agent.disabled && (
              <Badge tone="warning" className="self-start">
                {t('resources.disabled')}
              </Badge>
            )}
          </div>
        );
      },
    },
    { id: 'harness', header: t('agents.list.harness'), from: 'sm', cell: (agent) => harnessTitle(agent.harness_kind) },
    {
      id: 'pin',
      header: t('agents.list.pin'),
      from: 'md',
      className: 'text-text-muted',
      cell: (agent) => versionPin(agent.profile) ?? t('agents.list.pinDefault'),
    },
    {
      id: 'usable',
      header: t('agents.list.usable'),
      from: 'md',
      className: 'text-text-muted',
      cell: (agent) => agent.machine?.name ?? t('agents.list.anyRuntime'),
    },
    {
      id: 'state',
      header: t('agents.list.state'),
      from: 'sm',
      cell: (agent) => (agent.disabled ? <Badge tone="warning">{t('resources.disabled')}</Badge> : <Badge tone="accent">{t('agents.list.enabled')}</Badge>),
    },
    ];
  };

  const add = (
    <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setAdding(true)}>
      {t('agents.list.add')}
    </Button>
  );

  return (
    <>
      <ResourceList
        kind="agents"
        search={{ ...search }}
        acts={acts.list}
        title={t('screens.agents.title')}
        icon={Bot}
        lead={t('agents.list.lead')}
        label={t('agents.list.label')}
        readRow={resourceReaders.agents}
        filter={{ state: search.state, harness: search.harness }}
        columns={columns}
        rowKey={(agent) => agent.id}
        rowClassName={(agent) => (agent.disabled ? 'bg-bg-subtle' : undefined)}
        text={text}
        onText={setText}
        searchLabel={t('agents.list.search')}
        searchPlaceholder={t('agents.list.searchPlaceholder')}
        filters={
          <>
            <SegmentedControl
              label={t('agents.list.filter')}
              value={search.state ?? 'all'}
              onValueChange={(next) => onSearch({ state: oneOf(AGENT_STATES, next) })}
              segments={[
                { value: 'all', ...choiceOption(t, 'agentState.all') },
                { value: 'enabled', ...choiceOption(t, 'agentState.enabled') },
                { value: 'disabled', ...choiceOption(t, 'agentState.disabled') },
              ]}
            />
            {(harnesses.length > 1 || search.harness !== undefined) && (
              <ChoiceLocator
                label={t('agents.list.harnessFilter')}
                size="sm"
                value={search.harness ?? ANY_HARNESS}
                hint={knownHarnesses.isPending || knownHarnesses.isFetching ? t('common.loading') : undefined}
                onValueChange={(next) => onSearch({ harness: next === ANY_HARNESS ? undefined : next })}
                options={[{ value: ANY_HARNESS, ...choiceOption(t, 'statusFilter.any'), description: t('agents.list.anyHarness') }, ...harnesses.map((kind) => ({ value: kind, ...choiceOption(t, 'dynamic.harness', { name: harnessTitle(kind) }) })),
                  ...(retainedHarness === null ? [] : [{ value: retainedHarness, ...choiceOption(t, 'dynamic.harness', { name: harnessTitle(retainedHarness) }),
                    ...(registryReady ? { description: t('agents.overview.notReported'), disabled: true } : {}) }])
                ]}
                fieldClassName="w-auto"
              />
            )}
            {knownHarnesses.isError && <ErrorState compact error={knownHarnesses.error} retrying={knownHarnesses.isRefetching} onRetry={() => void knownHarnesses.refetch()} />}
          </>
        }
        action={add}
        empty={{ icon: <Bot aria-hidden />, title: t('agents.list.none'), description: t('agents.list.noneLead'), action: add }}
        noMatch={t('agents.list.noMatch')}
      />
      {adding && <AddAgentDialog onClose={() => setAdding(false)} />}
    </>
  );
}
