/** Real difference/door/query readers; synthetic typed native replies never grant another workspace. */
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { QueryClientProvider } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Workspace from '@/api/workspace';
import type * as Live from '@/realtime/live';
import { ApiError, client } from '@/api/client';
import { queryClient } from '@/api/query';
import { DifferenceTab } from '@/features/files/viewers/Difference';
import { makeTab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { workspaceThreadRead } from '@/features/workbench/workspaceRead';
import { threadWriter } from '@/realtime/stores/thread';
import type { Environment, EnvironmentChange, Thread } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { detailEnvironment } from './resource-detail-fixtures';
import { TooltipProvider } from '@/ui/Tooltip';

interface Reply { text: string; truncated: boolean }
interface Gate { promise: Promise<Reply>; resolve: (reply: Reply) => void }
const rig = vi.hoisted(() => ({
  environments: new Map<string, Environment>(), envCalls: [] as string[],
  reads: [] as { environmentId: string; threadId: string | null; staged: boolean }[],
  gates: new Map<string, Gate>(), changes: [] as EnvironmentChange[],
}));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useThreadFeed: () => undefined }));
vi.mock('@/api/workspace', async (original) => {
  const actual = await original<typeof Workspace>();
  return { ...actual,
    environmentQuery: (id: string) => ({ ...actual.environmentQuery(id), queryFn: () => {
      rig.envCalls.push(id);
      const environment = rig.environments.get(id);
      return environment === undefined ? Promise.reject(new ApiError({ type: 'orch:error:not_found', code: 'not_found', title: 'Not found', status: 404, detail: 'Unavailable synthetic environment', request_id: 'source-fixture' }, null)) : Promise.resolve(environment);
    } }),
    gitDiff: (door: Workspace.Door, staged: boolean) => {
      rig.reads.push({ ...door, staged });
      return rig.gates.get(`${door.environmentId}:${door.threadId ?? 'own'}:${staged}`)?.promise
        ?? Promise.resolve({ text: patch(`actual ${door.environmentId} ${door.threadId ?? 'own'}`), truncated: false });
    },
    changesQuery: (door: Workspace.Door, all: boolean) => ({ ...actual.changesQuery(door, all), queryFn: () => {
      rig.reads.push({ ...door, staged: false }); return Promise.resolve(rig.changes);
    } }),
  };
});
vi.mock('@/features/source/Review', () => ({ CommentOnLines: () => null }));
vi.mock('@/ui/DiffView', () => ({ DiffView: ({ diff }: { diff: string | null }) => <output aria-label="Actual difference">{diff}</output> }));

const savedSession = client.session.getState();
const savedOptions = queryClient.getDefaultOptions();
let thread: Thread;
function patch(text: string): string { return `diff --git a/main.rs b/main.rs\n--- a/main.rs\n+++ b/main.rs\n@@ -1 +1 @@\n-before\n+${text}\n`; }
function environment(id: string): Environment {
  return { ...detailEnvironment, id, name: id, access: { ...detailEnvironment.access, permissions: ['environment.view', 'files.view'] } };
}
function bind(environmentId: string | null, permissions = ['files.view']): void {
  const row: Thread = { ...thread, environment: environmentId === null ? null : { id: environmentId, name: environmentId, owner: thread.owner,
    address: `synthetic.${environmentId}`, status: 'running', placement: 'managed', ports_exposed: false }, environment_permissions: permissions };
  const read = workspaceThreadRead('workspace-a', row.id)!;
  const captured = threadWriter.capture(read, client.session.getState().readerVersion);
  threadWriter.reset(read, { thread: row, items: [], turns: [], segments: [] }, false, captured);
}
function held(key: string): Gate {
  let accept: ((reply: Reply) => void) | undefined;
  const promise = new Promise<Reply>((resolve) => { accept = resolve; });
  const gate = { promise, resolve: (reply: Reply) => { if (accept === undefined) throw new Error('The reply gate is not installed.'); accept(reply); } };
  rig.gates.set(key, gate); return gate;
}
function draw(kind: 'git' | 'change' = 'git', through = thread.id, staged = false) {
  const tab = makeTab('difference', { kind, env: 'workspace-a', thread: through, path: 'main.rs', item: kind === 'change' ? 'recorded-item-a' : '', staged: staged ? '1' : '0' });
  return render(<QueryClientProvider client={queryClient}><TooltipProvider><DifferenceTab tab={tab} area="main" /></TooltipProvider></QueryClientProvider>);
}
beforeEach(() => {
  queryClient.clear(); threadWriter.clear(); useWorkbench.getState().leave();
  rig.environments.clear(); rig.envCalls.length = 0; rig.reads.length = 0; rig.gates.clear(); rig.changes = [];
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The synthetic native thread is missing.');
  thread = world.threads.thread(person, row);
  client.session.setState({ status: 'signed_in', userId: person.me.id });
  queryClient.setDefaultOptions({ ...savedOptions, queries: { ...savedOptions.queries, retry: false } });
  bind('workspace-a');
});
afterEach(async () => {
  cleanup(); await queryClient.cancelQueries(); queryClient.clear(); threadWriter.clear();
  useWorkbench.getState().leave(); queryClient.setDefaultOptions(savedOptions); client.session.setState(savedSession);
});

