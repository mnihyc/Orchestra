/** The composer uses only offered native modes, writes the displayed revision, and refuses an unseen competing choice. */
import { act, screen, waitFor, within } from '@testing-library/react';
import { QueryClient } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import type * as SettingsApi from '@/api/settings';
import type * as ThreadsApi from '@/api/threads';
import { ApprovalModePicker } from '@/features/composer/ApprovalMode';
import { i18n, setLocale } from '@/i18n';
import { offeredChoice } from '@/i18n/choices';
import type { NodeSettingSetBody, ResolvedSetting, Thread, ThreadParameters } from '@/generated/api-types';
import { show } from './support';
import { nativeModeSetting, modeParameters } from '../doubles/mode-controls';

const server = vi.hoisted(() => ({ rows: [] as ResolvedSetting[], parameters: {} as ThreadParameters, settingReads: vi.fn(), parameterReads: vi.fn(), write: vi.fn<(id: string, key: string, body: NodeSettingSetBody) => Promise<ResolvedSetting>>() }));
vi.mock('@/api/settings', async (importOriginal) => ({ ...await importOriginal<typeof SettingsApi>(), settingsQuery: (id: string) => ({ queryKey: ['settings', id], queryFn: () => { server.settingReads(); return Promise.resolve(server.rows); }, staleTime: Infinity }) }));
vi.mock('@/api/threads', async (importOriginal) => ({ ...await importOriginal<typeof ThreadsApi>(),
  parametersQuery: (id: string) => ({ queryKey: ['thread-parameters', id], queryFn: () => { server.parameterReads(); return Promise.resolve(server.parameters); }, staleTime: Infinity }),
  setThreadSetting: (id: string, key: string, body: NodeSettingSetBody) => server.write(id, key, body),
}));
vi.mock('@/realtime/live', async (loadOriginal) => ({ ...await loadOriginal<object>(), refreshThread: vi.fn() }));

const row = (value: string, revision: number) => nativeModeSetting('approval.mode', value, revision);
const parameters = modeParameters;
const thread = { id: 'thread-1', archived: false, side_of: null, binding: { provider: { id: 'provider-1' } }, owner: { display_name: 'Ada' }, access: { permissions: ['thread.manage'] } } as Thread;

it.each(['en', 'zh-CN'] as const)('keeps the declared Auto choice unavailable when reviewer access changes in %s', async (locale) => {
  await setLocale(locale);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const declarations = [
    { mode: 'on_request', available: true, reason: null, requirement: null },
    { mode: 'auto', available: true, reason: null, requirement: null },
  ];
  server.parameters = Object.assign(parameters('on_request'), { approval_modes: ['on_request', 'auto'], approval_choices: declarations });
  const view = show(<ApprovalModePicker thread={thread} />, cache);
  try {
    const name = (mode: string) => offeredChoice(i18n.t, 'approvalMode', mode)?.name ?? mode;
    await userEvent.click(await screen.findByRole('button', { name: i18n.t('composer.mode.label', { mode: name('on_request') }) }));
    const picker = await screen.findByRole('dialog', { name: i18n.t('composer.mode.title') });
    const auto = await within(picker).findByRole('button', { name: name('auto') });
    expect(auto).toBeEnabled();
    act(() => { cache.setQueryData(['thread-parameters', thread.id], {
      ...server.parameters, approval_modes: ['on_request'], approval_choices: [declarations[0], {
        mode: 'auto', available: false, reason: 'reviewer_access_required', requirement: 'reviewer_use',
      }],
    }); });
    await waitFor(() => expect(auto).toBeDisabled());
    expect(auto).toHaveAccessibleDescription(i18n.t('composer.approvalUnavailable.reviewerAccess'));
    expect(picker).toHaveTextContent(i18n.t('composer.approvalRequirements.reviewerUse'));
    await userEvent.click(auto);
    expect(server.write).not.toHaveBeenCalled();
  } finally { view.unmount(); cache.clear(); await setLocale('en'); }
});

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

it('offers a served Auto mode and saves the current setting revision before a native session reports its reviewer', async () => {
  server.parameters = {
    ...parameters('on_request'),
    approval_modes: ['on_request', 'auto'],
    approval_reviewer: null,
  };
  show(<ApprovalModePicker thread={thread} />);
  await userEvent.click(await screen.findByRole('button', { name: 'Approval mode: On request' }));
  const picker = screen.getByRole('dialog', { name: 'Approval mode' });
  const auto = await within(picker).findByRole('button', { name: 'Auto' });
  await waitFor(() => expect(auto).toBeEnabled());
  expect(within(picker).queryByRole('button', { name: 'Bypass' })).not.toBeInTheDocument();
  await userEvent.click(auto);
  await waitFor(() => expect(server.write).toHaveBeenCalledExactlyOnceWith(thread.id, 'approval.mode', { value: 'auto', expected_revision: 3 }));
  expect(await screen.findByRole('button', { name: 'Approval mode: Auto' })).toBeVisible();
  expect(screen.queryByRole('dialog', { name: 'Approval mode' })).not.toBeInTheDocument();
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
