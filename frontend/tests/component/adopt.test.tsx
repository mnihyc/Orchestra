/** The sessions a harness holds are listed with Adopt…, and adopting says where the thread would stand, the model and the history before anything happens. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { adoptSession } from '@/api/adoption';
import { ApiError } from '@/api/client';
import { HarnessSessions, harnessesOfMachine, type SessionHarness } from '@/features/resources/adopt/Sessions';
import type { AdoptPlan, AgentRef, NativeSession } from '@/generated/api-types';
import { i18n } from '@/i18n';

import { show } from './support';
import { harnessName } from '../doubles/harness-names';

const sessions: NativeSession[] = [
  { native_session_ref: 'native-a', title: 'Fix the login', cwd: '/work/web', updated_at: '2026-10-14T08:00:00.000Z', adopted_thread_id: null },
  { native_session_ref: 'native-b', title: null, cwd: null, updated_at: null, adopted_thread_id: 'thread-b' },
];

const planned: AdoptPlan = {
  session: sessions[0]!,
  name: 'Fix the login',
  environment: { id: 'env-work', name: 'Work', address: 'acme.ada.work' } as AdoptPlan['environment'],
  directory: 'web',
  register_first: null,
  session_model: 'gpt-elsewhere',
  model: null,
  model_reason: 'not_offered',
  turns: 3,
  history_missing: null,
  adopted_thread_id: null,
};

let listed: () => Promise<NativeSession[]> = () => Promise.resolve(sessions);
let plan: () => Promise<AdoptPlan> = () => Promise.resolve(planned);

vi.mock('@/api/adoption', () => ({
  harnessSessionsQuery: (provider: string, agent: string) => ({ queryKey: ['harness-sessions', provider, agent], queryFn: () => listed(), retry: false }),
  adoptionPlanQuery: (topic: string) => ({ queryKey: ['adoption-plan', topic], queryFn: () => plan(), retry: false }),
  adoptSession: vi.fn(),
}));
vi.mock('@/features/settings/PlacePicker', () => ({
  PlacePicker: ({ label, onChange }: { label: string; onChange: (place: unknown) => void }) => (
    <button type="button" onClick={() => onChange({ id: 'topic-login', kind: 'topic', color: null, folder: false, path: 'Acme / Web / Login' })}>
      {label}
    </button>
  ),
}));
vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, to, 'aria-label': label }: { children: React.ReactNode; to: string; 'aria-label'?: string }) => <a href={to} aria-label={label}>{children}</a>,
  useNavigate: () => vi.fn(),
}));

afterEach(() => {
  listed = () => Promise.resolve(sessions);
  plan = () => Promise.resolve(planned);
  vi.clearAllMocks();
});

const codex: AgentRef = { id: 'agent-codex', name: 'Codex', harness_kind: 'codex', harness: harnessName('codex'), address: 'acme.codex', disabled: false, machine: null };
const harness: SessionHarness = { providerId: 'provider-laptop', providerName: "Ada's Codex", agent: codex };

function draw(harnesses: readonly SessionHarness[] = [harness]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  show(
    <QueryClientProvider client={client}>
      <HarnessSessions harnesses={harnesses} />
    </QueryClientProvider>,
  );
}

it('lists each session with its name or first words, its folder, and the thread of one adopted already', async () => {
  draw();
  const list = await screen.findByRole('list', { name: i18n.t('adopt.harness', { agent: 'Codex', provider: "Ada's Codex" }) });
  expect(within(list).getByText('Fix the login')).toBeInTheDocument();
  const folder = within(list).getByText('work/web');
  expect(folder).toHaveAttribute('tabindex', '0');
  await userEvent.tab();
  expect(folder).toHaveFocus();
  expect(await screen.findByRole('tooltip')).toHaveTextContent('/work/web');
  expect(folder).toHaveAccessibleDescription('/work/web');
  expect(within(list).getByRole('button', { name: i18n.t('resourceDetails.copyPath') })).toBeInTheDocument();
  expect(within(list).getByText(i18n.t('adopt.untitled'))).toBeInTheDocument();
  expect(within(list).getByText(i18n.t('adopt.noFolder'))).toBeInTheDocument();
  expect(within(list).getByRole('link', { name: i18n.t('craftPlaces.openThread.named', { name: i18n.t('adopt.untitled') }) })).toHaveAttribute('href', '/thread/thread-b');
  expect(within(list).getAllByRole('button', { name: i18n.t('adopt.adoptNamed', { name: 'Fix the login' }) })).toHaveLength(1);
  expect(within(list).getAllByRole('button')).toHaveLength(2);
});

it('says what adopting would do once a topic is chosen, and adopts into that topic only when asked', async () => {
  vi.mocked(adoptSession).mockResolvedValue({ id: 'thread-new', name: 'Fix the login' } as Awaited<ReturnType<typeof adoptSession>>);
  draw();
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: i18n.t('adopt.adoptNamed', { name: 'Fix the login' }) }));
  const dialog = await screen.findByRole('dialog');
  const confirm = within(dialog).getByRole('button', { name: i18n.t('adopt.dialog.confirm') });
  expect(confirm).toBeDisabled();
  await user.click(within(dialog).getByRole('button', { name: i18n.t('adopt.dialog.topic') }));
  expect(await within(dialog).findByText(i18n.t('adopt.dialog.inEnvironment', { environment: 'Work', directory: 'web' }))).toBeInTheDocument();
  expect(within(dialog).getByText(i18n.t('adopt.dialog.modelReason.not_offered', { model: 'gpt-elsewhere' }))).toBeInTheDocument();
  expect(within(dialog).getByText(i18n.t('adopt.dialog.historyTurns', { count: 3 }))).toBeInTheDocument();
  expect(adoptSession).not.toHaveBeenCalled();
  await waitFor(() => expect(confirm).toBeEnabled());
  await user.click(confirm);
  await waitFor(() => expect(adoptSession).toHaveBeenCalledTimes(1));
  expect(vi.mocked(adoptSession).mock.calls[0]![0]).toBe('topic-login');
  expect(vi.mocked(adoptSession).mock.calls[0]![1]).toEqual({ provider_id: 'provider-laptop', agent_id: 'agent-codex', native_session_ref: 'native-a' });
});

it('names the folder to register first and adopts nothing while none of the person’s environments holds it', async () => {
  plan = () => Promise.resolve({ ...planned, environment: null, directory: null, register_first: '/elsewhere/api' });
  draw();
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: i18n.t('adopt.adoptNamed', { name: 'Fix the login' }) }));
  const dialog = await screen.findByRole('dialog');
  await user.click(within(dialog).getByRole('button', { name: i18n.t('adopt.dialog.topic') }));
  expect(await within(dialog).findByText(i18n.t('adopt.dialog.registerFirst', { folder: '/elsewhere/api' }))).toBeInTheDocument();
  expect(within(dialog).getByText(i18n.t('adopt.dialog.registerHow'))).toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: i18n.t('adopt.dialog.confirm') })).toBeDisabled();
});

it('says so when a harness cannot list what it holds', async () => {
  listed = () =>
    Promise.reject(new ApiError({ type: 'orch:error:capability.unsupported', code: 'capability.unsupported', title: 'Unsupported', detail: 'no', status: 409, request_id: '' }, null));
  draw();
  expect(await screen.findByText(i18n.t('adopt.cannotList'))).toBeInTheDocument();
});

it('pairs each provider of a machine with the profiles of the same harness there', () => {
  const claude: AgentRef = { ...codex, id: 'agent-claude', name: 'Claude Code', harness_kind: 'claude-code' };
  const pairs = harnessesOfMachine({
    providers: [{ id: 'provider-laptop', name: "Ada's Codex", harness_kind: 'codex' } as never, { id: 'provider-key', name: 'A key', harness_kind: null } as never],
    agents: [codex, claude],
  });
  expect(pairs.map((pair) => `${pair.providerId}/${pair.agent.id}`)).toEqual(['provider-laptop/agent-codex']);
});
