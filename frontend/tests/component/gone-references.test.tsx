/** A thread whose provider, model, agent profile, machine or environment is gone says so in one line of its composer. */
import { QueryClient } from '@tanstack/react-query';
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as ThreadsApi from '@/api/threads';
import { ComposerControls } from '@/features/composer/Controls';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import { waitingWords } from '@/features/timeline/waiting';
import type { Thread, ThreadGone, ThreadParameters } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const change = vi.hoisted(() => vi.fn());
const read = vi.hoisted(() => ({ gone: [] as ThreadGone[], calls: 0 }));
vi.mock('@/features/change/target', async (loadOriginal) => ({ ...await loadOriginal<object>(), openChange: change }));
vi.mock('@/features/composer/ModelEffort', () => ({ ModelEffort: () => <button>Model and effort</button> }));
vi.mock('@/features/composer/ApprovalMode', () => ({ ApprovalModePicker: () => <button>Approval mode</button>, ConfinementPicker: () => <button>Sandbox</button> }));
vi.mock('@/features/usage/context/ContextRing', () => ({ ContextRing: () => <button>Context breakdown</button> }));
vi.mock('@/features/composer/runsWith', async (loadOriginal) => ({ ...await loadOriginal<object>(), useRunsWith: () => ({ parameters: undefined, states: [] }) }));
vi.mock('@/api/threads', async (loadOriginal) => ({ ...await loadOriginal<typeof ThreadsApi>(),
  parametersQuery: (id: string) => ({ queryKey: ['gone-parameters', id], queryFn: () => { read.calls += 1; return Promise.resolve(parameters(read.gone)); }, staleTime: Infinity }),
}));

function parameters(gone: ThreadGone[]): ThreadParameters {
  return { approval_mode: 'on_request', approval_reviewer: null, parameters: [], gone,
    model: { value: 'fake-small', writable: true, options: [], source: { scope: 'thread', node: null } },
    effort: { value: null, writable: true, supported: false, source: { scope: 'thread', node: null } } };
}

const thread = { id: 'thread-2', name: 'r13b-thread2', archived: false, state: 'idle', blocked: null, side_of: null, lock: null, environment: null, directory: '',
  model: 'fake-small', model_display: null, effort: null, pending_inputs: 0, source: null,
  owner: { id: 'ada', display_name: 'Ada' }, binding: { agent: { id: 'agent', name: 'Example agent' }, provider: { id: 'provider', name: 'r13a-fake', status: 'ready' } },
  access: { permissions: ['thread.view', 'thread.manage', 'thread.interact'] },
} as unknown as Thread;

const provider: ThreadGone = { kind: 'provider', id: 'provider', name: 'r13a-fake' };
const environment: ThreadGone = { kind: 'environment', id: 'environment', name: 'r13b-env' };
const agent: ThreadGone = { kind: 'agent', id: 'agent', name: 'Example agent' };

beforeEach(() => { change.mockClear(); read.gone = []; read.calls = 0; });
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('names a deleted provider in one line and opens the model list to choose another (%s)', async (locale) => {
  await setLocale(locale);
  const choose = vi.fn();
  show(<ComposerControls thread={thread} attachment={null} gone={[provider]} onChooseModel={choose} />);
  const line = screen.getByRole('group', { name: i18n.t('composer.gone.label') });
  expect(line).toHaveTextContent(i18n.t('composer.gone.provider', { name: 'r13a-fake' }));
  expect(within(line).getAllByRole('button')).toHaveLength(1);
  await userEvent.click(within(line).getByRole('button', { name: i18n.t('composer.gone.choose') }));
  expect(choose).toHaveBeenCalledOnce();
  expect(change).not.toHaveBeenCalled();
  // The line stands with the other controls, which stay usable.
  expect(screen.getByRole('button', { name: 'Model and effort' })).toBeEnabled();
});