it('keeps the matching thread door without attempting a direct-environment read that the person lacks', async () => {
  draw();
  expect(await screen.findByLabelText('Actual difference')).toHaveTextContent(`actual workspace-a ${thread.id}`);
  expect(rig.envCalls).toEqual([]);
  expect(rig.reads).toEqual([{ environmentId: 'workspace-a', threadId: thread.id, staged: false }]);
});

it('a rebound thread never retargets its saved diff to B, and a late A-through-T reply cannot defeat A refusal', async () => {
  const old = held(`workspace-a:${thread.id}:false`);
  draw(); await waitFor(() => expect(rig.reads).toHaveLength(1));
  act(() => bind('workspace-b'));
  await waitFor(() => expect(rig.envCalls).toEqual(['workspace-a']));
  await act(async () => { old.resolve({ text: patch('late old authorized response'), truncated: false }); await old.promise; });
  expect(screen.queryByLabelText('Actual difference')).toBeNull();
  expect(screen.queryByText('late old authorized response')).toBeNull();
  expect(rig.reads.every((read) => read.environmentId === 'workspace-a')).toBe(true);
});

it('after a real mismatch it requires A own standing, never reuses B standing or a late old through response', async () => {
  const old = held(`workspace-a:${thread.id}:false`);
  rig.environments.set('workspace-a', environment('workspace-a'));
  draw(); await waitFor(() => expect(rig.reads).toHaveLength(1));
  act(() => bind('workspace-b'));
  expect(await screen.findByLabelText('Actual difference')).toHaveTextContent('actual workspace-a own');
  await act(async () => { old.resolve({ text: patch('old through result'), truncated: false }); await old.promise; });
  expect(screen.getByLabelText('Actual difference')).toHaveTextContent('actual workspace-a own');
  expect(rig.envCalls).toEqual(['workspace-a']);
  expect(rig.reads).toEqual([{ environmentId: 'workspace-a', threadId: thread.id, staged: false }, { environmentId: 'workspace-a', threadId: null, staged: false }]);
});

it('does not upgrade an explicit matching thread files refusal with unrelated direct rights', () => {
  rig.environments.set('workspace-a', environment('workspace-a'));
  bind('workspace-a', []); draw();
  expect(rig.envCalls).toEqual([]); expect(rig.reads).toEqual([]);
  expect(screen.queryByLabelText('Actual difference')).toBeNull();
});

it('direct staged diffs need no global thread, preserve staged identity, and stop publishing after removal', async () => {
  rig.environments.set('workspace-a', environment('workspace-a'));
  const pending = held('workspace-a:own:true');
  const view = draw('git', '', true);
  await waitFor(() => expect(rig.reads).toEqual([{ environmentId: 'workspace-a', threadId: null, staged: true }]));
  view.unmount();
  await act(async () => { pending.resolve({ text: patch('no removed view'), truncated: false }); await pending.promise; });
  expect(screen.queryByLabelText('Actual difference')).toBeNull();
  expect(rig.envCalls).toEqual(['workspace-a']);
});

it('recorded differences retain the actual item rather than selecting another latest row with the same path', async () => {
  const base: EnvironmentChange = { path: 'main.rs', old_path: null, at: '2026-10-17T00:00:00Z', change: 'modify',
    item_id: 'recorded-item-a', thread_id: thread.id, binary: false, diff: patch('actual recorded item A'), added: 1, removed: 1, truncated: false };
  rig.changes = [{ ...base, item_id: 'newer-item-b', diff: patch('wrong latest same path') }, base];
  draw('change');
  expect(await screen.findByLabelText('Actual difference')).toHaveTextContent('actual recorded item A');
  expect(screen.getByLabelText('Actual difference')).not.toHaveTextContent('wrong latest same path');
  expect(rig.reads).toEqual([{ environmentId: 'workspace-a', threadId: thread.id, staged: false }]);
  expect(rig.envCalls).toEqual([]);
});
