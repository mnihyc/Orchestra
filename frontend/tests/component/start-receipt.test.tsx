/** A real start helper stops at an accepted receipt before full-thread followups or navigation. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { NO_PICKS, resolve } from '@/features/start/choices';
import { rememberSetup, useLastSetup } from '@/features/start/lastSetup';
import { useStart, type StartPlan } from '@/features/start/start';
import type { Thread, ThreadCreateResponse } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ErrorAlert } from '@/ui/ErrorState';
import { options } from '../unit/start-fixtures';
import { show } from './support';

const io = vi.hoisted(() => ({
  send: vi.fn<(request: Request) => Promise<Response>>(),
  navigate: vi.fn(),
  started: vi.fn<(thread: Thread) => void>(),
}));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  const client = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: io.send });
  return { ...actual, client, api: client.api };
});
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => io.navigate }));

function Starter({ selected }: { selected: StartPlan }) {
  const make = useStart(io.started);
  const last = useLastSetup();
  return <><button disabled={make.pending} onClick={() => make.start(selected)}>Start this setup</button>
    <output aria-label="Last started topic">{last?.place ?? ''}</output>
    <output aria-label="Made environment">{make.standing.environment?.id ?? ''}</output>
    {make.error !== null && <ErrorAlert error={make.error} />}</>;
}

beforeEach(() => {
  vi.clearAllMocks(); localStorage.clear(); queryClient.clear();
  client.session.setState({ status: 'signed_in', userId: 'receipt-starter' });
  rememberSetup('receipt-starter', { place: 'previous-topic', picks: NO_PICKS });
});
afterEach(async () => {
  act(() => client.session.setState({ status: 'signed_out', userId: null }));
  queryClient.clear();
  await setLocale('en');
});

it('acknowledges a committed start without full-thread followups or automatic resend', async () => {
  const topic = '00000000-0000-4000-8102-000000000003';
  const offered = options();
  const selected: StartPlan = {
    place: { id: topic, name: 'Login', kind: 'topic', parentId: null, archived: false, folder: false, status: 'open' },
    options: offered, resolved: resolve(offered, NO_PICKS, { suggest: true }),
    name: '', directory: '', privately: false, message: 'Only this start', letIntoEnvironment: true,
  };
  const receipt: ThreadCreateResponse = {
    projection: 'receipt', thread_id: '00000000-0000-4000-8102-000000000001',
    topic_id: topic, input_id: '00000000-0000-4000-8102-000000000002',
  };
  const sent: Request[] = [];
  io.send.mockImplementation((request) => {
    sent.push(request.clone());
    return Promise.resolve(new Response(JSON.stringify(receipt), {
      status: 200, headers: { 'Content-Type': 'application/json', 'X-Request-Id': 'start-accepted-once' },
    }));
  });
  const view = show(<Starter selected={selected} />);
  await userEvent.click(screen.getByRole('button', { name: 'Start this setup' }));
  expect(await screen.findByText(i18n.t('ui.receipt.accepted'))).toBeVisible();
  expect(view.container.querySelector('[data-state-line]')).toHaveAttribute('role', 'status');
  expect(view.container.querySelector('[data-state-line]')).toHaveAttribute('data-state-tone', 'neutral');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start this setup' })).toBeEnabled());
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('ui.receipt.readback') })).not.toBeInTheDocument();
  expect(screen.getAllByRole('button')).toHaveLength(1);
  expect(screen.getByLabelText('Last started topic')).toHaveTextContent('previous-topic');
  expect(screen.getByLabelText('Made environment')).toBeEmptyDOMElement();
  expect(io.started).not.toHaveBeenCalled();
  expect(io.navigate).not.toHaveBeenCalled();
  // Settling and rendering the same plan is not a second submission or an implicit readback.
  view.rerender(<Starter selected={selected} />);
  await act(async () => { await Promise.resolve(); });
  expect(io.send).toHaveBeenCalledOnce();
  expect(sent.map((request) => `${request.method} ${new URL(request.url).pathname}`)).toEqual([`POST /api/v1/topics/${topic}/threads`]);
  const request = sent[0];
  if (request === undefined) throw new Error('Expected the single accepted start');
  const body: unknown = await request.json();
  expect(body).toMatchObject({ environment: { use: 'e1' }, let_into_environment: true,
    first_input: { content: [{ type: 'text', text: selected.message }] } });
  expect(request.headers.get('Idempotency-Key')).toEqual(expect.any(String));
  expect(request.headers.get('Idempotency-Key')).not.toBe('');
});