it('opens where the thread works for a removed environment and the switch for a deleted agent profile', async () => {
  const choose = vi.fn();
  const view = show(<ComposerControls thread={thread} attachment={null} gone={[environment]} onChooseModel={choose} />);
  expect(screen.getByText(i18n.t('composer.gone.environment', { name: 'r13b-env' }))).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('composer.gone.choose') }));
  expect(change).toHaveBeenCalledExactlyOnceWith({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'environment' });
  view.unmount(); change.mockClear();
  show(<ComposerControls thread={thread} attachment={null} gone={[agent]} onChooseModel={choose} />);
  expect(screen.getByText(i18n.t('composer.gone.agent', { name: 'Example agent' }))).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('composer.gone.choose') }));
  expect(change).toHaveBeenCalledExactlyOnceWith({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider' });
  expect(choose).not.toHaveBeenCalled();
});

it('tells a driver who may not change the binding who can, with no act', () => {
  const driver = { ...thread, access: { permissions: ['thread.view', 'thread.interact'] } } as unknown as Thread;
  show(<ComposerControls thread={driver} attachment={null} gone={[provider]} onChooseModel={vi.fn()} />);
  const line = screen.getByRole('group', { name: i18n.t('composer.gone.label') });
  expect(line).toHaveTextContent(i18n.t('composer.row.bindingReadOnly', { who: 'Ada' }));
  expect(within(line).queryByRole('button')).not.toBeInTheDocument();
});

it('says nothing when nothing is gone', () => {
  show(<ComposerControls thread={thread} attachment={null} gone={[]} onChooseModel={vi.fn()} />);
  expect(screen.queryByRole('group', { name: i18n.t('composer.gone.label') })).not.toBeInTheDocument();
});

it('leaves a deleted provider to the composer line instead of a second banner, reading nothing of its own', () => {
  const blocked = { ...thread, state: 'blocked', blocked: { reason: 'provider_unavailable', detail: 'deleted' } } as unknown as Thread;
  // What the composer read is in the cache; the banner only looks at it.
  const cached = (gone: ThreadGone[]) => {
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    cache.setQueryData(['gone-parameters', blocked.id], parameters(gone));
    return cache;
  };
  const first = show(<BlockedBanner thread={blocked} onProceed={vi.fn()} />, cached([provider]));
  expect(screen.queryByText(i18n.t('resourceImpact.providerRemoved'))).not.toBeInTheDocument();
  first.unmount();
  show(<BlockedBanner thread={blocked} onProceed={vi.fn()} />, cached([]));
  expect(screen.getByText(i18n.t('resourceImpact.providerRemoved'))).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('change.banner.switchProvider') })).toBeVisible();
  expect(read.calls).toBe(0);
});

it('keeps the banner for a cause the line does not name, and for a thread it does not compose', () => {
  const blocked = { ...thread, state: 'blocked', blocked: { reason: 'provider_unavailable', detail: 'deleted' } } as unknown as Thread;
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(['gone-parameters', blocked.id], parameters([environment]));
  const first = show(<BlockedBanner thread={blocked} onProceed={vi.fn()} />, cache);
  expect(screen.getByText(i18n.t('resourceImpact.providerRemoved'))).toBeVisible();
  first.unmount();
  const archived: Thread = { ...blocked, archived: true };
  cache.setQueryData(['gone-parameters', blocked.id], parameters([provider]));
  const second = show(<BlockedBanner thread={archived} onProceed={vi.fn()} />, cache);
  expect(screen.getByText(i18n.t('resourceImpact.providerRemoved'))).toBeVisible();
  second.unmount();
  // The program's own composer has no gone line, so its thread keeps the banner.
  const program = { ...blocked, program: { id: 'orch' } } as unknown as Thread;
  show(<BlockedBanner thread={program} onProceed={vi.fn()} />, cache);
  expect(screen.getByText(i18n.t('resourceImpact.providerRemoved'))).toBeVisible();
  expect(read.calls).toBe(0);
});

it.each(['en', 'zh-CN'] as const)('says that the environment was removed, not that the thread has no agent yet (%s)', async (locale) => {
  await setLocale(locale);
  const removed = { ...thread, state: 'blocked', blocked: { reason: 'unbound', detail: 'Its environment was removed.' } } as unknown as Thread;
  expect(waitingWords(removed, i18n.t)).toBe(i18n.t('composer.gone.environmentUnnamed'));
  const unbound = { ...removed, blocked: { reason: 'unbound', detail: null } } as unknown as Thread;
  expect(waitingWords(unbound, i18n.t)).toBe(i18n.t('thread.blocked.unbound'));
});
