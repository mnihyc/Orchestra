/** System is a local scope choice, never a fabricated name result or a fallback to personal usage. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ScopePicker } from '@/features/usage/ScopePicker';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';
vi.mock('@/api/search', () => ({ namesQuery: () => ({ queryKey: ['system-scope-names'], queryFn: () => Promise.resolve([]) }) }));
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`offers active system reach through the same scope combobox (${locale})`, async () => {
    await setLocale(locale); const changed = vi.fn(), user = userEvent.setup(); const view = show(<ScopePicker scope={{ kind: 'me' }} name={null} allowSystem onChange={changed} />);
    await user.click(screen.getByRole('combobox', { name: i18n.t('usage.scope.label') })); await user.click(await screen.findByRole('option', { name: i18n.t('usage.scope.system') }));
    expect(changed).toHaveBeenCalledExactlyOnceWith('system'); view.rerender(<ScopePicker scope={{ kind: 'system' }} name={null} allowSystem onChange={changed} />);
    expect(screen.getByRole('combobox', { name: i18n.t('usage.scope.label') })).toHaveValue(i18n.t('usage.scope.system')); expect(screen.getAllByRole('combobox')).toHaveLength(1);
  });
  it(`does not offer System without current system reach (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup(); show(<ScopePicker scope={{ kind: 'me' }} name={null} onChange={() => undefined} />);
    await user.click(screen.getByRole('combobox', { name: i18n.t('usage.scope.label') })); expect(screen.queryByRole('option', { name: i18n.t('usage.scope.system') })).not.toBeInTheDocument();
  });
}
