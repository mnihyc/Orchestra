/** Fleet review uses the real typed client and one World producer for each native plan/job/receipt. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { NodeMovesDialog, environmentMoveGroups } from '@/features/fleet/NodeMoves';
import { planEnvironmentMove } from '@/api/changes';
import { moveLists } from '@/features/change/EnvironmentChange';
import type { Environment, NodeEnvironmentMoveCreateReceipt } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { RUNS_ON } from '../doubles/fixtures';
import { Refusal, World } from '../doubles/world';
import { show } from './support';

const wire = vi.hoisted(() => vi.fn<(request: Request) => Promise<Response>>());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  const client = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: wire });
  return { ...actual, client, api: client.api };
});
let world: World;
let environments: Environment[];
let opaque = false;
let rejectCreate = false;
let sent: Request[] = [];
function object(value: unknown): value is Record<string, unknown> { return typeof value === 'object' && value !== null && !Array.isArray(value); }

beforeEach(() => {
  world = new World(); opaque = false; rejectCreate = false; sent = []; wire.mockReset();
  const ada = world.people.get('ada');
  if (ada === undefined) throw new Error('The native reader is required');
  world.change.control('/mode', { mode: 'manual' });
  world.change.control('/host', { id: RUNS_ON.buildHost, schedulable: true });
  world.change.control('/environment', { id: RUNS_ON.sandbox, manages: ['ada'] });
  environments = [RUNS_ON.workspace, RUNS_ON.sandbox].map((id) => {
    const row = world.resources.inventoryView(ada, 'environment', id);
    if (row?.kind !== 'environment') throw new Error('The full native environment projection is required');
    return row.value;
  });
  wire.mockImplementation(async (request) => {
    sent.push(request.clone());
    const url = new URL(request.url), path = url.pathname.replace(/^\/api\/v1/, '');
    const body: unknown = request.method === 'GET' ? {} : await request.json();
    if (!object(body)) throw new Error('The native request body is an object');
    const create = request.method === 'POST' && /^\/nodes\/[^/]+\/environments\/moves$/.test(path);
    if (create && rejectCreate) { rejectCreate = false; throw new Error('The connection ended before acceptance'); }
    try {
      for (const { part } of world.parts) {
        const answer = await part.handle?.(ada, request.method, path, url.searchParams, body, request.headers.get('Idempotency-Key') ?? undefined);
        if (answer === null || answer === undefined) continue;
        let value = answer.body;
        if (create && opaque) {
          if (!object(value) || typeof value.batch_id !== 'string' || !Array.isArray(value.jobs)) throw new Error('The actual created batch is required');
          const job_ids = value.jobs.map((row: unknown) => {
            if (!object(row) || !object(row.job) || typeof row.job.id !== 'string') throw new Error('The actual created job is required');
            return row.job.id;
          });
          const node = path.split('/')[2];
          if (node === undefined) throw new Error('The actual native selector is required');
          const receipt: NodeEnvironmentMoveCreateReceipt = { projection: 'receipt', batch_id: value.batch_id, node_id: node, job_ids };
          value = receipt;
        }
        return new Response(answer.status === 204 ? null : JSON.stringify(value), { status: answer.status, headers: { 'Content-Type': 'application/json', ...answer.headers } });
      }
      throw new Error('No native fixture route for ' + path);
    } catch (error) {
      if (!(error instanceof Refusal)) throw error;
      return new Response(JSON.stringify(error.answer.body), { status: error.answer.status, headers: { 'Content-Type': 'application/json' } });
    }
  });
});
afterEach(async () => { cleanup(); delete document.documentElement.dataset.theme; await setLocale('en'); });

function creates() { return sent.filter((request) => request.method === 'POST' && /\/nodes\/[^/]+\/environments\/moves$/.test(new URL(request.url).pathname)); }
async function chooseCopy() {
  await userEvent.click(screen.getByRole('checkbox', { name: i18n.t('changeCompact.copyDirectories') }));
  const button = await screen.findByRole('button', { name: i18n.t('envTransfer.changeCount', { count: 2, number: '2' }) });
  await waitFor(() => expect(button).not.toHaveAttribute('aria-disabled', 'true'));
  return button;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`reviews both native lifecycles and sends the saved node plan with an explicit notification choice ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    show(<NodeMovesDialog environments={environments} onClose={vi.fn()} />);
    const confirm = await chooseCopy();
    expect(screen.getByRole('table', { name: i18n.t('fleetMoves.title') })).toHaveTextContent('Workspace');
    expect(screen.getByRole('table', { name: i18n.t('fleetMoves.title') })).toHaveTextContent('Sandbox');
    const before = sent.findLast((request) => new URL(request.url).pathname.endsWith('/environments/moves/preflight'));
    if (before === undefined) throw new Error('The actual saved plan is required');
    const request: unknown = await before.json();
    if (!object(request) || !Array.isArray(request.environment_ids)) throw new Error('The actual native selection is required');
    const selected = request.environment_ids.map((id: unknown) => { if (typeof id !== 'string') throw new Error('A native environment identifier is required'); return id; });
    expect(selected.sort()).toEqual(environments.map((environment) => environment.id).sort());
    expect(new URL(before.url).pathname).toBe(`/api/v1/nodes/${environments[0]?.home.id}/environments/moves/preflight`);
    await userEvent.click(confirm);
    await waitFor(() => expect(creates()).toHaveLength(1));
    const body: unknown = await creates()[0]?.json();
    if (!object(body)) throw new Error('The single native create body is required');
    expect(typeof body.preflight_id).toBe('string'); expect(body.notify).toBe(false);
    expect(body).not.toHaveProperty('environment_ids'); expect(body).not.toHaveProperty('host_id');
    expect(creates()[0]?.headers.get('Idempotency-Key')).toBeTruthy();
    expect(sent.filter((entry) => entry.method === 'POST' && /\/environments\/[^/]+\/moves$/.test(new URL(entry.url).pathname))).toHaveLength(0);
  });
}

it('acknowledges a generated receipt and retires the old plan rather than treating acceptance as failure', async () => {
  opaque = true;
  show(<NodeMovesDialog environments={environments} onClose={vi.fn()} />);
  await userEvent.click(await chooseCopy());
  await screen.findByRole('dialog', { name: i18n.t('fleetMoves.accepted') });
  expect(screen.queryByRole('table', { name: i18n.t('fleetMoves.title') })).not.toBeInTheDocument();
  expect(screen.queryByText('Workspace', { exact: true })).not.toBeInTheDocument();
  expect(screen.queryByText('Sandbox', { exact: true })).not.toBeInTheDocument();
  expect(creates()).toHaveLength(1);
});

it('keeps the unresolved native create identity when the connection fails before acceptance', async () => {
  rejectCreate = true;
  show(<NodeMovesDialog environments={environments} onClose={vi.fn()} />);
  const confirm = await chooseCopy();
  await userEvent.click(confirm);
  await waitFor(() => expect(creates()).toHaveLength(1));
  await waitFor(() => expect(confirm).not.toHaveAttribute('aria-busy', 'true'));
  await userEvent.click(confirm);
  await waitFor(() => expect(creates()).toHaveLength(2));
  expect(creates()[0]?.headers.get('Idempotency-Key')).toBe(creates()[1]?.headers.get('Idempotency-Key'));
  expect(await creates()[0]?.json()).toEqual(await creates()[1]?.json());
});

it('groups distinct native homes without dropping mixed lifecycles or counting an environment twice', () => {
  const first = environments[0], second = environments[1];
  if (first === undefined || second === undefined) throw new Error('Both lifecycle fixtures are required');
  const elsewhere = { ...second, home: { ...second.home, id: 'another-visible-native-place', name: 'Another home' } };
  const groups = environmentMoveGroups([first, elsewhere, first].map(({ id, home, template }) => ({ id, home, template })));
  expect(groups).toHaveLength(2);
  expect(groups[0]?.environments.map((row) => row.id)).toEqual([first.id]);
  expect(groups[1]?.environments.map((row) => row.id)).toEqual([second.id]);
});

for (const locale of ['en', 'zh-CN'] as const) it(`uses the declared old-unit fact in the saved review even when the source has a host page ${locale}`, async () => {
  await setLocale(locale);
  const plan = await planEnvironmentMove(RUNS_ON.workspace, { copy_directories: true });
  if (!('automatic' in plan.to)) throw new Error('The native permitted host fixture is required');
  const lists = moveLists({ ...plan, from: plan.to.host, source_has_unit: false }, locale, i18n.t);
  expect(lists.ends.find((line) => line.id === 'old')?.text).toBe(i18n.t('envTransfer.oldKept'));
  expect(lists.changes.find((line) => line.id === 'moves')?.text).toContain(i18n.t('envTransfer.copy'));
  for (const server of plan.mcp.filter((entry) => entry.fate === 'kept')) expect(lists.stays.filter((line) => line.id === `mcp-${server.id}`)).toHaveLength(1);
});
