/** System channels use the ordinary notifier form, explicit tests and native scope-bound writes. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Settings from '@/api/settings';
import type * as Auth from '@/api/auth';
import { ApiError } from '@/api/client';
import { Channels } from '@/features/admin/Channels';
import type { NotificationChannel, Notifier } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { meta } from '../doubles/fixtures';
import { systemChannel, systemNotifier } from './admin-system-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<NotificationChannel[]>>(), notifiers: vi.fn<() => Promise<Notifier[]>>(), scope: vi.fn<(scope: string) => void>(),
  create: vi.fn<typeof Settings.createSystemChannel>(), orgCreate: vi.fn<typeof Settings.createOrgChannel>(), update: vi.fn<typeof Settings.updateChannel>(),
  remove: vi.fn<typeof Settings.deleteChannel>(), test: vi.fn<typeof Settings.testChannel>(), policies: vi.fn() }));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof Settings>(),
  channelsQuery: (scope: string) => { calls.scope(scope); return { queryKey: ['channels', scope], queryFn: calls.read }; },
  notifiersQuery: { queryKey: ['notifiers'], queryFn: calls.notifiers },
  createSystemChannel: calls.create, createOrgChannel: calls.orgCreate, updateChannel: calls.update, deleteChannel: calls.remove, testChannel: calls.test,
}));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(), metaQuery: { queryKey: ['meta'], queryFn: () => Promise.resolve(meta()) } }));
vi.mock('@/features/settings/Editor', () => ({ SettingsEditor: (props: { target: Settings.SettingsTarget; only?: readonly string[] }) => { calls.policies(props); return null; } }));
beforeEach(() => {
  vi.clearAllMocks(); calls.read.mockReset().mockResolvedValue([]); calls.notifiers.mockReset().mockResolvedValue([systemNotifier()]);
  calls.create.mockReset().mockResolvedValue(systemChannel()); calls.update.mockReset(); calls.remove.mockReset().mockResolvedValue(undefined);
  calls.test.mockReset().mockResolvedValue({ ok: false, error: 'The synthetic relay refused this sender.' });
});
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`creates a system channel with write-only secret fields and updates the list from its answer in ${locale}`, async () => {
    await setLocale(locale); const user = userEvent.setup(); show(<Channels />);
    await waitFor(() => expect(calls.read).toHaveBeenCalledOnce());
    expect(calls.scope).toHaveBeenLastCalledWith('system');
    expect(calls.policies).toHaveBeenLastCalledWith(expect.objectContaining({ target: { kind: 'system' }, only: ['mail.shared', 'outbound.allowed_private_hosts'] }));
    await user.click(screen.getByRole('button', { name: i18n.t('settings.channels.add') }));
    const dialog = screen.getByRole('dialog');
    await user.type(within(dialog).getByRole('textbox', { name: i18n.t('settings.channels.name') }), 'System relay');
    const secret = within(dialog).getByLabelText('Relay token');
    expect(secret).toHaveAttribute('type', 'password');
    await user.type(secret, 'synthetic-relay-test-value');
    await user.click(within(dialog).getByRole('button', { name: i18n.t('settings.channels.make') }));
    await waitFor(() => expect(calls.create).toHaveBeenCalledExactlyOnceWith('relay', { name: 'System relay', config: {}, secrets: { token: 'synthetic-relay-test-value' } }, expect.any(String)));
    expect(await screen.findByRole('heading', { name: 'System relay' })).toBeVisible();
    expect(screen.queryByDisplayValue('synthetic-relay-test-value')).not.toBeInTheDocument();
    expect(calls.orgCreate).not.toHaveBeenCalled(); expect(calls.read).toHaveBeenCalledOnce();
  });

  it(`retains a refused system channel draft and retries the same native scope in ${locale}`, async () => {
    await setLocale(locale);
    calls.create.mockRejectedValueOnce(new ApiError({ type: 'orch:error:internal', code: 'internal', title: 'Write failed', detail: 'The synthetic channel write failed.', status: 500, request_id: 'relay' }, null));
    const user = userEvent.setup(); show(<Channels />);
    await waitFor(() => expect(calls.read).toHaveBeenCalledOnce());
    await user.click(screen.getByRole('button', { name: i18n.t('settings.channels.add') }));
    const dialog = screen.getByRole('dialog');
    await user.type(within(dialog).getByRole('textbox', { name: i18n.t('settings.channels.name') }), 'System relay');
    const secret = within(dialog).getByLabelText('Relay token');
    await user.type(secret, 'synthetic-relay-test-value');
    const create = within(dialog).getByRole('button', { name: i18n.t('settings.channels.make') });
    await user.click(create);
    expect(await within(dialog).findByRole('alert')).toHaveTextContent(i18n.t('errors.internal'));
    expect(secret).toHaveValue('synthetic-relay-test-value');
    await user.click(create);
    await screen.findByRole('heading', { name: 'System relay' });
    expect(calls.create).toHaveBeenCalledTimes(2);
    expect(calls.create.mock.calls[1]?.slice(0, 2)).toEqual(calls.create.mock.calls[0]?.slice(0, 2));
    expect(calls.orgCreate).not.toHaveBeenCalled(); expect(calls.read).toHaveBeenCalledOnce();
  });

  it(`edits, disables, explicitly tests and removes the existing system channel in ${locale}`, async () => {
    await setLocale(locale); const original = systemChannel(); calls.read.mockResolvedValue([original]);
    calls.update.mockResolvedValueOnce(systemChannel({ name: 'Changed relay' })).mockResolvedValueOnce(systemChannel({ name: 'Changed relay', enabled: false }));
    const user = userEvent.setup(); show(<Channels />);
    await screen.findByRole('heading', { name: original.name });
    expect(calls.test).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: i18n.t('settings.channels.editNamed', { name: original.name }) }));
    let dialog = screen.getByRole('dialog');
    const optional = i18n.t('ui.field.optional').replace(/[.*+?^${}()|[\]\\]/gu, '\\$&');
    const keptSecret = within(dialog).getByLabelText(new RegExp(`^Relay token\\s*${optional}$`, 'u'));
    expect(keptSecret).toHaveValue('');
    expect(keptSecret).toHaveAttribute('type', 'password');
    const name = within(dialog).getByRole('textbox', { name: i18n.t('settings.channels.name') });
    await user.clear(name); await user.type(name, 'Changed relay');
    await user.click(within(dialog).getByRole('button', { name: i18n.t('settings.save') }));
    await screen.findByRole('heading', { name: 'Changed relay' });
    expect(calls.update).toHaveBeenCalledWith(original.id, { name: 'Changed relay', config: {} });
    await user.click(screen.getByRole('switch', { name: i18n.t('day100.names.enableDelivery') }));
    await waitFor(() => expect(calls.update).toHaveBeenLastCalledWith(original.id, { enabled: false }));
    await waitFor(() => expect(screen.getByRole('switch')).not.toBeChecked());
    await user.click(screen.getByRole('button', { name: i18n.t('settings.channels.testNamed', { name: 'Changed relay' }) }));
    dialog = screen.getByRole('dialog');
    await user.type(within(dialog).getByRole('textbox', { name: i18n.t('settings.channels.testTo') }), 'qa@example.test');
    await user.click(within(dialog).getByRole('button', { name: i18n.t('settings.channels.sendTest') }));
    expect(await within(dialog).findByText('The synthetic relay refused this sender.')).toBeVisible();
    expect(calls.test).toHaveBeenCalledExactlyOnceWith(original.id, 'qa@example.test');
    await user.keyboard('{Escape}');
    await user.click(screen.getByRole('button', { name: i18n.t('settings.channels.removeNamed', { name: 'Changed relay' }) }));
    dialog = screen.getByRole('alertdialog', { name: i18n.t('settings.channels.removeTitle', { name: 'Changed relay' }) });
    await user.click(within(dialog).getByRole('button', { name: i18n.t('settings.channels.removeConfirm', { name: 'Changed relay' }) }));
    await waitFor(() => expect(calls.remove).toHaveBeenCalledExactlyOnceWith(original.id));
    expect(screen.queryByRole('heading', { name: 'Changed relay' })).not.toBeInTheDocument();
    expect(calls.read).toHaveBeenCalledOnce(); expect(calls.orgCreate).not.toHaveBeenCalled();
  });
}
