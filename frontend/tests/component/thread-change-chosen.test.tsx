/** A switch asked for from the model list opens with the provider, the agent and the model that the list named already chosen. */
import { screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as ForksApi from '@/api/forks';
import type * as NodesApi from '@/api/nodes';
import type * as ResourcesApi from '@/api/resources';
import type { Choices } from '@/features/change/choices';
import { ThreadChange } from '@/features/change/ThreadChange';
import type { Agent, ForkPlan, ForkRequest, Provider, ProviderModel, Thread } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { show } from './support';

const state = vi.hoisted(() => ({ node: null as Thread | null, providers: [] as Provider[], agents: [] as Agent[], models: new Map<string, ProviderModel[]>(), read: vi.fn<(request: ForkRequest) => Promise<ForkPlan>>() }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), useNavigate: () => vi.fn() }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodesApi>(), nodeQuery: () => ({ queryKey: ['chosen-node'], initialData: state.node, queryFn: () => Promise.resolve(state.node) }) }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourcesApi>(),
  providersQuery: () => ({ queryKey: ['chosen-providers'], queryFn: () => Promise.resolve(state.providers) }),
  agentsQuery: () => ({ queryKey: ['chosen-agents'], queryFn: () => Promise.resolve(state.agents) }),
  providerModelsQuery: (id: string) => ({ queryKey: ['chosen-models', id], queryFn: () => Promise.resolve(state.models.get(id) ?? []) }) }));
vi.mock('@/api/forks', async (original) => ({ ...await original<typeof ForksApi>(), planFork: (_id: string, request: ForkRequest) => state.read(request) }));
// The wizard's own choices are what is read: the picker shows them as words.
vi.mock('@/features/change/RunsOn', async (original) => ({ ...await original<object>(),
  RunsOn: ({ choices }: { choices: Choices }) => <output aria-label="Runs on">{JSON.stringify({ provider: choices.providerId, agent: choices.agentId, model: choices.model })}</output> }));
vi.mock('@/features/change/Where', () => ({ Where: () => null }));
vi.mock('@/features/change/plan', async (original) => ({ ...await original<object>(), changesOf: () => [], staysOf: () => [], endsOf: () => [], targetModelName: () => 'Shared model' }));
vi.mock('@/features/change/Time', () => ({ Time: () => null }));
vi.mock('@/features/change/Depends', () => ({ Depends: () => null }));

const provider = (id: string): Provider => ({ id, name: `Provider ${id}`, status: 'ready', protocols: ['openai-responses'], agent: null, home: null, owner: null, org_id: null }) as unknown as Provider;
const agent = (id: string): Agent => ({ id, name: `Agent ${id}`, disabled: false, descriptor: { kind: 'scripted' } }) as unknown as Agent;
const model = (name: string): ProviderModel => ({ model_name: name, display_name: name, enabled: true, parameters: {} }) as unknown as ProviderModel;
const plan = { result: 'replace', source: { name: 'Login' }, from: { provider: { id: 'current', name: 'Current' } },
  to: { provider: { id: 'next', name: 'Next' }, agent: { id: 'agent2' }, model: 'b-routing', effort: null, environment: { id: 'environment', name: 'Workspace' } },
  expires_at: new Date(Date.now() + 60_000).toISOString(), preflight_id: 'review', locks: [], blockers: [], depends: [], turn_running: false, mounts: [], mcp: [],
  options: { active_turn: 'wait', wait_minutes: 10 }, history: { report: { fidelity: 'lossless', by_kind: {}, items_total: 0, dropped: [], warnings: [] } } } as unknown as ForkPlan;

/** What the wizard holds as chosen, once it has read the lists. */
const held = () => JSON.parse(screen.getByLabelText('Runs on').textContent) as { provider: string | null; agent: string | null; model: string | null };

beforeEach(() => {
  vi.clearAllMocks();
  state.node = { id: 'thread', kind: 'thread', name: 'Login', model: 'a-routing', model_display: 'A', effort: null, state: 'idle', side_of: null, blocked: null, path: [], owner: { id: 'owner' },
    binding: { provider: { id: 'current' }, agent: { id: 'agent' } } } as unknown as Thread;
  state.providers = [provider('current'), provider('next')];
  state.agents = [agent('agent'), agent('agent2')];
  state.models = new Map([['current', [model('a-routing'), model('c-routing')]], ['next', [model('a-routing'), model('b-routing')]]]);
  state.read.mockImplementation(() => Promise.resolve(plan));
});
afterEach(async () => { await setLocale('en'); });

it('chooses the provider, the agent and the model that the list named once the lists are read', async () => {
  show(<ThreadChange threadId="thread" kind="switch" toTurn={null} focus="provider" chosen={{ provider: 'next', agent: 'agent2', model: 'b-routing' }} job={null} say={null} open onClose={() => undefined} />);
  await waitFor(() => expect(held()).toEqual({ provider: 'next', agent: 'agent2', model: 'b-routing' }));
});

it('chooses the agent alone, with the model the thread has, when the list named only the agent', async () => {
  show(<ThreadChange threadId="thread" kind="switch" toTurn={null} focus="provider" chosen={{ provider: null, agent: 'agent2', model: null }} job={null} say={null} open onClose={() => undefined} />);
  await waitFor(() => expect(held()).toEqual({ provider: null, agent: 'agent2', model: 'a-routing' }));
});

it('takes the first model of another provider when the one named is not among its models, and keeps the agent the thread has where the list named none', async () => {
  show(<ThreadChange threadId="thread" kind="switch" toTurn={null} focus="provider" chosen={{ provider: 'next', agent: null, model: 'missing-routing' }} job={null} say={null} open onClose={() => undefined} />);
  // The thread's own model is on the other provider too, so it is kept before the first listed.
  await waitFor(() => expect(held()).toEqual({ provider: 'next', agent: null, model: 'a-routing' }));
});

it('chooses nothing where the list named nothing', async () => {
  show(<ThreadChange threadId="thread" kind="switch" toTurn={null} focus="provider" chosen={{ provider: null, agent: null, model: null }} job={null} say={null} open onClose={() => undefined} />);
  await screen.findByLabelText('Runs on');
  expect(held()).toEqual({ provider: null, agent: null, model: null });
});
