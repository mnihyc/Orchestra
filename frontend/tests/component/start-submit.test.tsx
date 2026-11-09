/** One atomic request keeps the named setup and cannot change a place default. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import type * as AccessApi from '@/api/access';
import { queryClient } from '@/api/query';
import type { StartOptions } from '@/api/start';
import { NO_PICKS, resolve } from '@/features/start/choices';
import { useLastSetup } from '@/features/start/lastSetup';
import { useStart, type StartPlan } from '@/features/start/start';
import { bindingReason, runsLine } from '@/features/start/words';
import type { CreateThread, NewEnvironment } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { agent, arrangement, options, provider } from '../unit/start-fixtures';
import { CATALOGUE } from '../unit/general-access-fixtures';
import { show } from './support';

const io = vi.hoisted(() => ({
  environment: vi.fn<(body: NewEnvironment, key: string) => Promise<{ id: string; name: string }>>(),
  thread: vi.fn<(topic: string, body: CreateThread, key: string) => Promise<{ id: string; topic_id: string; environment: { id: string; name: string } }>>(),
  topic: vi.fn<(parent: string) => Promise<{ id: string; name: string }>>(),
  topicThread: vi.fn<(parent: string, body: CreateThread, key: string) => Promise<{ id: string; topic_id: string; environment: { id: string; name: string } }>>(),
  offered: null as StartOptions | null, reads: vi.fn<(node: string, target: string | null) => void>(),
  defaultEnvironment: vi.fn(), defaultSetting: vi.fn(), navigate: vi.fn(),
  readPage: vi.fn<typeof AccessApi.readPage>(),
}));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), useNavigate: () => io.navigate }));
vi.mock('@/api/environments', async (original) => ({ ...await original<object>(), createEnvironment: io.environment }));
vi.mock('@/api/threads', async (original) => ({ ...await original<object>(), createThread: io.thread }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), createTopic: io.topic, createTopicAndThread: io.topicThread }));
vi.mock('@/api/access', async (original) => ({ ...await original<object>(), permissionsQuery: {
  queryKey: ['start-submit-permissions'], queryFn: () => Promise.resolve(CATALOGUE.reply),
}, readPage: io.readPage }));
vi.mock('@/api/start', async (original) => ({ ...await original<object>(), setDefaultEnvironment: io.defaultEnvironment, setStartDefault: io.defaultSetting,
  startOptionsQuery: (node: string, target: string | null) => ({ queryKey: ['submit-preview', node, target], queryFn: (): Promise<StartOptions> => {
    io.reads(node, target); return io.offered === null ? Promise.reject(new Error('No preview')) : Promise.resolve(io.offered);
  } }),
}));

function plan(offered = options({ environments: [], environment_default: 'fresh' })): StartPlan {
  return { place: { id: 'topic', name: 'Login', kind: 'topic', parentId: null, archived: false, folder: false, status: 'open' },
    options: offered, resolved: resolve(offered, NO_PICKS, { suggest: true }), name: '', directory: '', privately: false, message: 'Only this start' };
}

function Starter({ selected }: { selected: StartPlan }) {
  const make = useStart(() => undefined);
  const last = useLastSetup();
  return <><p>{runsLine(selected.resolved, i18n.t)}</p><button disabled={make.pending} onClick={() => make.start(selected)}>Start this setup</button>
    <output aria-label="Last started topic">{last?.place ?? ''}</output>
    {make.error !== null && <p role="alert">{bindingReason(make.error, i18n.t) ?? 'The start was refused.'}</p>}
    {make.standing.environment !== null && <p>{i18n.t('start.made.environment', { name: make.standing.environment.name })}</p>}</>;
}

it.each(['topic', 'project'] as const)('submits the confirmed agent areas with the audience and first input atomically (%s)', async (kind) => {
  const selected = { ...plan(), agentAccess: { where: 'topic' as const, parts: { documents: 'edit' as const } } };
  selected.place = { ...selected.place, kind };
  show(<Starter selected={selected} />);
  await userEvent.click(screen.getByRole('button', { name: 'Start this setup' }));
  await waitFor(() => expect(io.navigate).toHaveBeenCalledOnce());
  const request = (kind === 'topic' ? io.thread : io.topicThread).mock.calls[0]?.[1];
  expect(request).toMatchObject({ agent_access: { where: 'topic', parts: { documents: 'edit' } },
    first_input: { content: [{ type: 'text', text: selected.message }] } });
  expect(io.defaultSetting).not.toHaveBeenCalled();
  expect(io.defaultEnvironment).not.toHaveBeenCalled();
});

it('binds a first thread’s explicit audience to its new topic, preserving later topic-role cuts', async () => {
  const selected = plan(); selected.place = { ...selected.place, id: 'project', kind: 'project' }; selected.generalAccess = 'interact';
  show(<Starter selected={selected} />);
  await userEvent.click(screen.getByRole('button', { name: 'Start this setup' }));
  await waitFor(() => expect(io.topicThread).toHaveBeenCalledOnce());
  const access = io.topicThread.mock.calls[0]?.[1].access;
  if (typeof access !== 'object' || access === null || 'as' in access) throw new Error('The explicit audience uses the normal permission page change');
  const holders = access.rows?.flatMap((row) => row.add?.map((entry) => entry.holder) ?? []) ?? [];
  expect(holders.length).toBeGreaterThan(0);
  expect(holders).toEqual(holders.map(() => ({ kind: 'role', role: 'viewer', at: null })));
  expect(io.topicThread.mock.calls[0]?.[1].first_input).toMatchObject({ content: [{ type: 'text', text: selected.message }] });
});

beforeEach(() => {
  vi.clearAllMocks(); localStorage.clear(); queryClient.clear();
  client.session.setState({ status: 'signed_in', userId: 'starter' });
  io.environment.mockResolvedValue({ id: 'made', name: 'Main' });
  io.offered = options({ environments: [], environment_default: 'fresh' });
  io.thread.mockResolvedValue({ id: 'thread', topic_id: 'topic', environment: { id: 'made', name: 'Main' } });
  io.topicThread.mockResolvedValue({ id: 'thread', topic_id: 'new-topic', environment: { id: 'made', name: 'Main' } });
  io.topic.mockResolvedValue({ id: 'topic', name: 'Login' });
  io.readPage.mockRejectedValue(new Error('Creation must not read or change a permission page after birth'));
});
afterEach(async () => { act(() => client.session.setState({ status: 'signed_out', userId: null })); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  for (const kind of ['topic', 'project'] as const) for (const fresh of [false, true]) for (const viewers of [false, true]) {
    it(`admits the exact environment reader choice with the first input (${locale}/${kind}/${fresh}/${viewers})`, async () => {
      await setLocale(locale);
      const selected = plan(fresh ? options({ environments: [], environment_default: 'fresh' }) : options());
      selected.place = { ...selected.place, kind };
      selected.letIntoEnvironment = viewers;
      show(<Starter selected={selected} />);
      await userEvent.click(screen.getByRole('button', { name: 'Start this setup' }));
      await waitFor(() => expect(io.navigate).toHaveBeenCalledOnce());
      const create = kind === 'topic' ? io.thread : io.topicThread;
      expect(create).toHaveBeenCalledOnce();
      expect(create.mock.calls[0]?.[1]).toMatchObject({
        let_into_environment: viewers,
        environment: fresh ? { new: {} } : { use: 'e1' },
        first_input: { content: [{ type: 'text', text: selected.message }], client_id: expect.any(String) },
      });
      expect(io.readPage).not.toHaveBeenCalled();
      expect(io.environment).not.toHaveBeenCalled();
      expect(io.topic).not.toHaveBeenCalled();
    });
  }

  it(`leaves an omitted API reader choice absent (${locale})`, async () => {
    await setLocale(locale);
    show(<Starter selected={plan()} />);
    await userEvent.click(screen.getByRole('button', { name: 'Start this setup' }));
    await waitFor(() => expect(io.navigate).toHaveBeenCalledOnce());
    expect(io.thread.mock.calls[0]?.[1]).not.toHaveProperty('let_into_environment');
    expect(io.readPage).not.toHaveBeenCalled();
  });

  it(`keeps a refused reader gift inside the same repeatable birth (${locale})`, async () => {
    await setLocale(locale);
    const selected = { ...plan(), letIntoEnvironment: true };
    io.thread.mockRejectedValueOnce(new Error('The native reader gift was refused'));
    show(<Starter selected={selected} />);
    const start = screen.getByRole('button', { name: 'Start this setup' });
    await userEvent.click(start);
    expect(await screen.findByRole('alert')).toBeVisible();
    expect(screen.queryByText(i18n.t('start.made.environment', { name: 'Main' }))).toBeNull();
    expect(io.navigate).not.toHaveBeenCalled();
    expect(io.readPage).not.toHaveBeenCalled();
    await waitFor(() => expect(start).toBeEnabled());
    await userEvent.click(start);
    await waitFor(() => expect(io.navigate).toHaveBeenCalledOnce());
    expect(io.thread).toHaveBeenCalledTimes(2);
    const first = io.thread.mock.calls[0], retry = io.thread.mock.calls[1];
    if (first === undefined || retry === undefined) throw new Error('Both explicit birth attempts are required');
    expect(first[1]).toHaveProperty('let_into_environment', true);
    expect(retry).toEqual(first);
    expect(io.readPage).not.toHaveBeenCalled();
  });

  it(`does not reuse an accepted birth for a different reader choice (${locale})`, async () => {
    await setLocale(locale);
    const selected = { ...plan(), letIntoEnvironment: true };
    const view = show(<Starter selected={selected} />);
    await userEvent.click(screen.getByRole('button', { name: 'Start this setup' }));
    await waitFor(() => expect(io.navigate).toHaveBeenCalledOnce());
    view.rerender(<Starter selected={{ ...selected, letIntoEnvironment: false }} />);
    const start = screen.getByRole('button', { name: 'Start this setup' });
    await waitFor(() => expect(start).toBeEnabled());
    await userEvent.click(start);
    await waitFor(() => expect(io.navigate).toHaveBeenCalledTimes(2));
    expect(io.thread).toHaveBeenCalledTimes(2);
    expect(io.thread.mock.calls[1]?.[1]).toHaveProperty('let_into_environment', false);
    expect(io.thread.mock.calls[1]?.[2]).not.toBe(io.thread.mock.calls[0]?.[2]);
    expect(io.readPage).not.toHaveBeenCalled();
  });

  it(`submits the chosen starting files, management and lifetime together (${locale})`, async () => {
    await setLocale(locale);
    const selected = plan();
    selected.resolved = resolve(selected.options, { ...NO_PICKS, newEnvironment: { access: 'only_me', files: { from_thread: 'source', selection: { mode: 'selection', include: ['src/**'] } } } }, { suggest: true });
    selected.temporary = true;
    show(<Starter selected={selected} />);
    await userEvent.click(screen.getByRole('button', { name: 'Start this setup' }));
    await waitFor(() => expect(io.navigate).toHaveBeenCalledOnce());
    expect(io.thread.mock.calls[0]?.[1].environment).toEqual({ new: { access: 'only_me', files: { from_thread: 'source', selection: { mode: 'selection', include: ['src/**'] } }, config: { 'env.lifetime': 'thread' } } });
    expect(io.environment).not.toHaveBeenCalled();
    expect(io.defaultEnvironment).not.toHaveBeenCalled();
  });

  it(`checks then submits one atomic request with the named setup despite a later options answer (${locale})`, async () => {
    await setLocale(locale);
    const selected = plan();
    const result = { id: 'thread', topic_id: 'topic', environment: { id: 'made', name: 'Main' } };
    let release: (value: typeof result) => void = () => undefined;
    const pending = new Promise<typeof result>((ready) => { release = ready; });
    io.thread.mockReturnValue(pending);
    const view = show(<Starter selected={selected} />);
    try {
      expect(screen.getByText(/Agent a1/)).toHaveTextContent('Provider p1');
      await userEvent.click(screen.getByRole('button', { name: 'Start this setup' }));
      await waitFor(() => expect(io.thread).toHaveBeenCalledOnce());
      expect(io.reads).not.toHaveBeenCalled();
      expect(io.environment).not.toHaveBeenCalled();
      // A later native answer updates the available setup, not the already accepted start.
      const updated = options({ environments: [], environment_default: 'fresh', defaults: { provider_id: 'p2', agent_id: 'a2', model_name: 'm2', effort: null, locked: false } });
      view.rerender(<Starter selected={plan(updated)} />);
      act(() => { release(result); });
      await waitFor(() => expect(io.thread).toHaveBeenCalledOnce());
      const body = io.thread.mock.calls[0]?.[1];
      expect(body).toMatchObject({ binding: { agent_id: 'a1', provider_id: 'p1', arrangement: 'separated' }, model: 'm1', effort: 'low', environment: { new: {} }, first_input: { content: [{ type: 'text', text: 'Only this start' }] } });
      await waitFor(() => expect(io.navigate).toHaveBeenCalledOnce());
      expect(io.defaultEnvironment).not.toHaveBeenCalled();
      expect(io.defaultSetting).not.toHaveBeenCalled();
    } finally { view.unmount(); release(result); }
  });

  for (const kind of ['topic', 'project'] as const) it(`refuses an impossible current template before creating anything (${locale}/${kind})`, async () => {
    await setLocale(locale);
    io.offered = options({ environments: [], providers: [provider('p1', { agents: [agent('a1', { reason: 'executor_missing', arrangements: [arrangement('separated', 1, false, 'executor_missing')] })] })] });
    const selected = plan(); selected.place = { ...selected.place, kind }; selected.options = io.offered;
    show(<Starter selected={selected} />);
    await userEvent.click(screen.getByRole('button', { name: 'Start this setup' }));
    expect(await screen.findByRole('alert')).toBeVisible();
    expect(io.thread).not.toHaveBeenCalled();
    expect(io.topic).not.toHaveBeenCalled();
    expect(io.topicThread).not.toHaveBeenCalled();
    expect(io.environment).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('start.reason.executor_missing'));
    expect(io.defaultEnvironment).not.toHaveBeenCalled();
    expect(io.defaultSetting).not.toHaveBeenCalled();
    expect(io.navigate).not.toHaveBeenCalled();
  });

  it(`admits a project's topic and first thread together in one creation request (${locale})`, async () => {
    await setLocale(locale);
    const selected = plan(); selected.place = { ...selected.place, id: 'project', kind: 'project' }; selected.temporary = true;
    show(<Starter selected={selected} />);
    await userEvent.click(screen.getByRole('button', { name: 'Start this setup' }));
    await waitFor(() => expect(io.navigate).toHaveBeenCalledOnce());
    expect(io.topicThread).toHaveBeenCalledOnce();
    expect(screen.getByLabelText('Last started topic')).toHaveTextContent('new-topic');
    expect(io.topicThread.mock.calls[0]?.[0]).toBe('project');
    expect(io.topicThread.mock.calls[0]?.[1]).toMatchObject({
      binding: { agent_id: 'a1', provider_id: 'p1', arrangement: 'separated' }, model: 'm1', effort: 'low',
      environment: { new: { config: { 'env.lifetime': 'thread' } } }, first_input: { content: [{ type: 'text', text: 'Only this start' }] },
    });
    expect(io.topic).not.toHaveBeenCalled();
    expect(io.thread).not.toHaveBeenCalled();
    expect(io.environment).not.toHaveBeenCalled();
    expect(io.reads).not.toHaveBeenCalled();
    expect(io.defaultEnvironment).not.toHaveBeenCalled();
    expect(io.defaultSetting).not.toHaveBeenCalled();
  });

  it(`leaves no separate environment or partial-made notice after an atomic refusal (${locale})`, async () => {
    await setLocale(locale);
    io.thread.mockRejectedValue(new Error('The complete request was refused'));
    show(<Starter selected={plan()} />);
    await userEvent.click(screen.getByRole('button', { name: 'Start this setup' }));
    expect(await screen.findByRole('alert')).toBeVisible();
    expect(io.thread).toHaveBeenCalledOnce();
    expect(io.environment).not.toHaveBeenCalled();
    expect(screen.queryByText(i18n.t('start.made.environment', { name: 'Main' }))).toBeNull();
    expect(io.defaultEnvironment).not.toHaveBeenCalled();
    expect(io.defaultSetting).not.toHaveBeenCalled();
  });
}
