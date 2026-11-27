/** Disable from a provider's menu acts at once: nothing is asked first; one line says the running threads it stopped,
 * Show uses opens the threads bound to it, and Undo switches it on again. A selection is switched off the same way. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as ResourceApi from '@/api/resources';
import type { MenuEntry } from '@/ui/Menu';
import { useProviderActs } from '@/features/resources/providers/acts';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ disable: vi.fn(), enable: vi.fn(), navigate: vi.fn() }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourceApi>(), disableProvider: calls.disable, enableProvider: calls.enable }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), useNavigate: () => calls.navigate }));

const provider = { ...planProvider(), running_threads: 2 };

/** The provider's menu as buttons, and the selection's Disable beside them. */
function Menu() {
  const acts = useProviderActs();
  const entries = acts.list.of(provider).entries.filter((entry): entry is Extract<MenuEntry, { onSelect: () => void }> => 'onSelect' in entry);
  const bulk = acts.list.bulk?.find((one) => one.id === 'disable');
  return <>
    {entries.map((entry) => <button key={entry.id} type="button" onClick={entry.onSelect}>{entry.label}</button>)}
    <button type="button" onClick={() => void bulk?.run?.(provider)}>selection</button>
    {bulk?.confirm === undefined ? null : <p>asks first</p>}
    {acts.dialogs}
  </>;
}

beforeEach(() => {
  vi.clearAllMocks();
  calls.disable.mockResolvedValue({ ...provider, status: 'disabled' });
  calls.enable.mockResolvedValue(provider);
});
afterEach(() => { cleanup(); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`switches the provider off at once with one line, Show uses and Undo (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    show(<Menu />);
    await user.click(screen.getByRole('button', { name: i18n.t('resources.act.disableNow') }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(calls.disable).toHaveBeenCalledExactlyOnceWith(provider.id);
    const line = (await screen.findAllByText(i18n.t('providers.danger.disabled', { name: provider.name }))).map((one) => one.closest('li')).filter((one) => one !== null).at(-1);
    if (line === undefined) throw new Error('no line');
    expect(line).toHaveTextContent(i18n.t('trash.stopped', { count: 2 }));
    await user.click(within(line).getByRole('button', { name: i18n.t('resourceImpact.showUses') }));
    expect(calls.navigate).toHaveBeenCalledWith({ to: '/resources/providers/$id', params: { id: provider.id }, search: { tab: 'threads' } });
    await user.click(within(line).getByRole('button', { name: i18n.t('places.toast.undo') }));
    await vi.waitFor(() => expect(calls.enable).toHaveBeenCalledExactlyOnceWith(provider.id));
    expect((await screen.findAllByText(i18n.t('providers.danger.enabledAgain', { name: provider.name }))).length).toBeGreaterThan(0);
  });
}

it('a selection is switched off without a question first', async () => {
  await setLocale('en');
  const user = userEvent.setup();
  show(<Menu />);
  expect(screen.queryByText('asks first')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'selection' }));
  expect(calls.disable).toHaveBeenCalledExactlyOnceWith(provider.id);
});
