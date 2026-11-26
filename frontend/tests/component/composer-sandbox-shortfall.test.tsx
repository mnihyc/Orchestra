/** Where the chosen tool access cannot be enforced where the thread's agent runs, one line says so before a message is
 * sent, in the program's own words, with the one choice that runs as the machine allows; nothing changes by itself. */
import { screen, waitFor } from '@testing-library/react';
import { QueryClient } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';

import type * as SettingsApi from '@/api/settings';
import type * as ThreadsApi from '@/api/threads';
import { runsAs, SandboxShortfallLine } from '@/features/composer/SandboxShortfall';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import { i18n, setLocale } from '@/i18n';
import type { NodeSettingSetBody, ResolvedSetting, SandboxReadiness, Thread } from '@/generated/api-types';
import { failedStart } from './failed-start-fixtures';
import { show } from './support';
import { nativeModeSetting } from '../doubles/mode-controls';

const server = vi.hoisted(() => ({ rows: [] as ResolvedSetting[], write: vi.fn<(id: string, key: string, body: NodeSettingSetBody) => Promise<ResolvedSetting>>() }));
vi.mock('@/api/settings', async (importOriginal) => ({ ...await importOriginal<typeof SettingsApi>(), settingsQuery: (id: string) => ({ queryKey: ['settings', id], queryFn: () => Promise.resolve(server.rows) }) }));
vi.mock('@/api/threads', async (importOriginal) => ({ ...await importOriginal<typeof ThreadsApi>(), setThreadSetting: (id: string, key: string, body: NodeSettingSetBody) => server.write(id, key, body) }));
vi.mock('@/realtime/live', async (loadOriginal) => ({ ...await loadOriginal<object>(), refreshThread: vi.fn() }));
vi.mock('@tanstack/react-router', async (loadOriginal) => ({ ...await loadOriginal<object>(), Link: ({ to, params, children, className }: { to: string; params?: { id: string }; children: ReactNode; className?: string }) =>
  <a className={className} href={to.replace('$id', params?.id ?? '')}>{children}</a> }));

const reason = "Claude Code's sandbox needs bubblewrap (bwrap) and socat, which are not installed on this machine.";
// What the walk-lab machine reported for Claude Code's own tools: Workspace is chosen, Open alone is enforced there.
const shortfall: SandboxReadiness = {
  ready: false, requested: 'workspace', widest: 'open', source: 'machine_policy', machine_limit: 'open', template_limit: null,
  executor: null, enforceable: ['open'], agent_modes: ['open'], reason,
};
const environment = { id: 'environment-1', name: 'Lab workspace', ports_exposed: false, address: 'acme.ada.lab-workspace', placement: 'connected', status: 'running', owner: { display_name: 'Ada' } } as NonNullable<Thread['environment']>;
const thread = (permissions: string[], more: Partial<Thread> = {}) => ({ id: 'thread-1', name: 'cc-start', archived: false, side_of: null, lock: null, state: 'idle', blocked: null,
  binding: { provider: { id: 'provider-1' } }, owner: { display_name: 'Verify bot' }, environment, access: { permissions }, ...more }) as unknown as Thread;

beforeEach(() => {
  server.rows = [nativeModeSetting('sandbox.mode', 'workspace', 3, { source: { scope: 'node', node: null } })];
  server.write.mockReset();
  server.write.mockImplementation((_id, key, body) => Promise.resolve(nativeModeSetting(key as 'sandbox.mode', String(body.value), 4)));
});

it('runs as the widest mode the machine enforces for the agent within what the environment allows', () => {
  expect(runsAs(shortfall)).toBe('open');
  expect(runsAs({ ...shortfall, widest: 'workspace' })).toBeNull();
  expect(runsAs({ ...shortfall, enforceable: ['read_only', 'open'], agent_modes: [] })).toBe('open');
});

it.each(['en', 'zh-CN'] as const)('says why before sending and lets the thread manager choose Open with the revision read at the press, in %s', async (locale) => {
  await setLocale(locale);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const view = show(<SandboxShortfallLine thread={thread(['thread.manage', 'thread.interact'])} report={shortfall} />, cache);
  try {
    const line = await screen.findByRole('status');
    const mode = i18n.t('choiceCatalog.sandboxLevel.workspace.name');
    expect(line).toHaveTextContent(i18n.t('composer.confinement.shortfall', { mode, place: 'Lab workspace' }));
    expect(line).toHaveTextContent(reason);
    const open = screen.getByRole('button', { name: i18n.t('composer.confinement.runAs', { mode: i18n.t('choiceCatalog.sandboxLevel.open.name') }) });
    expect(server.write).not.toHaveBeenCalled();
    await userEvent.click(open);
    await waitFor(() => expect(server.write).toHaveBeenCalledWith('thread-1', 'sandbox.mode', { value: 'open', expected_revision: 3 }));
  } finally { view.unmount(); cache.clear(); await setLocale('en'); }
});

it('names who may change it for someone who does not manage the thread, and offers nothing to press', async () => {
  const view = show(<SandboxShortfallLine thread={thread(['thread.interact'])} report={shortfall} />);
  try {
    const line = await screen.findByRole('status');
    expect(line).toHaveTextContent(i18n.t('composer.confinement.readOnly', { who: 'Verify bot' }));
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  } finally { view.unmount(); }
});

it('keeps a lock above in its own words and changes nothing', async () => {
  server.rows = [nativeModeSetting('sandbox.mode', 'workspace', 3, { writable: false, locked_by: { scope: 'org', node: { id: 'org-1', kind: 'org', name: 'Acme', redacted: false } } as ResolvedSetting['locked_by'] })];
  const view = show(<SandboxShortfallLine thread={thread(['thread.manage', 'thread.interact'])} report={shortfall} />);
  try {
    await userEvent.click(await screen.findByRole('button', { name: i18n.t('composer.confinement.runAs', { mode: i18n.t('choiceCatalog.sandboxLevel.open.name') }) }));
    expect(await screen.findByText(i18n.t('composer.mode.locked', { place: 'Acme' }))).toBeInTheDocument();
    expect(server.write).not.toHaveBeenCalled();
  } finally { view.unmount(); }
});

it('offers the same choice on a thread already held for its tool access', async () => {
  const native = failedStart().thread;
  const place = { ...environment, ...native.environment, name: 'Lab workspace', status: 'running' } as NonNullable<Thread['environment']>;
  const held: Thread = { ...native, id: 'thread-1', state: 'blocked', lock: null, environment: place,
    blocked: { reason: 'binding_invalid', detail: 'Sandbox: Lab workspace: the agent’s own tools cannot confine commands.', sandbox: { environment: place, machine: null, policy_manage: false, sandbox: shortfall } } };
  const view = show(<BlockedBanner thread={held} onProceed={() => undefined} />);
  try {
    // The banner says what the program reported, as the line before sending did.
    expect(document.querySelector('[data-blocked="binding_invalid"]')).toHaveTextContent(reason);
    await userEvent.click(await screen.findByRole('button', { name: i18n.t('composer.confinement.runAs', { mode: i18n.t('choiceCatalog.sandboxLevel.open.name') }) }));
    await waitFor(() => expect(server.write).toHaveBeenCalledWith('thread-1', 'sandbox.mode', { value: 'open', expected_revision: 3 }));
  } finally { view.unmount(); }
});
