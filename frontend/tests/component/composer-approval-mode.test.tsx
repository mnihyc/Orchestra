/** The composer uses only offered native modes, writes the displayed revision, and refuses an unseen competing choice. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import type * as SettingsApi from '@/api/settings';
import type * as ThreadsApi from '@/api/threads';
import { ApprovalModePicker } from '@/features/composer/ApprovalMode';
import type { NodeSettingSetBody, ResolvedSetting, Thread, ThreadParameters } from '@/generated/api-types';
import { show } from './support';

const server = vi.hoisted(() => ({ rows: [] as ResolvedSetting[], parameters: {} as ThreadParameters, settingReads: vi.fn(), parameterReads: vi.fn(), write: vi.fn<(id: string, key: string, body: NodeSettingSetBody) => Promise<ResolvedSetting>>() }));
vi.mock('@/api/settings', async (importOriginal) => ({ ...await importOriginal<typeof SettingsApi>(), settingsQuery: (id: string) => ({ queryKey: ['settings', id], queryFn: () => { server.settingReads(); return Promise.resolve(server.rows); }, staleTime: Infinity }) }));
vi.mock('@/api/threads', async (importOriginal) => ({ ...await importOriginal<typeof ThreadsApi>(),
  parametersQuery: (id: string) => ({ queryKey: ['thread-parameters', id], queryFn: () => { server.parameterReads(); return Promise.resolve(server.parameters); }, staleTime: Infinity }),
  setThreadSetting: (id: string, key: string, body: NodeSettingSetBody) => server.write(id, key, body),
}));
vi.mock('@/realtime/live', () => ({ refreshThread: vi.fn() }));

const row = (value: string, revision: number): ResolvedSetting => ({ key: 'approval.mode', value, own: { value, revision, locked: false }, source: { scope: 'thread', node: null }, locked_by: null, writable: true, parts: [] });
const parameters = (mode: string): ThreadParameters => ({ model: { value: 'model-1', options: [], writable: true, source: { scope: 'thread', node: null } },
  effort: { value: null, writable: true, supported: false, source: { scope: 'default', node: null } }, parameters: [], approval_mode: mode, approval_modes: ['ask', 'on_request'], approval_reviewer: null });
const thread = { id: 'thread-1', archived: false, side_of: null, binding: { provider: { id: 'provider-1' } }, owner: { display_name: 'Ada' }, access: { permissions: ['thread.manage'] } } as Thread;

beforeEach(() => {
  server.write.mockReset();
  server.settingReads.mockClear();
  server.parameterReads.mockClear();
  server.rows = [row('on_request', 3)];
  server.parameters = parameters('on_request');
  server.write.mockImplementation((_id, _key, body) => {
    const next = row(String(body.value), 4);
    server.rows = [next];
    server.parameters = parameters(String(body.value));
    return Promise.resolve(next);
  });
});

async function open() {
  await userEvent.click(await screen.findByRole('button', { name: 'Approval mode: On request' }));
  const picker = screen.getByRole('dialog', { name: 'Approval mode' });
  await waitFor(() => expect(within(picker).getByRole('button', { name: 'Ask' })).toBeEnabled());
  return picker;
}

it('shows the current mode, offers only declared choices and writes their displayed revision', async () => {
  show(<ApprovalModePicker thread={thread} />);
  const picker = await open();
  expect(within(picker).getByRole('button', { name: 'On request', pressed: true })).toBeVisible();
  expect(within(picker).queryByRole('button', { name: 'Auto' })).not.toBeInTheDocument();
  expect(within(picker).queryByRole('button', { name: 'Bypass' })).not.toBeInTheDocument();
  expect(within(picker).getByText(/Capability, port and access requests follow their own policies/)).toBeVisible();
  await userEvent.click(within(picker).getByRole('button', { name: 'Ask' }));
  await waitFor(() => expect(server.write).toHaveBeenCalledExactlyOnceWith(thread.id, 'approval.mode', { value: 'ask', expected_revision: 3 }));
  expect(await screen.findByRole('button', { name: 'Approval mode: Ask' })).toBeVisible();
  expect(server.settingReads).toHaveBeenCalledTimes(1);
  expect(server.parameterReads).toHaveBeenCalledTimes(1);
  await userEvent.click(screen.getByRole('button', { name: 'Approval mode: Ask' }));
  const again = screen.getByRole('dialog', { name: 'Approval mode' });
  await userEvent.click(within(again).getByRole('button', { name: 'On request' }));
  await waitFor(() => expect(server.write).toHaveBeenLastCalledWith(thread.id, 'approval.mode', { value: 'on_request', expected_revision: 4 }));
  expect(server.settingReads).toHaveBeenCalledTimes(1);
  expect(server.parameterReads).toHaveBeenCalledTimes(1);
});

it.each([false, true])('keeps a refused choice open with the current mode and credits only its exact revision (overtaken: %s)', async (overtaken) => {
  show(<ApprovalModePicker thread={thread} />);
  const picker = await open();
  server.rows = [row('ask', 4)];
  server.parameters = parameters('ask');
  server.write.mockImplementationOnce(() => {
    const current = server.rows[0];
    const error = new ApiError({ type: 'orch:error:conflict.stale_revision', title: 'The setting changed', status: 409, code: 'conflict.stale_revision', detail: 'The setting changed.', request_id: 'r1', current, changed_by: { id: 'grace', display_name: 'Grace' } }, null);
    if (overtaken) server.rows = [row('ask', 5)];
    return Promise.reject(error);
  });
  await userEvent.click(within(picker).getByRole('button', { name: 'Ask' }));
  const notice = await within(picker).findByRole('status');
  expect(notice).toHaveTextContent(overtaken ? 'The mode changed to Ask.' : 'Grace changed the mode to Ask.');
  expect(notice).not.toHaveTextContent(/model|Bravo/);
  expect(within(picker).getByRole('button', { name: 'Ask', pressed: true })).toBeVisible();
  expect(server.write).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(within(picker).getByRole('button', { name: 'On request' })).toBeEnabled());
  await userEvent.click(within(picker).getByRole('button', { name: 'On request' }));
  await waitFor(() => expect(server.write).toHaveBeenCalledTimes(2));
  expect(server.write).toHaveBeenLastCalledWith(thread.id, 'approval.mode', { value: 'on_request', expected_revision: overtaken ? 5 : 4 });
});

it.each(['viewer', 'locked'] as const)('shows the mode read-only for %s without submitting a setting', async (why) => {
  if (why === 'locked') server.rows = [{ ...row('on_request', 3), locked_by: { scope: 'org', node: { id: 'org-1', kind: 'org', name: 'Acme', redacted: false } }, writable: false }];
  show(<ApprovalModePicker thread={thread} readOnly={why === 'viewer'} />);
  await userEvent.click(await screen.findByRole('button', { name: 'Approval mode: On request' }));
  const picker = screen.getByRole('dialog', { name: 'Approval mode' });
  const ask = await within(picker).findByRole('button', { name: 'Ask' });
  await waitFor(() => expect(ask).toBeDisabled());
  expect(within(picker).getByText(why === 'viewer' ? 'You can watch this thread. The access needed to work with it is listed above.' : 'Decided by Acme')).toBeVisible();
  await userEvent.click(ask);
  expect(server.write).not.toHaveBeenCalled();
});
