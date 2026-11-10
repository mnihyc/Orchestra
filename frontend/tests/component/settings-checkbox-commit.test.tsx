/** A compound setting's explicit keyboard commit survives the checkbox's native Enter guard. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { putSetting } from '@/api/settings';
import { SettingRow } from '@/features/settings/SettingRow';
import type { KeyDef } from '@/features/settings/settingsModel';
import type { ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/api/settings', async (load) => ({ ...await load<object>(), putSetting: vi.fn() }));
const def: KeyDef = { key: 'invitations.forms', title: 'Invitation forms', description: 'Allowed invitation delivery methods.',
  group: 'access', class: 'policy', scopes: ['org'], merge: 'intersect', default: ['email', 'link'], example: null,
  schema: { type: 'array', items: { enum: ['email', 'link'] }, uniqueItems: true }, text_key: null, keywords: [] };
const target = { kind: 'node', id: 'invitation-org' } as const;
const setting: ResolvedSetting = { key: def.key, value: ['email', 'link'], own: { value: ['email', 'link'], revision: 3, locked: false },
  source: { scope: 'org', node: { id: target.id, kind: 'org', name: 'An org', redacted: false } }, parts: [], locked_by: null, writable: true };
const caches: QueryClient[] = [];
beforeEach(() => { vi.clearAllMocks(); });
afterEach(async () => { cleanup(); for (const cache of caches.splice(0)) cache.clear(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const modifier of ['Control', 'Meta'] as const) {
  it(`commits only the edited choices once with ${modifier}+Enter in ${locale}`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    vi.mocked(putSetting).mockResolvedValue({ ...setting, value: ['link'], own: { value: ['link'], revision: 4, locked: false } });
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    caches.push(cache);
    show(<QueryClientProvider client={cache}><ul><SettingRow def={def} setting={setting} target={target}
      place={{ name: 'An org', kind: 'org', owner: null, access: { permissions: ['settings.manage'] } }} /></ul></QueryClientProvider>);
    const edit = screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: def.title }) });
    await user.click(edit);
    const checkbox = screen.getAllByRole('checkbox')[0];
    if (checkbox === undefined) throw new Error('The compound setting must expose its first declared choice.');
    await user.click(checkbox);
    expect(checkbox).not.toBeChecked();
    await user.keyboard('{Enter}');
    expect(checkbox).not.toBeChecked();
    expect(putSetting).not.toHaveBeenCalled();
    fireEvent.keyDown(checkbox, { key: 'Enter', ctrlKey: modifier === 'Control', metaKey: modifier === 'Meta', isComposing: true });
    expect(putSetting).not.toHaveBeenCalled();
    await user.keyboard(`{${modifier}>}{Enter}{/${modifier}}`);
    await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: ['link'], expectedRevision: 3 }));
    await waitFor(() => expect(edit).toHaveAttribute('aria-expanded', 'false'));
  });
}
