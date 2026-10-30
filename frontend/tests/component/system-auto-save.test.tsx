/** Management's scalar controls share the registry's guarded one-answer automatic save/Undo path. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Admin from '@/api/admin';
import type * as Settings from '@/api/settings';
import { NumberSetting, SwitchSetting } from '@/features/admin/SettingRows';
import type { ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { TYPING_PAUSE_MS } from '@/lib/debounce';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<ResolvedSetting[]>>(), put: vi.fn<typeof Settings.putSetting>(), clear: vi.fn<typeof Settings.clearSetting>() }));
vi.mock('@/api/admin', async (load) => ({ ...await load<typeof Admin>(), systemSettingsQuery: { queryKey: ['admin', 'settings'], queryFn: calls.read } }));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof Settings>(), putSetting: calls.put, clearSetting: calls.clear }));
const appearance = useAppearance.getState();
const row = (key: string, value: unknown, revision = 7): ResolvedSetting => ({ key, value, own: { value, locked: false, revision },
  source: { scope: 'system', node: null }, locked_by: null, writable: true, parts: [] });
beforeEach(() => { vi.clearAllMocks(); calls.put.mockReset(); calls.clear.mockReset(); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

for (const [locale, theme] of [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const) {
  it(`saves a valid number without a Save button and fences Undo in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    calls.read.mockResolvedValue([row('hosts.full_percent', 50)]);
    calls.put.mockResolvedValueOnce(row('hosts.full_percent', 95, 8)).mockResolvedValueOnce(row('hosts.full_percent', 50, 9));
    const user = userEvent.setup();
    show(<NumberSetting settingKey="hosts.full_percent" label={i18n.t('admin.hosts.fullFrom')} min={1} max={100} />);
    const number = await screen.findByRole('spinbutton', { name: i18n.t('admin.hosts.fullFrom') });
    expect(calls.put).not.toHaveBeenCalled();
    await user.clear(number);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, TYPING_PAUSE_MS * 2)); });
    expect(calls.put).not.toHaveBeenCalled();
    await user.type(number, '95');
    const undoName = i18n.t('settings.compact.undoNamed', { name: i18n.t('admin.hosts.fullFrom') });
    await screen.findByRole('button', { name: undoName });
    expect(calls.put).toHaveBeenCalledExactlyOnceWith({ kind: 'system' }, 'hosts.full_percent', { value: 95, locked: false, expectedRevision: 7 });
    expect(number).toHaveValue(95);
    expect(screen.queryByRole('button', { name: i18n.t('common.save') })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: undoName }));
    await waitFor(() => expect(number).toHaveValue(50));
    expect(calls.put).toHaveBeenLastCalledWith({ kind: 'system' }, 'hosts.full_percent', { value: 50, locked: false, expectedRevision: 8 });
    expect(calls.read).toHaveBeenCalledTimes(1);
  });

  it(`restores absence by guarded DELETE after the first switch override in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const original = { ...row('auth.local_login', false), own: null };
    calls.read.mockResolvedValue([original]); calls.put.mockResolvedValue(row('auth.local_login', true, 1)); calls.clear.mockResolvedValue(original);
    const user = userEvent.setup(); show(<SwitchSetting settingKey="auth.local_login" label={i18n.t('admin.signIn.password.label')} />);
    const control = await screen.findByRole('switch', { name: i18n.t('admin.signIn.password.label') });
    await user.click(control);
    const undoName = i18n.t('settings.compact.undoNamed', { name: i18n.t('admin.signIn.password.label') });
    await screen.findByRole('button', { name: undoName });
    expect(control).toBeChecked();
    expect(calls.put).toHaveBeenCalledExactlyOnceWith({ kind: 'system' }, 'auth.local_login', { value: true, locked: false, expectedRevision: null });
    await user.click(screen.getByRole('button', { name: undoName }));
    await waitFor(() => expect(control).not.toBeChecked());
    expect(calls.clear).toHaveBeenCalledExactlyOnceWith({ kind: 'system' }, 'auth.local_login', 1);
    expect(calls.read).toHaveBeenCalledTimes(1);
  });
}
